use portable_pty::CommandBuilder;
use serde_json::Value;
use std::{
    env,
    ffi::OsStr,
    io::{self, Read},
    os::fd::AsRawFd,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub fn executable() -> std::ffi::OsString {
    env::var_os("KIRO_AGENTS_KIRO_BIN").unwrap_or_else(|| "kiro-cli".into())
}

pub fn launch(cwd: &str, agent: Option<&str>) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable());
    command.args(["chat", "--v2", "--tui"]);
    if let Some(agent) = agent {
        command.args(["--agent", agent]);
    }
    command.cwd(cwd);
    command
}

/// How long a captured Kiro capability command may run before it is stopped.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// Reading stays bounded so a chatty or looping Kiro can never exhaust the board.
const MAX_LIST_BYTES: usize = 256 * 1024;
/// Only this many agent names are offered in one picker listing.
pub const MAX_AGENTS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentList {
    pub names: Vec<String>,
    /// True when Kiro returned more rows than the picker keeps.
    pub truncated: bool,
    /// True when Kiro's output shape is unknown, so the picker offers no
    /// names and falls back to the Kiro default plus explicit typing.
    pub unrecognized: bool,
}

/// Printed form of a failed discovery. Never contains Kiro's stderr text,
/// which can carry account or auth details.
#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error("Agent discovery cancelled")]
    Cancelled,
    #[error("Cannot start Kiro agent listing: {0}")]
    Spawn(String),
    #[error("Kiro agent listing timed out after {}s. Ctrl+R retries.", .0.as_secs())]
    Timeout(Duration),
    #[error("Kiro agent listing failed (exit {}). Log in to Kiro and press Ctrl+R to retry.", code.map_or_else(|| "unknown".to_owned(), |code| code.to_string()))]
    Failed { code: Option<i32> },
    #[error("Kiro agent listing produced too much output. Type an agent name instead.")]
    TooLarge,
}

/// Capture Kiro's agent listing for `cwd` without ever writing to the terminal:
/// stdout is piped, stderr is discarded, stdin is closed, and the child is
/// stopped at the timeout.
#[cfg(test)]
fn capture_agent_list(
    cwd: &Path,
    program: impl AsRef<OsStr>,
    timeout: Duration,
) -> Result<AgentList, ListError> {
    capture_agent_list_cancel(cwd, program, timeout, &AtomicBool::new(false))
}

