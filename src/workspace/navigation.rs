use super::*;
use std::{
    os::unix::fs::MetadataExt,
    thread,
    time::{Duration, Instant},
};

const SESSION: &str = "@kiro-agents-session";
const FRONTEND: &str = "@kiro-agents-frontend";
const EMPTY: &str = "@kiro-agents-empty";

struct Attachment {
    workspace: TmuxWorkspace,
    pane: String,
    token: String,
}

impl WorkspaceAttachment for Attachment {}

impl Drop for Attachment {
    fn drop(&mut self) {
        if self
            .workspace
            .pane_value(&self.pane, FRONTEND)
            .ok()
            .as_deref()
            == Some(&self.token)
        {
            let _ = self.workspace.output(&[
                "set-option",
                "-pu",
                "-t",
                &self.pane,
                "@kiro-agents-title",
            ]);
            let _ = self
                .workspace
                .output(&["set-option", "-pu", "-t", &self.pane, SESSION]);
            let _ = self
                .workspace
                .output(&["set-option", "-pu", "-t", &self.pane, FRONTEND]);
        }
    }
}

impl TmuxWorkspace {
    pub(super) fn app_command(&self, args: &[&str], extra: &[(&str, &str)]) -> Result<String> {
        let state = self
            .state_dir
            .to_str()
            .context("State path must be UTF-8")?;
        let socket = self.socket.to_str().context("Socket path must be UTF-8")?;
        let executable = self
            .executable
            .to_str()
            .context("Executable path must be UTF-8")?;
        let mut command = format!(
            "exec env KIRO_AGENTS_HOME={} KIRO_AGENTS_WORKSPACE_SOCKET={}",
            shell_quote(state),
            shell_quote(socket)
        );
        for (key, value) in extra {
            command.push_str(&format!(" {key}={}", shell_quote(value)));
        }
        command.push(' ');
        command.push_str(&shell_quote(executable));
        for arg in args {
            command.push(' ');
            command.push_str(&shell_quote(arg));
        }
        Ok(command)
    }

    pub(super) fn navigation_launcher(&self, tail: &[&str]) -> Result<String> {
        // run-shell expands formats once. Escape literal path hashes first and
        // use tmux's shell-quoting format only for the invoking client context.
        let command = self.app_command(tail, &[])?.replace('#', "##");
        Ok(format!(
            "KIRO_AGENTS_WORKSPACE_CLIENT=#{{q:client_name}} TMUX_PANE=#{{pane_id}} {command}"
        ))
    }

    fn pane_value(&self, pane: &str, option: &str) -> Result<String> {
        let format = format!("#{{{option}}}");
        let output = self.output(&["display-message", "-p", "-t", pane, &format])?;
        anyhow::ensure!(output.status.success(), "Workspace pane no longer exists");
        Ok(String::from_utf8(output.stdout)?
            .trim_end_matches('\n')
            .to_owned())
    }

    fn client_for(&self, pane: &str) -> Result<String> {
        let output = self.output(&["list-clients", "-F", "#{client_name}\t#{pane_id}"])?;
        anyhow::ensure!(output.status.success(), "No attached workspace client");
        let text = String::from_utf8(output.stdout)?;
        let explicit = env::var("KIRO_AGENTS_WORKSPACE_CLIENT").ok();
        let candidates: Vec<_> = text
            .lines()
            .filter_map(|line| {
                let (client, active) = line.split_once('\t')?;
                if explicit
                    .as_deref()
                    .map_or(active == pane, |wanted| wanted == client)
                {
                    Some(client.to_owned())
                } else {
                    None
                }
            })
            .collect();
        anyhow::ensure!(
            candidates.len() == 1,
            "Open the dashboard with Ctrl+Q in the desired workspace client"
        );
        Ok(candidates[0].clone())
    }

