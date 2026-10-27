use crate::features::{SplitDirection, WorkspaceFeature};
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::IsTerminal,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
const MAX_NAME: usize = 64; // tmux rewrites dots and reads punctuation as target syntax.
const MAX_SOCKET_PATH: usize = 100; // `sockaddr_un` allows 108 bytes including the NUL.
const CURRENT_PATH: &str = "#{pane_current_path}";
pub struct TmuxWorkspace {
    state_dir: PathBuf,
    root: PathBuf,
    socket: PathBuf,
    executable: PathBuf,
    binary: OsString,
}
impl TmuxWorkspace {
    pub fn new(state_dir: PathBuf, executable: PathBuf) -> Result<Self> {
        anyhow::ensure!(
            executable.is_absolute(),
            "Workspace panes need an absolute executable path"
        );
        anyhow::ensure!(
            state_dir.is_absolute(),
            "Workspace state path must be absolute"
        );
        for path in [&state_dir, &executable] {
            check_path(path.to_str().context("Workspace paths must be UTF-8")?)?;
        }
        let root = state_dir.join("workspaces");
        let socket = root.join("tmux.sock");
        let length = socket.as_os_str().as_bytes().len();
        anyhow::ensure!(length <= MAX_SOCKET_PATH, "Workspace socket path is {length} chars, over the tmux limit {MAX_SOCKET_PATH}; set KIRO_AGENTS_HOME shorter");
        let binary = env::var_os("KIRO_AGENTS_TMUX_BIN").unwrap_or_else(|| OsString::from("tmux"));
        Ok(Self {
            state_dir,
            root,
            socket,
            executable,
            binary,
        })
    }
    // Every call is bound to the private socket and loads no user tmux configuration.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("-S")
            .arg(&self.socket)
            .arg("-f")
            .arg("/dev/null")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn output(&self, args: &[&str]) -> Result<std::process::Output> {
        for path in [&self.root, &self.socket] {
            anyhow::ensure!(
                !fs::symlink_metadata(path)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false),
                "Workspace directory and socket must not be symlinks"
            );
        }
        self.command(args).output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow!("tmux is not installed ({})", self.binary.to_string_lossy())
            } else {
                anyhow!("Could not run tmux: {error}")
            }
        })
    }
    fn run(&self, args: &[&str], what: &str) -> Result<()> {
        let output = self.output(args)?;
        if output.status.success() {
            return Ok(());
        }
        Err(anyhow!(
            "{what}: {}",
            tmux_message(&output.stderr, &output.stdout)
        ))
    }
    fn ensure_runtime(&self) -> Result<()> {
        let output = self.output(&["-V"])?;
        anyhow::ensure!(
            output.status.success(),
            "tmux could not report its version: {}",
            tmux_message(&output.stderr, &output.stdout)
        );
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let version = text
            .strip_prefix("tmux ")
            .ok_or_else(|| anyhow!("Unrecognised tmux version {text:?}"))?;
        let mut parts = version.split('.');
        let major = parts.next().unwrap_or_default().parse::<u32>();
        let minor = parts
            .next()
            .unwrap_or_default()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .unwrap_or(0);
        let Ok(major) = major else {
            return Err(anyhow!("tmux version {version:?} is not understood"));
        };
        anyhow::ensure!(
            major > 3 || (major == 3 && minor >= 2),
            "Workspaces need tmux 3.2 or newer (found {version})"
        );
        Ok(())
    }
    fn ensure_root(&self) -> Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        anyhow::ensure!(
            !fs::symlink_metadata(&self.root)?.file_type().is_symlink(),
            "Workspace directory must not be a symlink"
        );
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        anyhow::ensure!(
            !fs::symlink_metadata(&self.socket)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "Workspace socket must not be a symlink"
        );
        Ok(())
    }
    fn setup_lock(&self) -> Result<File> {
        self.ensure_root()?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join("setup.lock"))?;
        file.lock_exclusive()?;
        Ok(file)
    }
    fn session_exists(&self, name: &str) -> Result<bool> {
        Ok(self.workspaces()?.iter().any(|row| row.0 == name))
    }
    fn workspaces(&self) -> Result<Vec<(String, u32, u32)>> {
        let output = self.output(&[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_windows}\t#{session_attached}",
        ])?;
        if !output.status.success() {
            let text = String::from_utf8_lossy(&output.stderr).to_string();
            if unreachable_server(text.as_bytes()) || text.contains("no sessions") {
                return Ok(Vec::new());
            }
            return Err(anyhow!(
                "Could not list workspaces: {}",
                tmux_message(&output.stderr, &output.stdout)
            ));
        }
        let text = String::from_utf8(output.stdout).context("tmux returned non-UTF-8 metadata")?;
        let mut rows = Vec::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let mut fields = line.split('\t');
            let (Some(name), Some(windows), Some(attached)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            rows.push((
                name.to_owned(),
                windows.parse().unwrap_or(0),
                attached.parse().unwrap_or(0),
            ));
        }
        Ok(rows)
    }
    // Pane command lines reach a POSIX shell, so only single quoting is trusted here.
    fn pane_command(&self, terminal: bool) -> Result<String> {
        let state_dir = self
            .state_dir
            .to_str()
            .ok_or_else(|| anyhow!("State path must be UTF-8"))?;
        let socket = self
            .socket
            .to_str()
            .ok_or_else(|| anyhow!("Socket path must be UTF-8"))?;
        let shell = env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/sh"));
        let (program, suffix) = if terminal {
            (
                shell
                    .to_str()
                    .ok_or_else(|| anyhow!("SHELL must be UTF-8"))?
                    .to_owned(),
                "",
            )
        } else {
            (
                self.executable
                    .to_str()
                    .ok_or_else(|| anyhow!("Executable path must be UTF-8"))?
                    .to_owned(),
                " board",
            )
        };
        check_path(&program)?;
        Ok(format!(
            "exec env KIRO_AGENTS_HOME={} KIRO_AGENTS_WORKSPACE_SOCKET={} {}{suffix}",
            shell_quote(state_dir),
            shell_quote(socket),
            shell_quote(&program)
        ))
    }
    fn in_own_server(&self) -> bool {
        let Some(value) = env::var_os("TMUX") else {
            return false;
        };
        socket_from_tmux(&value)
            .map(|path| same_socket(&path, &self.socket))
            .unwrap_or(false)
    }
    // Only panes this feature started inside our own server may be split or extended.
    fn owned_pane(&self) -> Result<(String, String)> {
        anyhow::ensure!(
            self.in_own_server(),
            "Open a kiro-agents workspace first, then split from one of its panes"
        );
        anyhow::ensure!(
            env::var_os("KIRO_AGENTS_WORKSPACE_SOCKET").as_deref() == Some(self.socket.as_os_str()),
            "This pane was not started by kiro-agents; its panes stay untouched"
        );
        let raw = env::var_os("TMUX_PANE")
            .ok_or_else(|| anyhow!("TMUX_PANE is not set; open a kiro-agents pane first"))?;
        let raw = raw.to_str().context("TMUX_PANE must be UTF-8")?;
        anyhow::ensure!(is_pane_id(raw), "TMUX_PANE {raw:?} is not a pane ID");
        let output = self.output(&[
            "display-message",
            "-p",
            "-t",
            raw,
            "#{pane_id}\t#{window_id}\t#{session_name}",
        ])?;
        anyhow::ensure!(
            output.status.success(),
            "Pane {raw} no longer exists in this workspace server"
        );
        let text = String::from_utf8(output.stdout).context("tmux returned non-UTF-8 metadata")?;
        let mut fields = text.trim_end_matches('\n').split('\t');
        let (Some(id), Some(window), Some(session)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(anyhow!("Could not identify the current workspace pane"));
        };
        anyhow::ensure!(id == raw, "TMUX_PANE does not identify an existing pane");
        anyhow::ensure!(
            self.workspaces()?.iter().any(|row| row.0 == session),
            "The current pane belongs to an unmanaged tmux session; refusing to modify it"
        );
        Ok((id.to_owned(), window.to_owned()))
    }
    // Pass tmux argv directly. Quoting belongs only to the shell command inside a pane.
    // Focus, resize, zoom, copy mode and detach retain the native default bindings.
    fn configure_server(&self) -> Result<()> {
        let board = self.pane_command(false)?;
        let shell = self.pane_command(true)?;
        // Primary-screen applications rely on terminal scrollback, not injected
        // editing keys. Tmux owns that buffer inside a workspace. Only forward
        // mouse events when the application actually requested them.
        self.run(
            &["set-option", "-g", "mouse", "on"],
            "Could not enable workspace mouse input",
        )?;
        let page_up = r##"if-shell -F -t = "#{pane_in_mode}" "send-keys -X -t = page-up" "copy-mode -e -u -t =""##;
        let page_down = r##"if-shell -F -t = "#{pane_in_mode}" "send-keys -X -t = page-down""##;
        for (wheel, scroll) in [("WheelUpPane", page_up), ("WheelDownPane", page_down)] {
            self.run(
                &[
                    "bind-key",
                    "-T",
                    "root",
                    wheel,
                    "if-shell",
                    "-F",
                    "-t",
                    "=",
                    "#{&&:#{mouse_any_flag},#{!:#{pane_in_mode}}}",
                    "send-keys -M -t =",
                    scroll,
                ],
                "Could not set workspace wheel paging",
            )?;
        }
        self.run(
            &[
                "bind-key",
                "-T",
                "root",
                "PPage",
                "if-shell",
                "-F",
                "#{alternate_on}",
                "send-keys PPage",
                "copy-mode -e -u",
            ],
            "Could not set workspace PageUp scrollback",
        )?;
        self.run(
            &[
                "bind-key",
                "-T",
                "root",
                "NPage",
                "if-shell",
                "-F",
                "#{alternate_on}",
                "send-keys NPage",
            ],
            "Could not set workspace PageDown handling",
        )?;
        for (key, command, direction, program) in [
            ("%", "split-window", "-h", &board),
            ("\"", "split-window", "-v", &board),
            ("c", "new-window", "-a", &board),
            ("T", "split-window", "-h", &shell),
            ("C", "new-window", "-a", &shell),
        ] {
            self.run(
                &[
                    "bind-key",
                    "-T",
                    "prefix",
                    key,
                    command,
                    direction,
                    "-c",
                    CURRENT_PATH,
                    program,
                ],
                "Could not set the workspace key binding",
            )?;
        }
        Ok(())
    }
    fn start_session(&self, name: &str, cwd: &str) -> Result<()> {
        let command = self.pane_command(false)?;
        let output = self.output(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            &cwd.replace('#', "##"),
            &command,
        ])?;
        if output.status.success() {
            return Ok(());
        }
        Err(anyhow!(
            "Could not open workspace {name}: {}",
            tmux_message(&output.stderr, &output.stdout)
        ))
    }
}
impl WorkspaceFeature for TmuxWorkspace {
    fn open(&self, name: &str, cwd: &Path, detached: bool) -> Result<()> {
        let name = workspace_name(name)?;
        let cwd = directory(cwd)?;
        self.ensure_runtime()?;
        anyhow::ensure!(
            detached || env::var_os("TMUX").is_none() || self.in_own_server(),
            "Already inside another tmux server; use `workspace open {name} --detach`"
        );
        anyhow::ensure!(
            detached
                || self.in_own_server()
                || (std::io::stdin().is_terminal() && std::io::stdout().is_terminal()),
            "No terminal to attach to; use `--detach`"
        );
        let lock = self.setup_lock()?;
        let existed = self.session_exists(&name)?;
        if !existed {
            self.start_session(&name, &cwd)?;
        }
        if let Err(error) = self.configure_server() {
            if !existed {
                let _ = self.output(&["kill-session", "-t", &target(&name)]);
            }
            return Err(error);
        }
        drop(lock);
        if detached {
            return Ok(());
        }
        if self.in_own_server() {
            return self.run(
                &["switch-client", "-t", &target(&name)],
                "Could not switch to the workspace",
            );
        }
        anyhow::ensure!(
            std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            "No terminal to attach to; use `--detach`"
        );
        let status = self
            .command(&["attach", "-t", &target(&name)])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|error| anyhow!("Could not start tmux: {error}"))?;
        anyhow::ensure!(status.success(), "Workspace {name} stays open; use `workspace list` or `workspace close {name}` (tmux {status})");
        Ok(())
    }
    fn split(&self, direction: SplitDirection, terminal: bool, cwd: &Path) -> Result<()> {
        let cwd = directory(cwd)?;
        self.ensure_runtime()?;
        let (pane, _) = self.owned_pane()?;
        let side = match direction {
            SplitDirection::Horizontal => "-h",
            SplitDirection::Vertical => "-v",
        };
        let command = self.pane_command(terminal)?;
        self.run(
            &[
                "split-window",
                side,
                "-t",
                &pane,
                "-c",
                &cwd.replace('#', "##"),
                &command,
            ],
            if terminal {
                "Could not add a shell pane"
            } else {
                "Could not add a session pane"
            },
        )
    }
    fn window(&self, terminal: bool, cwd: &Path) -> Result<()> {
        let cwd = directory(cwd)?;
        self.ensure_runtime()?;
        let (_, window) = self.owned_pane()?;
        let command = self.pane_command(terminal)?;
        self.run(
            &[
                "new-window",
                "-a",
                "-t",
                &window,
                "-c",
                &cwd.replace('#', "##"),
                &command,
            ],
            if terminal {
                "Could not add a shell window"
            } else {
                "Could not add a session window"
            },
        )
    }
    fn list(&self) -> Result<String> {
        self.ensure_runtime()?;
        let mut out = String::new();
        for (name, windows, attached) in self.workspaces()? {
            out.push_str(&format!("{name}\twindows={windows}\tattached={attached}\n"));
        }
        Ok(out)
    }
    fn close(&self, name: &str) -> Result<()> {
        let name = workspace_name(name)?;
        self.ensure_runtime()?;
        let _lock = self.setup_lock()?;
        anyhow::ensure!(
            self.session_exists(&name)?,
            "Workspace {name} is not open; run `workspace list`"
        );
        // Shell panes die with the named workspace only; Kiro children live in the host.
        self.run(
            &["kill-session", "-t", &target(&name)],
            "Could not close the workspace",
        )
    }
}
fn target(name: &str) -> String {
    format!("={name}")
}
fn unreachable_server(stderr: &[u8]) -> bool {
    let t = String::from_utf8_lossy(stderr);
    t.contains("no server running") || t.contains("error connecting")
}
fn tmux_message(stderr: &[u8], stdout: &[u8]) -> String {
    let t = String::from_utf8_lossy(if stderr.is_empty() { stdout } else { stderr })
        .trim()
        .to_owned();
    if t.is_empty() {
        "tmux reported a failure".to_owned()
    } else {
        t
    }
}
fn check_path(v: &str) -> Result<()> {
    anyhow::ensure!(
        !v.chars().any(char::is_control),
        "Path has control characters and cannot be a tmux pane"
    );
    Ok(())
}
fn is_pane_id(v: &str) -> bool {
    let Some(d) = v.strip_prefix('%') else {
        return false;
    };
    !d.is_empty() && d.len() <= 12 && d.bytes().all(|b| b.is_ascii_digit())
}
fn same_socket(l: &Path, r: &Path) -> bool {
    l == r || matches!((fs::canonicalize(l), fs::canonicalize(r)), (Ok(l), Ok(r)) if l == r)
}
// Shell single quoting: no expansion, embedded quotes are closed and re-opened.
fn shell_quote(value: &str) -> String {
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
// The marker is `<socket>,<pid>,<session id>`; only the socket path is taken.
fn socket_from_tmux(value: &OsStr) -> Option<PathBuf> {
    let text = value.to_str()?;
    let (rest, session) = text.rsplit_once(',')?;
    let (path, pid) = rest.rsplit_once(',')?;
    (!path.is_empty()
        && !pid.is_empty()
        && !session.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && session.bytes().all(|b| b.is_ascii_digit()))
    .then(|| PathBuf::from(path))
}
fn workspace_name(name: &str) -> Result<String> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "Workspace name cannot be empty");
    anyhow::ensure!(
        name.chars().count() <= MAX_NAME,
        "Workspace name must be at most {MAX_NAME} characters"
    );
    anyhow::ensure!(
        name.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')),
        "Workspace names may only use letters, digits, '-' and '_'"
    );
    anyhow::ensure!(
        name.as_bytes()[0].is_ascii_alphanumeric(),
        "Workspace names must start with a letter or digit"
    );
    Ok(name.to_owned())
}
fn directory(path: &Path) -> Result<String> {
    let path = fs::canonicalize(path).map_err(|error| {
        anyhow!(
            "Workspace directory {} does not exist: {error}",
            path.display()
        )
    })?;
    anyhow::ensure!(path.is_dir(), "Workspace path is not a directory");
    let text = path
        .to_str()
        .ok_or_else(|| anyhow!("Workspace path must be UTF-8"))?;
    check_path(text)?;
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_be_tmux_target_expressions_or_options() {
        for name in [
            "", " ", ".hidden", "-flag", "a.b", "a:b", "a,b", "%1", "a\nb", "a/b", "a;b",
        ] {
            assert!(workspace_name(name).is_err(), "{name:?}");
        }
        assert!(workspace_name(&"a".repeat(65)).is_err());
        assert_eq!(workspace_name(" project-2_dev ").unwrap(), "project-2_dev");
    }

    #[test]
    fn pane_context_is_strict_and_allows_commas_in_socket_paths() {
        for pane in ["", "%", "%1:2", "-t", "1", "%a", "%1234567890123"] {
            assert!(!is_pane_id(pane));
        }
        assert!(is_pane_id("%123"));
        assert_eq!(
            socket_from_tmux(OsStr::new("/tmp/a,b,123,0")),
            Some(PathBuf::from("/tmp/a,b"))
        );
        for marker in ["", "/tmp/sock", "/tmp/sock,x,0", "/tmp/sock,1,$0"] {
            assert!(socket_from_tmux(OsStr::new(marker)).is_none());
        }
    }

    #[test]
    fn shell_command_keeps_metacharacters_literal() {
        let value = "a'b $HOME `id` $(id) ; #{pane_id} \\\"";
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", shell_quote(value)))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
    }

    #[test]
    fn construction_rejects_invalid_paths_without_creating_state() {
        let dir = tempfile::tempdir().unwrap();
        assert!(TmuxWorkspace::new(PathBuf::from("relative"), PathBuf::from("/bin/app")).is_err());
        assert!(TmuxWorkspace::new(dir.path().to_owned(), PathBuf::from("relative")).is_err());
        assert!(
            TmuxWorkspace::new(dir.path().join("a".repeat(100)), PathBuf::from("/bin/app"))
                .is_err()
        );
        assert!(
            TmuxWorkspace::new(dir.path().join("line\nbreak"), PathBuf::from("/bin/app")).is_err()
        );
        assert!(!dir.path().join("workspaces").exists());
    }
}