fn capture_agent_list_cancel(
    cwd: &Path,
    program: impl AsRef<OsStr>,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<AgentList, ListError> {
    let mut child = Command::new(program)
        .args(["agent", "list", "--format", "json-pretty"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| ListError::Spawn(error.kind().to_string()))?;
    let mut reaped = false;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ListError::Spawn("Kiro stdout is unavailable".to_owned()))?;
        let fd = stdout.as_raw_fd();
        // SAFETY: stdout owns this valid descriptor throughout the capture.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(ListError::Spawn(
                "Cannot configure Kiro output capture".to_owned(),
            ));
        }
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut eof = false;
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(ListError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(ListError::Timeout(timeout));
            }
            match stdout.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if bytes.len() + n > MAX_LIST_BYTES {
                        return Err(ListError::TooLarge);
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    continue;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(ListError::Spawn("Cannot read Kiro output".to_owned())),
            }
            // Do not reap while a descendant still holds stdout: keeping our
            // child unreaped makes its process group safe to stop at timeout.
            if eof {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| ListError::Spawn(error.kind().to_string()))?
                {
                    reaped = true;
                    if !status.success() {
                        return Err(ListError::Failed {
                            code: status.code(),
                        });
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    let mut names = parse_agent_names(&text);
                    let empty_listing = serde_json::from_str::<Value>(&text).is_ok_and(|value| {
                        matches!(&value, Value::Array(items) if items.is_empty())
                            || ["agents", "profiles", "data", "items"].iter().any(|key| {
                                matches!(value.get(key), Some(Value::Array(items)) if items.is_empty())
                            })
                    });
                    let unrecognized = names.is_empty() && !empty_listing;
                    let truncated = names.len() > MAX_AGENTS;
                    names.truncate(MAX_AGENTS);
                    return Ok(AgentList {
                        names,
                        truncated,
                        unrecognized,
                    });
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() && !reaped {
        // SAFETY: we created the process group and have not reaped its leader
        // on timeout/read failure. No foreign Kiro process is targeted.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Extract agent names from plausible `agent list --format json-pretty` shapes.
///
/// The exact schema is unverified (findings: profile enumeration is blocked by
/// login), so only name-bearing JSON is accepted. An unknown shape yields no
/// names: the picker then offers the Kiro default and explicit typing rather
/// than inventing profiles.
pub fn parse_agent_names(text: &str) -> Vec<String> {
    let value: Value = match serde_json::from_str(text.trim()) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut names = Vec::new();
    collect_names(&value, &mut names);
    names
}

fn collect_names(value: &Value, names: &mut Vec<String>) {
    if names.len() > MAX_AGENTS {
        return;
    }
    match value {
        Value::String(name) => push_name(name, names),
        Value::Array(items) => {
            for item in items {
                collect_names(item, names);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                push_name(name, names);
                return;
            }
            // Known container keys first, then a legacy name -> profile map.
            let mut descended = false;
            for key in ["agents", "profiles", "data", "items"] {
                if let Some(inner) = map.get(key) {
                    descended = true;
                    collect_names(inner, names);
                }
            }
            if descended {
                return;
            }
            for value in map
                .values()
                .filter(|value| value.get("name").and_then(Value::as_str).is_some())
            {
                collect_names(value, names);
            }
        }
        _ => (),
    }
}

fn push_name(name: &str, names: &mut Vec<String>) {
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|ch| !ch.is_control()) {
        return;
    }
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_owned());
    }
}

/// Let the explicit CLI command present Kiro's own listing and errors.
/// The board uses captured asynchronous discovery instead.
pub fn agents(cwd: &Path) -> anyhow::Result<()> {
    let status = Command::new(executable())
        .args(["agent", "list", "--format", "json-pretty"])
        .current_dir(cwd)
        .status()?;
    anyhow::ensure!(status.success(), "Kiro agent listing failed: {status}");
    Ok(())
}

/// One asynchronous discovery request. The board drops the receiver to cancel;
/// `token` lets it discard results that arrived after a retry or cancellation.
pub struct AgentScan {
    pub token: u64,
    pub cwd: String,
    pub result: Result<AgentList, String>,
}