    pub(super) fn show_dashboard(&self, placement: Option<PanePlacement>) -> Result<()> {
        self.ensure_runtime()?;
        let (pane, _) = self.owned_pane()?;
        let client = self.client_for(&pane)?;
        let placement = match placement {
            Some(PanePlacement::Right) => "right",
            Some(PanePlacement::Below) => "below",
            Some(PanePlacement::Window) => "window",
            None => "",
        };
        let command = self.app_command(
            &["__workspace-board"],
            &[
                ("TMUX_PANE", &pane),
                ("KIRO_AGENTS_WORKSPACE_CLIENT", &client),
                ("KIRO_AGENTS_WORKSPACE_POPUP", "1"),
                ("KIRO_AGENTS_WORKSPACE_PLACEMENT", placement),
            ],
        )?;
        self.run(
            &[
                "display-popup",
                "-E",
                "-c",
                &client,
                "-t",
                &pane,
                "-x",
                "C",
                "-y",
                "C",
                "-w",
                "85%",
                "-h",
                "80%",
                "-d",
                CURRENT_PATH,
                &command,
            ],
            "Could not open the session dashboard",
        )
    }

    pub(super) fn mark_empty_home(&self, pane: &str) -> Result<()> {
        anyhow::ensure!(is_pane_id(pane), "Invalid launcher pane");
        let pid: u32 = self.pane_value(pane, "pane_pid")?.parse()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let actual = fs::metadata(format!("/proc/{pid}/exe"));
            let expected = fs::metadata(&self.executable);
            if matches!((actual, expected), (Ok(actual), Ok(expected)) if actual.dev() == expected.dev() && actual.ino() == expected.ino())
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let token = process_token(pid).context("Workspace launcher ended during startup")?;
        self.run(
            &["set-option", "-p", "-t", pane, EMPTY, &token],
            "Could not register empty workspace launcher",
        )
    }

    pub(super) fn track_attachment(
        &self,
        id: &str,
    ) -> Result<Option<Box<dyn WorkspaceAttachment>>> {
        validate_session(id)?;
        let (pane, _) = self.owned_pane()?;
        let pane_pid: u32 = self.pane_value(&pane, "pane_pid")?.parse()?;
        let pid = std::process::id();
        let token = process_token(pid).context("Frontend process disappeared")?;
        // Shell-launched frontends are allowed only while they are descendants
        // of this pane and own its foreground process group.
        if !foreground_frontend(&token, pane_pid) {
            return Ok(None);
        }
        self.run(
            &["set-option", "-p", "-t", &pane, FRONTEND, &token],
            "Could not register frontend ownership",
        )?;
        if let Err(error) = self.run(
            &["set-option", "-p", "-t", &pane, SESSION, id],
            "Could not register the session pane",
        ) {
            let _ = self.output(&["set-option", "-pu", "-t", &pane, FRONTEND]);
            return Err(error);
        }
        if let Ok(crate::protocol::Reply::Sessions(mut rows)) =
            crate::host::call(&self.state_dir, crate::protocol::Action::Status, false)
        {
            let _ = crate::custom_names::enrich(&self.state_dir, &mut rows);
            if let Some(row) = rows.iter().find(|row| row.id == id) {
                let title = row
                    .display_name()
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(100)
                    .collect::<String>();
                self.run(
                    &[
                        "set-option",
                        "-p",
                        "-t",
                        &pane,
                        "@kiro-agents-title",
                        &title,
                    ],
                    "Could not label session pane",
                )?;
            }
        }
        let _ = self.output(&["set-option", "-pu", "-t", &pane, EMPTY]);
        Ok(Some(Box::new(Attachment {
            workspace: self.clone(),
            pane,
            token,
        })))
    }

    pub(super) fn navigate_existing(&self, id: &str) -> Result<bool> {
        self.navigate_existing_zoom(id, false)
    }

