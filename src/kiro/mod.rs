pub mod titles;

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
/// Cap on profile files examined per scope directory during the file fallback.
const MAX_PROFILE_FILES: usize = 500;
/// Only this many leading bytes of a profile file are read to find its name.
const MAX_PROFILE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentList {
    pub names: Vec<String>,
    /// True when Kiro returned more rows than the picker keeps.
    pub truncated: bool,
    /// True when the native listing is unavailable/unrecognized; scoped files
    /// may still supply candidates, with a visible fallback status.
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

/// Locations of the Kiro profile scopes for one cwd, snapshotted from the
/// environment once per discovery. Kiro documents `KIRO_AGENT_CONFIG_DIR` as a
/// global scan override that replaces the workspace scan. Tests construct a
/// scope explicitly instead of mutating the process environment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ProfileScope {
    /// `<cwd>/.kiro/agents`, absent when the override replaces the scan.
    workspace: Option<std::path::PathBuf>,
    /// `$KIRO_AGENT_CONFIG_DIR`, else `$KIRO_HOME/agents`, else `~/.kiro/agents`.
    global: Option<std::path::PathBuf>,
}

fn non_empty(var: &str) -> Option<std::path::PathBuf> {
    env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

impl ProfileScope {
    fn from_env(cwd: &Path) -> Self {
        if let Some(dir) = non_empty("KIRO_AGENT_CONFIG_DIR") {
            return Self {
                workspace: None,
                global: Some(dir),
            };
        }
        let global = non_empty("KIRO_HOME")
            .map(|home| home.join("agents"))
            .or_else(|| non_empty("HOME").map(|home| home.join(".kiro").join("agents")));
        let modern_workspace = cwd.join(".kiro/agents");
        let legacy_workspace = cwd.join(".amazonq/cli-agents");
        let workspace = if !modern_workspace.exists() && legacy_workspace.exists() {
            legacy_workspace
        } else {
            modern_workspace
        };
        let global = global.map(|modern| {
            if non_empty("KIRO_HOME").is_none() && !modern.exists() {
                if let Some(home) = non_empty("HOME") {
                    let legacy = home.join(".aws/amazonq/cli-agents");
                    if legacy.exists() {
                        return legacy;
                    }
                }
            }
            modern
        });
        Self {
            workspace: Some(workspace),
            global,
        }
    }

    /// Scopes in precedence order: workspace first, then global.
    fn dirs(&self) -> impl Iterator<Item = &Path> {
        [self.workspace.as_deref(), self.global.as_deref()]
            .into_iter()
            .flatten()
    }
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

/// Full discovery for one cwd: Kiro's own capability command first, then a
/// bounded read-only scan of the genuine scoped profile files. The file scan
/// supplements CLI results (the listing is login-gated and can omit profiles
/// Kiro would still accept) and is the only source when the command fails.
/// Nothing is ever written. Nested names are bounded; directory symlinks are
/// never traversed, and profile-file symlinks are read only within the file budget.
fn discover_agents_in(
    cwd: &Path,
    program: impl AsRef<OsStr>,
    timeout: Duration,
    cancelled: &AtomicBool,
    scope: &ProfileScope,
) -> Result<AgentList, ListError> {
    let cli = capture_agent_list_cancel(cwd, program, timeout, cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Err(ListError::Cancelled);
    }
    if let Err(error @ (ListError::Cancelled | ListError::TooLarge)) = cli {
        return Err(error);
    }
    let mut fallback = Vec::new();
    let mut scan_hit_cap = false;
    for dir in scope.dirs() {
        scan_profiles_cancel(dir, &mut fallback, &mut scan_hit_cap, cancelled);
        if cancelled.load(Ordering::Relaxed) {
            return Err(ListError::Cancelled);
        }
    }
    match cli {
        Ok(mut list) => {
            for name in fallback {
                push_name(&name, &mut list.names);
            }
            list.truncated |= scan_hit_cap || list.names.len() > MAX_AGENTS;
            list.names.truncate(MAX_AGENTS);
            Ok(list)
        }
        // Cancellation must stay immediate and oversized output stays unsafe;
        // every other listing failure falls back to the scoped files.
        Err(error @ (ListError::Cancelled | ListError::TooLarge)) => Err(error),
        Err(error) => {
            if fallback.is_empty() {
                return Err(error);
            }
            let truncated = scan_hit_cap || fallback.len() > MAX_AGENTS;
            fallback.truncate(MAX_AGENTS);
            Ok(AgentList {
                names: fallback,
                truncated,
                unrecognized: true,
            })
        }
    }
}

/// Bounded traversal of profile scopes, including documented team/name profiles.
/// Symlinked files are read-only candidates; symlinked directories are never walked.
fn scan_profiles_cancel(
    dir: &Path,
    names: &mut Vec<String>,
    hit_cap: &mut bool,
    cancelled: &AtomicBool,
) {
    let mut pending = vec![(dir.to_owned(), 0usize)];
    let mut examined = 0usize;
    let mut bytes = 0u64;
    while let Some((current, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut entries_sorted = Vec::new();
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if examined >= MAX_PROFILE_FILES {
                *hit_cap = true;
                break;
            }
            examined += 1;
            let Ok(entry) = entry else { continue };
            entries_sorted.push(entry.path());
        }
        entries_sorted.sort();
        for path in entries_sorted {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            let Ok(link_info) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if link_info.is_dir() {
                if depth < 4 {
                    pending.push((path, depth + 1));
                } else {
                    *hit_cap = true;
                }
                continue;
            }
            // Follow a profile-file symlink only; never recurse through one.
            let Ok(info) = std::fs::metadata(&path) else {
                continue;
            };
            if !info.is_file() {
                continue;
            }
            if !matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("json" | "md")
            ) {
                continue;
            }
            if info.len() > MAX_PROFILE_BYTES || bytes + info.len() > 8 * 1024 * 1024 {
                *hit_cap = true;
                continue;
            }
            bytes += info.len();
            if let Some(mut name) = profile_name(&path) {
                if let Ok(relative) = path.strip_prefix(dir) {
                    if relative.components().count() > 1 {
                        let relative = relative.with_extension("");
                        let Some(text) = relative.to_str() else {
                            continue;
                        };
                        if text.starts_with('-') || text.chars().any(char::is_control) {
                            continue;
                        }
                        name = text.to_owned();
                    }
                }
                push_name(&name, names);
            }
        }
    }
}

#[cfg(test)]
fn scan_profile_dir(dir: &Path, names: &mut Vec<String>, hit_cap: &mut bool) {
    scan_profiles_cancel(dir, names, hit_cap, &AtomicBool::new(false));
}

/// Name a genuine profile file declares: `.json` `"name"`, `.md` frontmatter
/// `name:`, otherwise the file stem (Kiro's profile identity is its filename).
/// Nothing is synthesized: only names present in the user's own files.
fn profile_name(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    // Lock files, leases and drafts start with '.'; only .json/.md are profiles.
    if file_name.starts_with('.') {
        return None;
    }
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "json" | "md") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let Ok(file) = std::fs::File::open(path) else {
        return None;
    };
    let mut buffer = Vec::new();
    let mut taken = file.take(MAX_PROFILE_BYTES);
    if taken.read_to_end(&mut buffer).is_err() {
        return None;
    }
    let text = std::str::from_utf8(&buffer).ok()?;
    let declared = if extension == "json" {
        let value = serde_json::from_str::<Value>(text.trim()).ok()?;
        if !value.is_object() {
            return None;
        }
        value.get("name").and_then(Value::as_str).map(str::to_owned)
    } else {
        frontmatter_name(text)
    };
    // A declared name must itself be usable; otherwise the genuine file stem
    // is the fallback. Nothing is invented when both are unusable.
    let declared = declared.filter(|name| usable_profile_name(name));
    let candidate = declared.as_deref().unwrap_or(stem);
    usable_profile_name(candidate).then(|| candidate.to_owned())
}