pub struct AgentScanHandle {
    receiver: Receiver<AgentScan>,
    cancelled: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl std::ops::Deref for AgentScanHandle {
    type Target = Receiver<AgentScan>;
    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl Drop for AgentScanHandle {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn spawn_agent_scan(token: u64, cwd: &str, timeout: Duration) -> AgentScanHandle {
    spawn_agent_scan_using(token, cwd, timeout, executable())
}

fn spawn_agent_scan_using(
    token: u64,
    cwd: &str,
    timeout: Duration,
    program: std::ffi::OsString,
) -> AgentScanHandle {
    let (sender, receiver) = mpsc::channel();
    let cwd = cwd.to_owned();
    let cancelled = Arc::new(AtomicBool::new(false));
    let child_cancelled = cancelled.clone();
    let worker = thread::spawn(move || {
        let path = Path::new(&cwd);
        let result = if path.is_dir() {
            capture_agent_list_cancel(path, &program, timeout, &child_cancelled)
                .map_err(|error| error.to_string())
        } else {
            Err("Project directory is unavailable".to_owned())
        };
        // A closed channel means the board cancelled: nothing to deliver.
        let _ = sender.send(AgentScan { token, cwd, result });
    });
    AgentScanHandle {
        receiver,
        cancelled,
        worker: Some(worker),
    }
}

/// Read-only bounded subdirectory listing used by the directory picker.
/// Hidden and heavy directories are skipped; an unreadable directory lists
/// nothing instead of surfacing an error.
pub fn subdirectories(dir: &Path, limit: usize) -> (Vec<String>, bool) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return (Vec::new(), false),
    };
    let mut names = Vec::new();
    let mut truncated = false;
    for (examined, entry) in entries.enumerate() {
        if examined >= 2000 {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        // Symlinks are never treated as directories, so traversal stays bounded.
        let is_dir = std::fs::symlink_metadata(entry.path())
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        names.push(name);
    }
    names.sort();
    truncated |= names.len() > limit;
    names.truncate(limit);
    (names, truncated)
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "__pycache__",
    "cache",
    "venv",
];

pub fn doctor() -> anyhow::Result<()> {
    let version = Command::new(executable()).arg("--version").output()?;
    anyhow::ensure!(version.status.success(), "Kiro version check failed");
    std::io::Write::write_all(&mut std::io::stdout(), &version.stdout)?;
    let help = Command::new(executable())
        .args(["chat", "--help"])
        .output()?;
    let text = String::from_utf8_lossy(&help.stdout);
    anyhow::ensure!(
        help.status.success() && text.contains("--v2") && text.contains("--tui"),
        "This Kiro build must support chat --v2 --tui"
    );
    std::io::Write::write_all(
        &mut std::io::stdout(),
        b"V2 and native terminal UI supported. Authenticate separately with kiro-cli login.\n",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    // Serialize fixture writes and fork/exec: another thread can briefly inherit
    // an open fixture-writer descriptor until exec, producing Linux ETXTBSY.
    static COMMAND_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn parses_the_documented_and_plausible_shapes() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        // Shape the fake CLI fixture emits and the most likely real one.
        assert_eq!(
            parse_agent_names(r#"{"agents":[{"name":"reviewer"},{"name":"planner"}]}"#),
            ["reviewer", "planner"]
        );
        assert_eq!(
            parse_agent_names(r#"[{"name":"a"},{"name":"a"},{"name":"b"}]"#),
            ["a", "b"]
        );
        assert_eq!(parse_agent_names(r#"["one","two"]"#), ["one", "two"]);
        assert_eq!(
            parse_agent_names(r#"{"profiles":{"kiro_default":{"name":"kiro_default"}}}"#),
            ["kiro_default"]
        );
        // Unknown object keys are not evidence of an agent name.
        assert!(parse_agent_names(r#"{"metadata":{},"config":{"tools":[]}}"#).is_empty());
    }

    #[test]
    fn unknown_shapes_yield_no_names_instead_of_invented_profiles() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        assert!(parse_agent_names("").is_empty());
        assert!(parse_agent_names("not json at all").is_empty());
        assert!(parse_agent_names("null").is_empty());
        assert!(parse_agent_names(r#"{"agents":[{"id":1}]}"#).is_empty());
        assert!(parse_agent_names(r#"{"count":2}"#).is_empty());
        assert!(parse_agent_names(r#"[1,2,3]"#).is_empty());
    }

    fn fake_kiro(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("kiro-cli");
        fs::write(&path, format!("#!/bin/sh\ncat <<'EOF'\n{body}\nEOF\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn captured_listing_never_touches_the_parent_terminal() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[{"name":"reviewer"}]}"#);
        let list = capture_agent_list(dir.path(), &program, LIST_TIMEOUT).unwrap();
        assert_eq!(list.names, ["reviewer"]);
        assert!(!list.unrecognized);
        assert!(!list.truncated);
    }

    #[test]
    fn failing_and_unrecognized_listings_are_recoverable_text() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("kiro-cli");
        fs::write(
            &program,
            "#!/bin/sh\necho 'secret token leaked' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let error = capture_agent_list(dir.path(), &program, LIST_TIMEOUT).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("Ctrl+R"), "{text}");
        assert!(!text.contains("secret"), "{text}");

        let program = fake_kiro(dir.path(), "Kiro says hello in plain text");
        let list = capture_agent_list(dir.path(), &program, LIST_TIMEOUT).unwrap();
        assert!(list.names.is_empty() && list.unrecognized);
    }

    #[test]
    fn hung_listing_is_stopped_at_the_timeout() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("kiro-cli");
        fs::write(&program, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let error =
            capture_agent_list(dir.path(), &program, Duration::from_millis(300)).unwrap_err();
        assert!(matches!(error, ListError::Timeout(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn oversized_listing_is_rejected_not_streamed() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("kiro-cli");
        fs::write(
            &program,
            "#!/bin/sh\nyes '{\"agents\":[{\"name\":\"x\"}],' | head -c 400000\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let error = capture_agent_list(dir.path(), &program, LIST_TIMEOUT).unwrap_err();
        assert!(matches!(error, ListError::TooLarge), "{error}");
    }

    #[test]
    fn async_scan_reports_its_cwd_and_token() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[{"name":"planner"}]}"#);
        let receiver = spawn_agent_scan_using(
            7,
            &dir.path().to_string_lossy(),
            LIST_TIMEOUT,
            program.into_os_string(),
        );
        let scan = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(scan.token, 7);
        assert_eq!(scan.cwd, dir.path().to_string_lossy());
        assert_eq!(scan.result.unwrap().names, ["planner"]);
    }

    #[test]
    fn cancelling_scan_reaps_the_listing_child_promptly() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("kiro-cli");
        fs::write(
            &program,
            "#!/bin/sh\necho $$ > listing.pid\nexec sleep 30\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let handle = spawn_agent_scan_using(
            1,
            &dir.path().to_string_lossy(),
            LIST_TIMEOUT,
            program.into_os_string(),
        );
        let marker = dir.path().join("listing.pid");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "listing never started");
            thread::sleep(Duration::from_millis(10));
        }
        let pid = fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        let started = Instant::now();
        drop(handle);
        assert!(started.elapsed() < Duration::from_secs(1));
        // SAFETY: signal 0 observes existence only; it sends no signal.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "listing child survived cancellation"
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn missing_directory_is_an_actionable_error() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let receiver = spawn_agent_scan(
            1,
            &dir.path().join("gone").to_string_lossy(),
            Duration::from_millis(200),
        );
        let scan = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(scan.result.unwrap_err().contains("unavailable"));
    }

    #[test]
    fn empty_agent_container_is_a_valid_listing() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[]}"#);
        let list = capture_agent_list(dir.path(), program, LIST_TIMEOUT).unwrap();
        assert!(list.names.is_empty());
        assert!(!list.unrecognized);
    }

    #[test]
    fn directory_scan_caps_examined_entries_even_when_none_are_directories() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for index in 0..2100 {
            fs::write(dir.path().join(format!("file-{index}")), "").unwrap();
        }
        let (names, truncated) = subdirectories(dir.path(), 200);
        assert!(names.is_empty());
        assert!(truncated, "entry budget must apply to ordinary files too");
    }

    #[test]
    fn listing_is_bounded_skips_heavy_trees_and_ignores_symlinks() {
        let _guard = COMMAND_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        for name in ["b", "a", "node_modules", ".git"] {
            fs::create_dir(dir.path().join(name)).unwrap();
        }
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let (names, truncated) = subdirectories(dir.path(), 10);
        assert_eq!(names, ["a", "b"]);
        assert!(!truncated);
        let (names, truncated) = subdirectories(dir.path(), 1);
        assert_eq!(names, ["a"]);
        assert!(truncated);
        let (names, _) = subdirectories(&dir.path().join("missing"), 10);
        assert!(names.is_empty());
    }
}