    pub(super) fn navigate_existing_zoom(&self, id: &str, zoom: bool) -> Result<bool> {
        validate_session(id)?;
        let (origin, _) = self.owned_pane()?;
        let client = self.client_for(&origin)?;
        let output = self.output(&["list-panes", "-a", "-F", "#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{session_name}\t#{window_id}\t#{@kiro-agents-session}\t#{@kiro-agents-frontend}"])?;
        anyhow::ensure!(output.status.success(), "Could not locate session panes");
        let text = String::from_utf8(output.stdout)?;
        let mut matches = Vec::new();
        for line in text.lines().take(1024) {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 7 || fields[5] != id || fields[2] != "0" {
                continue;
            }
            let Ok(pid) = fields[1].parse::<u32>() else {
                continue;
            };
            if !foreground_frontend(fields[6], pid) {
                continue;
            }
            matches.push((
                fields[0].to_owned(),
                fields[3].to_owned(),
                fields[4].to_owned(),
            ));
        }
        anyhow::ensure!(
            matches.len() <= 1,
            "More than one pane claims this session; no attachment was changed"
        );
        let Some((pane, session, window)) = matches.pop() else {
            return Ok(false);
        };
        if self.pane_value(&pane, "pane_in_mode")? == "1" {
            self.run(
                &["send-keys", "-X", "-t", &pane, "cancel"],
                "Could not return session pane to live input",
            )?;
        }
        if self.pane_value(&pane, "window_zoomed_flag")? == "1"
            && self.pane_value(&pane, "pane_active")? != "1"
        {
            self.run(
                &["resize-pane", "-Z", "-t", &pane],
                "Could not reveal session pane",
            )?;
        }
        self.run(
            &["select-window", "-t", &window],
            "Could not select the session window",
        )?;
        self.run(
            &["select-pane", "-t", &pane],
            "Could not focus the session pane",
        )?;
        if zoom && self.pane_value(&pane, "window_zoomed_flag")? != "1" {
            self.run(
                &["resize-pane", "-Z", "-t", &pane],
                "Could not zoom the session pane",
            )?;
        }
        // Switching may close this client's popup: finish pane selection first.
        self.run(
            &["switch-client", "-c", &client, "-t", &target(&session)],
            "Could not switch to the session workspace",
        )?;
        Ok(true)
    }

    pub(super) fn place_session(
        &self,
        id: &str,
        cwd: &Path,
        placement: PanePlacement,
    ) -> Result<()> {
        self.place_session_impl(id, cwd, placement, false)
    }

    pub(super) fn place_session_move(
        &self,
        id: &str,
        cwd: &Path,
        placement: PanePlacement,
    ) -> Result<()> {
        self.place_session_impl(id, cwd, placement, true)
    }