fn frontmatter_name(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines.take(100) {
        let line = line.trim_end();
        if line == "---" || line == "..." {
            break;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(|ch| ch == '"' || ch == '\'');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn usable_profile_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains('/')
        && name.chars().all(|ch| !ch.is_control())
}

/// Extract agent names from plausible `agent list --format json-pretty` shapes.
///
/// The exact schema is unverified (findings: profile enumeration is blocked by
/// login), so only name-bearing JSON is accepted. An unknown shape yields no
/// names: discovery then supplements from the genuine scoped profile files
/// rather than inventing profiles.
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

/// Bounded list of declared profile names in one directory, for tests and
/// diagnostics. Discovery itself uses [`discover_agents`].
#[cfg(test)]
fn profile_names_in(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut hit_cap = false;
    scan_profile_dir(dir, &mut names, &mut hit_cap);
    names
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
    let scope = ProfileScope::from_env(Path::new(cwd));
    spawn_agent_scan_using(token, cwd, timeout, executable(), scope)
}

/// Test seam: discovery with an explicit program and profile scope, so tests
/// never read the ambient HOME/KIRO_HOME environment of a parallel run.
fn spawn_agent_scan_using(
    token: u64,
    cwd: &str,
    timeout: Duration,
    program: std::ffi::OsString,
    scope: ProfileScope,
) -> AgentScanHandle {
    let (sender, receiver) = mpsc::channel();
    let cwd = cwd.to_owned();
    let cancelled = Arc::new(AtomicBool::new(false));
    let child_cancelled = cancelled.clone();
    let worker = thread::spawn(move || {
        let path = Path::new(&cwd);
        let result = if path.is_dir() {
            discover_agents_in(path, &program, timeout, &child_cancelled, &scope)
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

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        // A failed assertion must not poison the shared fixture lock and make
        // every later command test report a bogus PoisonError.
        COMMAND_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn parses_the_documented_and_plausible_shapes() {
        let _guard = test_guard();
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
        let _guard = test_guard();
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
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[{"name":"reviewer"}]}"#);
        let list = capture_agent_list(dir.path(), &program, LIST_TIMEOUT).unwrap();
        assert_eq!(list.names, ["reviewer"]);
        assert!(!list.unrecognized);
        assert!(!list.truncated);
    }

    #[test]
    fn failing_and_unrecognized_listings_are_recoverable_text() {
        let _guard = test_guard();
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
        let _guard = test_guard();
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
        let _guard = test_guard();
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
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[{"name":"planner"}]}"#);
        let receiver = spawn_agent_scan_using(
            7,
            &dir.path().to_string_lossy(),
            LIST_TIMEOUT,
            program.into_os_string(),
            ProfileScope::default(),
        );
        let scan = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(scan.token, 7);
        assert_eq!(scan.cwd, dir.path().to_string_lossy());
        assert_eq!(scan.result.unwrap().names, ["planner"]);
    }

    #[test]
    fn cancelling_scan_reaps_the_listing_child_promptly() {
        let _guard = test_guard();
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
            ProfileScope::default(),
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
        let _guard = test_guard();
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
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[]}"#);
        let list = capture_agent_list(dir.path(), program, LIST_TIMEOUT).unwrap();
        assert!(list.names.is_empty());
        assert!(!list.unrecognized);
    }

    fn write_profile(dir: &Path, file: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(file), body).unwrap();
    }

    fn failing_cli(dir: &Path) -> PathBuf {
        let program = dir.join("kiro-cli");
        fs::write(&program, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[test]
    fn failed_listing_falls_back_to_genuine_scoped_profiles() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = failing_cli(dir.path());
        let workspace = dir.path().join(".kiro/agents");
        let global = dir.path().join("home/.kiro/agents");
        write_profile(&workspace, "reviewer.json", r#"{"name":"reviewer"}"#);
        write_profile(&global, "planner.json", r#"{"name":"planner"}"#);
        write_profile(
            &global,
            "kirocrew-guest.json",
            r#"{"name":"kirocrew-guest"}"#,
        );
        let scope = ProfileScope {
            workspace: Some(workspace),
            global: Some(global),
        };
        let list = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap();
        assert_eq!(list.names, ["reviewer", "kirocrew-guest", "planner"]);
        assert!(!list.truncated && list.unrecognized);
    }

    #[test]
    fn scoped_profiles_supplement_a_successful_listing() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = fake_kiro(dir.path(), r#"{"agents":[{"name":"cli-agent"}]}"#);
        let workspace = dir.path().join(".kiro/agents");
        let global = dir.path().join("global-agents");
        write_profile(&workspace, "reviewer.json", r#"{"name":"reviewer"}"#);
        // The same profile installed globally must not create a second row.
        write_profile(&global, "cli-agent.json", r#"{"name":"cli-agent"}"#);
        let scope = ProfileScope {
            workspace: Some(workspace),
            global: Some(global),
        };
        let list = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap();
        assert_eq!(list.names, ["cli-agent", "reviewer"]);
    }

    #[test]
    fn agent_config_dir_override_replaces_the_workspace_scope() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = failing_cli(dir.path());
        write_profile(
            &dir.path().join(".kiro/agents"),
            "workspace-only.json",
            r#"{"name":"workspace-only"}"#,
        );
        let override_dir = dir.path().join("override-agents");
        write_profile(&override_dir, "aws-expert.json", r#"{"name":"aws-expert"}"#);
        let scope = ProfileScope {
            workspace: None,
            global: Some(override_dir),
        };
        let list = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap();
        assert_eq!(list.names, ["aws-expert"]);
    }

    #[test]
    fn markdown_profiles_use_declared_frontmatter_names() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("review-persona.md"),
            "---\nname: \"code-reviewer\"\ndescription: reads diffs\n---\nbody\n",
        )
        .unwrap();
        fs::write(agents.join("plain.md"), "just a prompt, no frontmatter\n").unwrap();
        assert_eq!(profile_names_in(&agents), ["plain", "code-reviewer"]);
    }

    #[test]
    fn declared_json_name_wins_and_unreadable_or_hostile_files_are_skipped() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        // Declared name overrides the file stem.
        write_profile(&agents, "stem-name.json", r#"{"name":"declared-name"}"#);
        // Missing name falls back to the genuine file stem.
        write_profile(&agents, "stem-only.json", r#"{"model":"auto"}"#);
        // Invalid and oversized JSON are skipped; incomplete bytes are not profiles.
        write_profile(&agents, "broken.json", "not json at all");
        let huge = format!(
            "{{\"name\":\"deep\",\"prompt\":\"{}\"}}",
            "x".repeat(300 * 1024)
        );
        write_profile(&agents, "huge.json", &huge);
        // Hidden/other files are skipped. Nested names and file symlinks are supported.
        fs::write(agents.join(".profile.lock"), "").unwrap();
        fs::write(agents.join(".hidden.json"), r#"{"name":"hidden"}"#).unwrap();
        fs::write(agents.join("notes.txt"), r#"{"name":"txt"}"#).unwrap();
        let nested = agents.join("nested");
        write_profile(&nested, "nested.json", r#"{"name":"nested"}"#);
        let outside = tempfile::tempdir().unwrap();
        write_profile(outside.path(), "linked.json", r#"{"name":"linked"}"#);
        std::os::unix::fs::symlink(outside.path().join("linked.json"), agents.join("link.json"))
            .unwrap();
        // An unreadable scope contributes nothing instead of erroring.
        let unreadable = dir.path().join("locked-agents");
        write_profile(&unreadable, "secret.json", r#"{"name":"secret"}"#);
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        let names = profile_names_in(&agents);
        assert!(profile_names_in(&unreadable).is_empty());
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            names,
            ["linked", "declared-name", "stem-only", "nested/nested"]
        );
        assert!(profile_names_in(&unreadable).contains(&"secret".to_owned()));
        assert!(profile_names_in(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn unsafe_profile_names_are_rejected_not_offered() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        write_profile(&agents, "-flag.json", r#"{"name":"-flag"}"#);
        fs::write(agents.join("weird.json"), "{\"name\":\"a/b\"}").unwrap();
        fs::write(agents.join("spacey.json"), "{\"name\":\"   \"}").unwrap();
        // A declared name containing '/' or blank falls back to the genuine
        // file stem; an unsafe stem (leading '-') yields nothing at all.
        assert_eq!(profile_names_in(&agents), ["spacey", "weird"]);
    }

    #[test]
    fn listing_failure_without_profiles_still_reports_the_failure() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let program = failing_cli(dir.path());
        let scope = ProfileScope {
            workspace: Some(dir.path().join(".kiro/agents")),
            global: Some(dir.path().join("home/.kiro/agents")),
        };
        let error = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap_err();
        assert!(matches!(error, ListError::Failed { .. }), "{error}");
    }

    #[test]
    fn cancellation_and_oversized_output_never_take_the_fallback_path() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let scope = ProfileScope {
            workspace: Some(dir.path().join(".kiro/agents")),
            global: None,
        };
        write_profile(
            &dir.path().join(".kiro/agents"),
            "reviewer.json",
            r#"{"name":"reviewer"}"#,
        );
        let program = dir.path().join("kiro-cli");
        fs::write(&program, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let error = discover_agents_in(
            dir.path(),
            &program,
            Duration::from_millis(200),
            &AtomicBool::new(true),
            &scope,
        )
        .unwrap_err();
        assert!(matches!(error, ListError::Cancelled), "{error}");

        let program = dir.path().join("kiro-cli-2");
        fs::write(
            &program,
            "#!/bin/sh\nyes '{\"agents\":[{\"name\":\"x\"}],' | head -c 400000\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let error = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap_err();
        assert!(matches!(error, ListError::TooLarge), "{error}");
    }

    #[test]
    fn scan_reports_truncation_when_the_file_cap_is_hit() {
        let _guard = test_guard();
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        for index in 0..=MAX_PROFILE_FILES {
            fs::write(agents.join(format!("profile-{index}.json")), "{}").unwrap();
        }
        let scope = ProfileScope {
            workspace: None,
            global: Some(agents),
        };
        let program = failing_cli(dir.path());
        let list = discover_agents_in(
            dir.path(),
            &program,
            LIST_TIMEOUT,
            &AtomicBool::new(false),
            &scope,
        )
        .unwrap();
        assert_eq!(list.names.len(), MAX_AGENTS);
        assert!(list.truncated);
    }

    #[test]
    fn directory_scan_caps_examined_entries_even_when_none_are_directories() {
        let _guard = test_guard();
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
        let _guard = test_guard();
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
