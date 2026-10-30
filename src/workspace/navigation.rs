use super::*;
use crate::features::PaneStep;
use std::{
    collections::HashSet,
    os::unix::fs::MetadataExt,
    thread,
    time::{Duration, Instant},
};

const SESSION: &str = "@kiro-agents-session";
const FRONTEND: &str = "@kiro-agents-frontend";
const EMPTY: &str = "@kiro-agents-empty";

/// One list-panes row: pane ID, pane PID, dead flag, workspace name, window
/// index, pane index, window ID and the three wrapper pane markers.
const PANE_FIELDS: usize = 10;
const PANE_FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{session_name}\t#{window_index}\t#{pane_index}\t#{window_id}\t#{@kiro-agents-session}\t#{@kiro-agents-frontend}\t#{@kiro-agents-empty}";

/// A pane in the private workspace server, as reported by one list-panes row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneRow {
    pane: String,
    pid: u32,
    dead: bool,
    workspace: String,
    window_index: u32,
    pane_index: u32,
    window: String,
    session: String,
    frontend: String,
    empty: String,
}

impl PaneRow {
    fn parse(line: &str) -> Option<Self> {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != PANE_FIELDS {
            return None;
        }
        let dead = match fields[2] {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let row = Self {
            pane: fields[0].to_owned(),
            pid: fields[1].parse().ok()?,
            dead,
            workspace: fields[3].to_owned(),
            window_index: fields[4].parse().ok()?,
            pane_index: fields[5].parse().ok()?,
            window: fields[6].to_owned(),
            session: fields[7].to_owned(),
            frontend: fields[8].to_owned(),
            empty: fields[9].to_owned(),
        };
        (is_pane_id(&row.pane) && row.window.starts_with('@')).then_some(row)
    }

    /// Numeric pane identity keeps the order stable when tmux renumbers panes.
    fn pane_number(&self) -> u32 {
        self.pane
            .strip_prefix('%')
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(u32::MAX)
    }

    fn order(&self) -> (&str, u32, u32, u32) {
        (
            self.workspace.as_str(),
            self.window_index,
            self.pane_index,
            self.pane_number(),
        )
    }
}

/// Pick the next or previous pane in a deterministically ordered cycle. The
/// origin does not have to be a session pane: a shell or empty launcher steps
/// into the near end of the list. None means the layout stays untouched.
fn step_target(panes: &[PaneRow], origin: &str, step: PaneStep) -> Option<usize> {
    if panes.is_empty() {
        return None;
    }
    let index = match (panes.iter().position(|pane| pane.pane == origin), step) {
        (Some(at), PaneStep::Next) => (at + 1) % panes.len(),
        (Some(at), PaneStep::Previous) => (at + panes.len() - 1) % panes.len(),
        (None, PaneStep::Next) => 0,
        (None, PaneStep::Previous) => panes.len() - 1,
    };
    (panes[index].pane != origin).then_some(index)
}

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
            for option in ["@kiro-agents-title", "@kiro-agents-cycle-zoom"] {
                let _ = self
                    .workspace
                    .output(&["set-option", "-pu", "-t", &self.pane, option]);
            }
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
        let pane_pid: u32 = self.pane_value(&pane, "pane_pid")?.parse()?;
        let origin_session = if foreground_frontend(&self.pane_value(&pane, FRONTEND)?, pane_pid) {
            self.pane_value(&pane, SESSION)?
        } else {
            String::new()
        };
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
                ("KIRO_AGENTS_WORKSPACE_ORIGIN_SESSION", &origin_session),
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
        let (origin, _) = self.owned_pane()?;
        let client = self.client_for(&origin)?;
        // The dashboard keeps a destination window's own layout unless it asks
        // for fullscreen, so an ordinary Enter never unzooms an unrelated pane.
        self.focus_existing(id, &client, zoom.then_some(true))
    }

    /// Focus the pane that already hosts `id` for one attached client. Some(true)
    /// forces fullscreen, Some(false) forces the split layout and None leaves the
    /// destination window's presentation alone. False means no pane claims it.
    pub(super) fn focus_existing(
        &self,
        id: &str,
        client: &str,
        zoom: Option<bool>,
    ) -> Result<bool> {
        validate_session(id)?;
        let output = self.output(&["list-panes", "-a", "-F", PANE_FORMAT])?;
        anyhow::ensure!(output.status.success(), "Could not locate session panes");
        let text = String::from_utf8(output.stdout)?;
        let mut matches = Vec::new();
        for line in text.lines().take(1024) {
            let Some(row) = PaneRow::parse(line) else {
                continue;
            };
            if row.session != id || row.dead {
                continue;
            }
            if !foreground_frontend(&row.frontend, row.pid) {
                continue;
            }
            matches.push((row.pane, row.workspace, row.window));
        }
        anyhow::ensure!(
            matches.len() <= 1,
            "More than one pane claims this session; no attachment was changed"
        );
        let Some((pane, session, window)) = matches.pop() else {
            return Ok(false);
        };
        self.focus_pane(&pane, &window, &session, client, zoom)?;
        Ok(true)
    }

    /// One Ctrl+J/Ctrl+K step across the live session panes this frontend owns.
    /// Nothing is created, moved or stopped; a pane that vanished mid-scan just
    /// means no focus change happened.
    pub(super) fn step_live_session(&self, step: PaneStep) -> Result<bool> {
        self.ensure_runtime()?;
        let (client, origin) = self.binding_pane()?;
        let managed: HashSet<String> = self.workspaces()?.into_iter().map(|row| row.0).collect();
        anyhow::ensure!(
            managed.contains(&self.pane_value(&origin, "session_name")?),
            "The current pane belongs to an unmanaged tmux session; refusing to modify it"
        );
        let crate::protocol::Reply::Sessions(rows) =
            crate::host::call(&self.state_dir, crate::protocol::Action::Status, false)?
        else {
            anyhow::bail!("Could not verify live sessions");
        };
        let live: HashSet<_> = rows
            .into_iter()
            .filter(|row| row.state == "live")
            .map(|row| row.id)
            .collect();
        let output = self.output(&["list-panes", "-a", "-F", PANE_FORMAT])?;
        anyhow::ensure!(
            output.status.success(),
            "Could not list workspace session panes"
        );
        let text = String::from_utf8(output.stdout)?;
        let mut panes = Vec::new();
        for line in text.lines().take(1024) {
            let Some(row) = PaneRow::parse(line) else {
                continue;
            };
            // Shells, empty launchers, popups and stale or foreign frontends are
            // never focus targets, so they never appear in the cycle.
            let owns_frontend = foreground_frontend(&row.frontend, row.pid);
            let launcher = !row.empty.is_empty()
                && process_token(row.pid).as_deref() == Some(row.empty.as_str());
            if !live.contains(&row.session)
                || !cycle_candidate(&row, &managed, owns_frontend, launcher)
            {
                continue;
            }
            panes.push(row);
        }
        if !panes.iter().any(|row| row.pane == origin) {
            return Ok(false);
        }
        panes.sort_by(|left, right| left.order().cmp(&right.order()));
        let Some(index) = step_target(&panes, &origin, step) else {
            return Ok(false);
        };
        let target = &panes[index];
        let fullscreen = self.pane_value(&origin, "window_zoomed_flag")? == "1"
            || (self.pane_value(&origin, "window_panes")? == "1"
                && self.pane_value(&origin, "@kiro-agents-cycle-zoom")? == "1");
        match self.focus_pane(
            &target.pane,
            &target.window,
            &target.workspace,
            &client,
            Some(fullscreen),
        ) {
            Ok(()) => Ok(true),
            // A session that closed, or lost its frontend between the scan and
            // the focus, must not turn a key press into a failure: the next
            // press sees the new layout.
            Err(_) if !self.pane_exists(&target.pane) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn pane_exists(&self, pane: &str) -> bool {
        self.output(&["display-message", "-p", "-t", pane, "#{pane_id}"])
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn focus_pane(
        &self,
        pane: &str,
        window: &str,
        session: &str,
        client: &str,
        zoom: Option<bool>,
    ) -> Result<()> {
        if self.pane_value(pane, "pane_in_mode")? == "1" {
            self.run(
                &["send-keys", "-X", "-t", pane, "cancel"],
                "Could not return session pane to live input",
            )?;
        }
        if self.pane_value(pane, "window_zoomed_flag")? == "1"
            && self.pane_value(pane, "pane_active")? != "1"
        {
            self.run(
                &["resize-pane", "-Z", "-t", pane],
                "Could not reveal session pane",
            )?;
        }
        self.run(
            &["select-window", "-t", window],
            "Could not select the session window",
        )?;
        self.run(
            &["select-pane", "-t", pane],
            "Could not focus the session pane",
        )?;
        if let Some(fullscreen) = zoom {
            self.run(
                &[
                    "set-option",
                    "-p",
                    "-t",
                    pane,
                    "@kiro-agents-cycle-zoom",
                    if fullscreen { "1" } else { "0" },
                ],
                "Could not preserve fullscreen navigation mode",
            )?;
            if (self.pane_value(pane, "window_zoomed_flag")? == "1") != fullscreen {
                self.run(
                    &["resize-pane", "-Z", "-t", pane],
                    if fullscreen {
                        "Could not show the session pane fullscreen"
                    } else {
                        "Could not restore the split layout"
                    },
                )?;
            }
        }
        // Switching may close this client's popup: finish pane selection first.
        self.run(
            &["switch-client", "-c", client, "-t", &target(session)],
            "Could not switch to the session workspace",
        )?;
        Ok(())
    }

    /// Ctrl+J/Ctrl+K arrive from a private-server key binding, so the invoking
    /// client and pane travel as environment set by that binding. Both must
    /// agree on this server before any pane is touched.
    fn binding_pane(&self) -> Result<(String, String)> {
        anyhow::ensure!(
            env::var_os("KIRO_AGENTS_WORKSPACE_SOCKET").as_deref() == Some(self.socket.as_os_str()),
            "This pane was not started by kiro-agents; its panes stay untouched"
        );
        let client = env::var("KIRO_AGENTS_WORKSPACE_CLIENT")
            .context("Workspace navigation needs the client that pressed the key")?;
        anyhow::ensure!(
            !client.is_empty() && client.len() <= 4096 && !client.chars().any(char::is_control),
            "Workspace client identity is not usable"
        );
        let pane = env::var("TMUX_PANE").context("Workspace navigation needs the focused pane")?;
        anyhow::ensure!(is_pane_id(&pane), "TMUX_PANE {pane:?} is not a pane ID");
        let output = self.output(&["list-clients", "-F", "#{client_name}\t#{pane_id}"])?;
        anyhow::ensure!(output.status.success(), "No attached workspace client");
        let text = String::from_utf8(output.stdout)?;
        let mut rows = text
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(name, _)| *name == client.as_str());
        let (name, active) = rows
            .next()
            .context("Open this workspace with one attached client to cycle sessions")?;
        anyhow::ensure!(
            rows.next().is_none(),
            "Open this workspace with one attached client to cycle sessions"
        );
        anyhow::ensure!(
            name == client && active == pane,
            "The navigation key was not pressed in this client's focused pane"
        );
        Ok((client, pane))
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

/// A cycle target must be a live pane of an owned workspace that a frontend
/// still owns. `owns_frontend` and `launcher` come from /proc identity checks.
fn cycle_candidate(
    row: &PaneRow,
    managed: &HashSet<String>,
    owns_frontend: bool,
    launcher: bool,
) -> bool {
    !row.dead
        && managed.contains(&row.workspace)
        && validate_session(&row.session).is_ok()
        && owns_frontend
        && !launcher
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pane: &str, workspace: &str, window: u32, index: u32, session: &str) -> PaneRow {
        let window_id = format!("@{window}");
        PaneRow::parse(&format!(
            "{pane}\t4242\t0\t{workspace}\t{window}\t{index}\t{window_id}\t{session}\t100:1:2:3\t"
        ))
        .unwrap()
    }

    fn ordered(panes: &[PaneRow]) -> Vec<String> {
        let mut rows = panes.to_vec();
        rows.sort_by(|left, right| left.order().cmp(&right.order()));
        rows.iter().map(|row| row.pane.clone()).collect()
    }

    #[test]
    fn pane_rows_only_accept_complete_tmux_metadata() {
        assert_eq!(row("%7", "main", 0, 1, "abc123").pane, "%7");
        assert_eq!(row("%7", "main", 0, 1, "abc123").window, "@0");
        let good = "%7\t4242\t0\tmain\t0\t1\t@0\tabc123\t100:1:2:3\t".to_owned();
        for bad in [
            String::new(),
            format!("{good}\textra"),
            good.replace("\t100:1:2:3\t", ""),
            "%7\t4242\tx\tmain\t0\t1\t@0\tabc123\ttok\t".into(),
            "%7\tnot-a-pid\t0\tmain\t0\t1\t@0\tabc123\ttok\t".into(),
            "7\t4242\t0\tmain\t0\t1\t@0\tabc123\ttok\t".into(),
            "%7\t4242\t0\tmain\t0\t1\t0\tabc123\ttok\t".into(),
        ] {
            assert!(PaneRow::parse(&bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn cycle_orders_workspaces_windows_and_panes_numerically() {
        let panes = [
            row("%10", "beta", 0, 0, "b"),
            row("%1", "alpha", 2, 0, "a"),
            row("%9", "alpha", 10, 0, "c"),
            row("%3", "alpha", 2, 1, "d"),
            row("%2", "alpha", 2, 0, "e"),
        ];
        assert_eq!(ordered(&panes), ["%1", "%2", "%3", "%9", "%10"]);
    }

    #[test]
    fn cycle_wraps_and_skips_the_invoking_non_session_pane() {
        let panes: Vec<PaneRow> = ["%1", "%2", "%3"]
            .iter()
            .map(|pane| row(pane, "main", 0, 0, "a"))
            .collect();
        assert_eq!(ordered(&panes), ["%1", "%2", "%3"]);
        let target =
            |pane: &str, step| step_target(&panes, pane, step).map(|at| panes[at].pane.clone());
        assert_eq!(target("%1", PaneStep::Next).as_deref(), Some("%2"));
        assert_eq!(target("%3", PaneStep::Next).as_deref(), Some("%1"));
        assert_eq!(target("%1", PaneStep::Previous).as_deref(), Some("%3"));
        assert_eq!(target("%2", PaneStep::Previous).as_deref(), Some("%1"));
        // A shell or launcher pane enters the near end of the cycle.
        assert_eq!(target("%9", PaneStep::Next).as_deref(), Some("%1"));
        assert_eq!(target("%9", PaneStep::Previous).as_deref(), Some("%3"));
        // The only reachable pane is the one already focused.
        let single = vec![row("%1", "main", 0, 0, "a")];
        assert!(step_target(&single, "%1", PaneStep::Next).is_none());
        assert!(step_target(&single, "%1", PaneStep::Previous).is_none());
        assert!(step_target(&[], "%1", PaneStep::Next).is_none());
    }

    #[test]
    fn only_live_owned_session_panes_are_cycle_targets() {
        let managed: HashSet<String> = ["main".to_owned()].into_iter().collect();
        let session = row("%1", "main", 0, 0, "abc");
        assert!(cycle_candidate(&session, &managed, true, false));
        let mut dead = session.clone();
        dead.dead = true;
        assert!(!cycle_candidate(&dead, &managed, true, false));
        let mut foreign = session.clone();
        foreign.workspace = "someone-elses".into();
        assert!(!cycle_candidate(&foreign, &managed, true, false));
        let mut shell = session.clone();
        shell.session.clear();
        assert!(!cycle_candidate(&shell, &managed, true, false));
        let mut odd = session.clone();
        odd.session = "not a session id!".into();
        assert!(!cycle_candidate(&odd, &managed, true, false));
        // Stale or inherited markers must still belong to this pane's process.
        assert!(!cycle_candidate(&session, &managed, false, false));
        assert!(!cycle_candidate(&session, &managed, true, true));
    }
}