    fn place_session_impl(
        &self,
        id: &str,
        cwd: &Path,
        placement: PanePlacement,
        move_here: bool,
    ) -> Result<()> {
        validate_session(id)?;
        let cwd = directory(cwd)?;
        let _lock = self.setup_lock()?;
        if !move_here && self.navigate_existing(id)? {
            return Ok(());
        }
        let (origin, window) = self.owned_pane()?;
        let origin_pid: u32 = self.pane_value(&origin, "pane_pid")?.parse()?;
        if move_here
            && self.pane_value(&origin, SESSION)? == id
            && foreground_frontend(&self.pane_value(&origin, FRONTEND)?, origin_pid)
        {
            self.navigate_existing(id)?;
            return Ok(());
        }
        let empty = placement != PanePlacement::Window
            && process_token(origin_pid).as_deref()
                == Some(self.pane_value(&origin, EMPTY)?.as_str());
        let home = self.app_command(&["__workspace-home"], &[])?;
        let pane = if empty {
            origin
        } else {
            let mut args = match placement {
                PanePlacement::Right => vec!["split-window", "-h", "-t", origin.as_str()],
                PanePlacement::Below => vec!["split-window", "-v", "-t", origin.as_str()],
                PanePlacement::Window => vec!["new-window", "-a", "-t", window.as_str()],
            };
            let cwd_format = cwd.replace('#', "##");
            args.extend(["-d", "-P", "-F", "#{pane_id}", "-c", &cwd_format, &home]);
            let output = self.output(&args)?;
            anyhow::ensure!(
                output.status.success(),
                "Could not create session pane: {}",
                tmux_message(&output.stderr, &output.stdout)
            );
            let pane = String::from_utf8(output.stdout)?.trim().to_owned();
            anyhow::ensure!(is_pane_id(&pane), "Unexpected session pane identity");
            pane
        };
        self.run(
            &["set-option", "-p", "-t", &pane, "remain-on-exit", "on"],
            "Could not prepare session pane",
        )?;
        let command = if move_here {
            self.app_command(&["__workspace-session", id, "--move"], &[])?
        } else {
            self.app_command(&["__workspace-session", id], &[])?
        };
        if let Err(error) = self.run(
            &[
                "respawn-pane",
                "-k",
                "-t",
                &pane,
                "-c",
                &cwd.replace('#', "##"),
                &command,
            ],
            "Could not launch session frontend",
        ) {
            if !empty {
                let _ = self.output(&["kill-pane", "-t", &pane]);
            }
            return Err(error);
        }
        let pid: u32 = self.pane_value(&pane, "pane_pid")?.parse()?;
        let token = process_token(pid).unwrap_or_else(|| format!("ended:{pid}"));
        let deadline = Instant::now() + Duration::from_secs(if move_here { 15 } else { 6 });
        while Instant::now() < deadline {
            if self.pane_value(&pane, SESSION).ok().as_deref() == Some(id)
                && self.pane_value(&pane, FRONTEND).ok().as_deref() == Some(&token)
            {
                self.run(
                    &["set-option", "-p", "-t", &pane, "remain-on-exit", "off"],
                    "Could not finish session pane startup",
                )?;
                anyhow::ensure!(
                    self.navigate_existing(id)?,
                    "Session pane ended before it could be focused"
                );
                return Ok(());
            }
            if self.pane_value(&pane, "pane_dead").ok().as_deref() == Some("1") {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        // Remove only the frontend attempt this operation created. Kiro remains
        // host-owned; a failed or foreign attachment is never stolen.
        if self.pane_value(&pane, "pane_pid").ok().as_deref() == Some(pid.to_string().as_str())
            && (process_token(pid).as_deref() == Some(&token)
                || self.pane_value(&pane, "pane_dead").ok().as_deref() == Some("1"))
        {
            if empty {
                let _ = self.output(&["respawn-pane", "-k", "-t", &pane, &home]);
                let _ = self.output(&["set-option", "-p", "-t", &pane, "remain-on-exit", "off"]);
                let _ = self.mark_empty_home(&pane);
            } else {
                let _ = self.output(&["kill-pane", "-t", &pane]);
            }
        }
        anyhow::bail!("Could not attach the session pane. If it is open elsewhere, press Esc then m to move it here. Older source frontends must be detached and reopened once with the updated binary; Kiro stays running and no host restart is needed.")
    }
}

fn validate_session(id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "Invalid session identity"
    );
    Ok(())
}

fn process_token(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let started = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?;
    let image = fs::metadata(format!("/proc/{pid}/exe")).ok()?;
    Some(format!("{pid}:{started}:{}:{}", image.dev(), image.ino()))
}

fn foreground_frontend(token: &str, pane_pid: u32) -> bool {
    let Some(pid) = token
        .split(':')
        .next()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return false;
    };
    if process_token(pid).as_deref() != Some(token) {
        return false;
    }
    let mut current = pid;
    for _ in 0..12 {
        let Ok(stat) = fs::read_to_string(format!("/proc/{current}/stat")) else {
            return false;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            return false;
        };
        let fields: Vec<_> = fields.split_whitespace().collect();
        if current == pid && (fields.first() == Some(&"Z") || fields.get(2) != fields.get(5)) {
            return false;
        }
        if current == pane_pid {
            return process_token(pid).as_deref() == Some(token);
        }
        let Some(parent) = fields.get(1).and_then(|pid| pid.parse::<u32>().ok()) else {
            return false;
        };
        if parent == 0 || parent == current {
            return false;
        }
        current = parent;
    }
    false
}

pub(super) fn tmux_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
    )
}
