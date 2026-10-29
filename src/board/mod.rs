mod directories;
mod model;
mod picker;
mod screen;
mod view;

use crate::{
    attach,
    features::{PanePlacement, WorkspaceFeature},
    host, kiro,
    protocol::{Action, Reply, SessionInfo},
};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use model::Board;
use picker::{Accepted, AgentPicker, DirPicker};
use screen::Screen;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

enum Mode {
    Browse,
    Search,
    Directory(DirPicker),
    Agent {
        directory: DirPicker,
        picker: AgentPicker,
    },
    Move(String),
    Stop(String),
    Rename {
        id: String,
        query: String,
    },
    Placement {
        flow: PlacementFlow,
        selected: usize,
    },
}

/// What a confirmed pane placement applies to: an existing live session row or
/// a pending creation whose directory/agent forms stay available behind Esc.
enum PlacementFlow {
    Move {
        id: String,
        cwd: String,
    },
    Existing {
        id: String,
        cwd: String,
    },
    Create {
        cwd: String,
        agent: Option<String>,
        directory: DirPicker,
        picker: Box<AgentPicker>,
    },
}

enum Effect {
    Quit,
    Refresh,
    Attach(String),
    Move(String),
    MovePlaced {
        id: String,
        cwd: String,
        placement: PanePlacement,
    },
    Create {
        cwd: String,
        agent: Option<String>,
    },
    /// Floating dashboard only: focus the selected live session in its pane.
    Focus(String, bool),
    /// Floating dashboard only: place a live session through the workspace.
    Open {
        id: String,
        cwd: String,
        placement: PanePlacement,
    },
    /// Floating dashboard only: create after the placement is confirmed.
    CreatePlaced {
        cwd: String,
        agent: Option<String>,
        placement: PanePlacement,
    },
    Stop(String),
    Rename {
        id: String,
        name: Option<String>,
    },
}

struct App {
    board: Board,
    mode: Mode,
    message: String,
    scan: Option<kiro::AgentScanHandle>,
    scan_token: u64,
    popup: bool,
}

impl App {
    fn key(&mut self, mut key: KeyEvent) -> Option<Effect> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        // Legacy terminals/Guacamole encode BS as Ctrl+Backspace or Ctrl+H.
        // Normalize before the control-shortcut guard so editing is not swallowed.
        if key.code == KeyCode::Backspace
            || matches!(key.code, KeyCode::Char('\u{8}' | '\u{7f}'))
            || (key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            key.code = KeyCode::Backspace;
            key.modifiers = KeyModifiers::NONE;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q' | 'c') => return Some(Effect::Quit),
                KeyCode::Char('n') if matches!(self.mode, Mode::Browse) => {
                    let base = std::env::current_dir()
                        .unwrap_or_else(|_| "/".into())
                        .to_string_lossy()
                        .to_string();
                    let mut recent = Vec::new();
                    for row in &self.board.rows {
                        if !recent.contains(&row.cwd) {
                            recent.push(row.cwd.clone());
                        }
                    }
                    self.mode = Mode::Directory(DirPicker::open(base, recent));
                    self.message.clear();
                    return None;
                }
                KeyCode::Char('k') if matches!(self.mode, Mode::Browse) => {
                    if let Some(row) = self.board.selected().filter(|row| row.state == "live") {
                        self.mode = Mode::Stop(row.id.clone());
                    } else {
                        self.message = "Only a live session can be stopped.".into();
                    }
                    return None;
                }
                KeyCode::Char('u') => {
                    match &mut self.mode {
                        Mode::Rename { query, .. } => query.clear(),
                        Mode::Directory(picker) => picker.clear_query(),
                        Mode::Agent { picker, .. } => {
                            picker.query.clear();
                            picker.backspace();
                        }
                        _ => (),
                    }
                    return None;
                }
                KeyCode::Char('r') => {
                    match &mut self.mode {
                        Mode::Directory(picker) => picker.refresh(),
                        Mode::Agent { picker, .. } => {
                            self.scan = None;
                            self.scan_token += 1;
                            self.scan = Some(picker.retry(self.scan_token, kiro::LIST_TIMEOUT));
                        }
                        _ => return Some(Effect::Refresh),
                    }
                    return None;
                }
                _ => return None,
            }
        }
        match &mut self.mode {
            Mode::Browse => match key.code {
                KeyCode::F(2) => {
                    if let Some(row) = self.board.selected() {
                        self.mode = Mode::Rename {
                            id: row.id.clone(),
                            query: row.name.clone().unwrap_or_default(),
                        };
                        self.message.clear();
                    }
                }
                KeyCode::Char('q') => return Some(Effect::Quit),
                KeyCode::Up | KeyCode::Char('k') => self.board.move_selection(-1),
                KeyCode::Down | KeyCode::Char('j') => self.board.move_selection(1),
                KeyCode::Tab => self.board.toggle_filter(),
                KeyCode::Char('/') => {
                    self.board.set_query(String::new());
                    self.mode = Mode::Search;
                }
                KeyCode::Esc if self.popup => return Some(Effect::Quit),
                KeyCode::Esc => self.board.set_query(String::new()),
                KeyCode::Char('r') => return Some(Effect::Refresh),
                KeyCode::Char('m') => {
                    if let Some(row) = self.board.selected().filter(|row| row.state == "live") {
                        self.mode = Mode::Move(row.id.clone());
                        self.message.clear();
                    } else {
                        self.message = "Only a live session can move between terminals.".into();
                    }
                }
                KeyCode::Char(ch @ ('v' | 'h' | 'w')) if self.popup => {
                    if let Some(row) = self.board.selected().filter(|row| row.state == "live") {
                        let placement = match ch {
                            'v' => PanePlacement::Right,
                            'h' => PanePlacement::Below,
                            _ => PanePlacement::Window,
                        };
                        return Some(Effect::Open {
                            id: row.id.clone(),
                            cwd: row.cwd.clone(),
                            placement,
                        });
                    }
                    self.message =
                        "Only a live session can be placed in a workspace pane; Ctrl+N creates one."
                            .into();
                }
                KeyCode::Enter | KeyCode::Char('f') if self.popup || key.code == KeyCode::Enter => {
                    if let Some(row) = self.board.selected() {
                        if row.state == "live" {
                            if self.popup {
                                return Some(Effect::Focus(
                                    row.id.clone(),
                                    key.modifiers.contains(KeyModifiers::SHIFT)
                                        || key.code == KeyCode::Char('f'),
                                ));
                            }
                            return Some(Effect::Attach(row.id.clone()));
                        }
                        self.message = format!("Cannot attach: session is {}. Create a fresh session or use Kiro history manually.", row.state);
                    }
                }
                _ => (),
            },
            Mode::Search => match key.code {
                KeyCode::Enter => self.mode = Mode::Browse,
                KeyCode::Esc => {
                    self.board.set_query(String::new());
                    self.mode = Mode::Browse;
                }
                KeyCode::Backspace => {
                    let mut query = self.board.query.clone();
                    query.pop();
                    self.board.set_query(query);
                }
                KeyCode::Char(ch) => {
                    let mut query = self.board.query.clone();
                    query.push(ch);
                    self.board.set_query(query);
                }
                _ => (),
            },
            Mode::Directory(picker) => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Browse;
                    self.message.clear();
                }
                KeyCode::Backspace => {
                    picker.backspace();
                    self.message.clear();
                }
                KeyCode::Char(ch) => {
                    picker.press(ch);
                    self.message.clear();
                }
                KeyCode::Up => picker.move_selection(-1),
                KeyCode::Down => picker.move_selection(1),
                KeyCode::PageUp => picker.move_selection(-8),
                KeyCode::PageDown => picker.move_selection(8),
                KeyCode::Left => {
                    picker.parent();
                }
                KeyCode::Right | KeyCode::Tab => {
                    picker.descend();
                }
                KeyCode::Enter => {
                    if let Some(cwd) = picker.accept() {
                        if !Path::new(&cwd).is_dir() {
                            self.message =
                                "Cannot create: directory is no longer available.".into();
                            return None;
                        }
                        let directory = picker.clone();
                        self.scan_token += 1;
                        let (picker, scan) =
                            AgentPicker::open(cwd, self.scan_token, kiro::LIST_TIMEOUT);
                        self.scan = Some(scan);
                        self.mode = Mode::Agent { directory, picker };
                        self.message.clear();
                    } else {
                        self.message =
                            "Cannot create: choose an existing directory or edit the path.".into();
                    }
                }
                _ => (),
            },
            Mode::Agent { directory, picker } => match key.code {
                KeyCode::Esc => {
                    self.scan = None;
                    self.mode = Mode::Directory(directory.clone());
                    self.message.clear();
                }
                KeyCode::Backspace => picker.backspace(),
                KeyCode::Char(ch) => picker.press(ch),
                KeyCode::Up => picker.move_selection(-1),
                KeyCode::Down => picker.move_selection(1),
                KeyCode::PageUp => picker.move_selection(-8),
                KeyCode::PageDown => picker.move_selection(8),
                KeyCode::Enter => {
                    let agent = match picker.accept() {
                        Accepted::Default => None,
                        Accepted::Agent(name) | Accepted::Manual(name) => Some(name),
                    };
                    if self.popup {
                        let flow = PlacementFlow::Create {
                            cwd: picker.cwd.clone(),
                            agent,
                            directory: directory.clone(),
                            picker: Box::new(picker.clone()),
                        };
                        self.scan = None;
                        self.mode = Mode::Placement {
                            flow,
                            selected: default_placement_index(),
                        };
                        self.message.clear();
                        return None;
                    }
                    let effect = Effect::Create {
                        cwd: picker.cwd.clone(),
                        agent,
                    };
                    self.scan = None;
                    return Some(effect);
                }
                _ => (),
            },
            Mode::Placement { flow, selected } => match key.code {
                KeyCode::Esc => {
                    let back = match flow {
                        PlacementFlow::Existing { .. } | PlacementFlow::Move { .. } => Mode::Browse,
                        PlacementFlow::Create {
                            directory, picker, ..
                        } => Mode::Agent {
                            directory: directory.clone(),
                            picker: picker.as_ref().clone(),
                        },
                    };
                    self.mode = back;
                    self.message.clear();
                }
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(PLACEMENTS.len() - 1),
                code @ (KeyCode::Enter | KeyCode::Char('v' | 'h' | 'w')) => {
                    let index = match code {
                        KeyCode::Enter => *selected,
                        KeyCode::Char('v') => 0,
                        KeyCode::Char('h') => 1,
                        _ => 2,
                    };
                    let placement = PLACEMENTS[index].1;
                    let effect = match flow {
                        PlacementFlow::Move { id, cwd } => Effect::MovePlaced {
                            id: id.clone(),
                            cwd: cwd.clone(),
                            placement,
                        },
                        PlacementFlow::Existing { id, cwd } => Effect::Open {
                            id: id.clone(),
                            cwd: cwd.clone(),
                            placement,
                        },
                        PlacementFlow::Create { cwd, agent, .. } => Effect::CreatePlaced {
                            cwd: cwd.clone(),
                            agent: agent.clone(),
                            placement,
                        },
                    };
                    return Some(effect);
                }
                _ => (),
            },
            Mode::Rename { id, query } => match key.code {
                KeyCode::Esc => self.mode = Mode::Browse,
                KeyCode::Backspace => {
                    query.pop();
                }
                KeyCode::Char(ch) if !ch.is_control() => {
                    if query.chars().count() < 80 {
                        query.push(ch);
                    }
                }
                KeyCode::Enter => {
                    let name = if query.trim().is_empty() {
                        None
                    } else {
                        match crate::features::validate_name(query) {
                            Ok(name) => Some(name),
                            Err(error) => {
                                self.message = error.to_string();
                                return None;
                            }
                        }
                    };
                    return Some(Effect::Rename {
                        id: id.clone(),
                        name,
                    });
                }
                _ => (),
            },
            Mode::Move(id) => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let id = id.clone();
                    self.mode = Mode::Browse;
                    return Some(Effect::Move(id));
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => (),
            },
            Mode::Stop(id) => match key.code {
                KeyCode::Char('y' | 'Y') => {
                    let id = id.clone();
                    self.mode = Mode::Browse;
                    return Some(Effect::Stop(id));
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => self.mode = Mode::Browse,
                _ => (),
            },
        }
        None
    }

    fn poll_scan(&mut self) -> bool {
        let directories_changed = match &mut self.mode {
            Mode::Directory(picker)
            | Mode::Agent {
                directory: picker, ..
            } => picker.poll_directories(),
            _ => false,
        };
        let result = self.scan.as_ref().and_then(|scan| scan.try_recv().ok());
        if let Some(scan) = result {
            self.scan = None;
            if let Mode::Agent { picker, .. } = &mut self.mode {
                return picker.apply(scan);
            }
        }
        directories_changed
    }

    fn apply_rows(&mut self, result: Result<Vec<SessionInfo>, String>) -> bool {
        match result {
            Ok(rows) => {
                let changed = self.board.rows != rows;
                self.board.replace_rows(rows);
                if self.message.starts_with("Refresh failed:")
                    || self.message == "Loading sessions…"
                {
                    self.message.clear();
                    return true;
                }
                changed
            }
            Err(error) => {
                let message = format!("Refresh failed: {error}. r retries; q quits.");
                let changed = self.message != message;
                self.message = message;
                changed
            }
        }
    }
}

fn refresh_task(dir: &Path) -> std::sync::mpsc::Receiver<Result<Vec<SessionInfo>, String>> {
    let (send, receive) = std::sync::mpsc::channel();
    let dir = dir.to_owned();
    std::thread::spawn(move || {
        let _ = send.send(sessions(&dir).map_err(|error| format!("{error:#}")));
    });
    receive
}

fn sessions(dir: &Path) -> Result<Vec<SessionInfo>> {
    if !dir.join("host.sock").exists() && !dir.join("registry.json").exists() {
        return Ok(Vec::new());
    }
    match host::call(dir, Action::Status, true)? {
        Reply::Sessions(mut rows) => {
            crate::custom_names::enrich(dir, &mut rows)?;
            Ok(rows)
        }
        _ => anyhow::bail!("Unexpected status response"),
    }
}

struct Signals {
    ids: Vec<signal_hook::SigId>,
    interrupted: Arc<AtomicBool>,
}

impl Signals {
    fn register() -> Result<Self> {
        let mut signals = Self {
            ids: Vec::new(),
            interrupted: Arc::new(AtomicBool::new(false)),
        };
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            signals.ids.push(signal_hook::flag::register(
                signal,
                signals.interrupted.clone(),
            )?);
        }
        Ok(signals)
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

/// Pane placements offered by the floating dashboard, in picker order.
const PLACEMENTS: [(&str, PanePlacement); 3] = [
    ("Split right (default)", PanePlacement::Right),
    ("Split below", PanePlacement::Below),
    ("New window", PanePlacement::Window),
];

/// Initial placement picker index from KIRO_AGENTS_WORKSPACE_PLACEMENT.
fn placement_index(value: &str) -> usize {
    match value.trim().to_ascii_lowercase().as_str() {
        "below" => 1,
        "window" => 2,
        _ => 0,
    }
}

fn default_placement_index() -> usize {
    placement_index(&std::env::var("KIRO_AGENTS_WORKSPACE_PLACEMENT").unwrap_or_default())
}

pub fn run(dir: &Path) -> Result<()> {
    run_board(dir, None)
}

/// Floating dashboard for the workspace popup. It navigates existing sessions
/// and places them through the workspace feature; it never attaches a native
/// PTY inside the popup, so the focused session stays attached elsewhere.
pub fn run_popup(dir: &Path, navigator: &dyn WorkspaceFeature) -> Result<()> {
    run_board(dir, Some(navigator))
}

fn run_board(dir: &Path, navigator: Option<&dyn WorkspaceFeature>) -> Result<()> {
    let signals = Signals::register()?;
    let mut app = App {
        board: Board::default(),
        mode: Mode::Browse,
        message: "Loading sessions…".into(),
        scan: None,
        scan_token: 0,
        popup: navigator.is_some(),
    };
    let mut refresh = Some(refresh_task(dir));
    let mut screen = Screen::open()?;
    let mut last_refresh = Instant::now();
    let mut dirty = true;
    while !signals.interrupted.load(Ordering::Relaxed) && terminal_connected()? {
        if let Some(result) = refresh.as_ref().and_then(|task| task.try_recv().ok()) {
            refresh = None;
            dirty |= app.apply_rows(result);
        }
        dirty |= app.poll_scan();
        if dirty {
            screen
                .terminal
                .draw(|frame| view::draw(frame, &app.board, &app.mode, &app.message, app.popup))?;
            dirty = false;
        }
        let effect = if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(key) => {
                    dirty = true;
                    app.key(key)
                }
                Event::Resize(_, _) => {
                    screen.redraw()?;
                    dirty = true;
                    None
                }
                _ => None,
            }
        } else {
            None
        };
        match effect {
            Some(Effect::Quit) => break,
            Some(Effect::Refresh) => {
                app.message.clear();
                if refresh.is_none() {
                    refresh = Some(refresh_task(dir));
                }
                screen.redraw()?;
                dirty = true;
                last_refresh = Instant::now();
            }
            Some(Effect::Rename { id, name }) => {
                match crate::custom_names::set(dir, &id, name) {
                    Ok(()) => {
                        app.mode = Mode::Browse;
                        app.message = "Session name updated.".into();
                    }
                    Err(error) => app.message = format!("Rename failed: {error:#}"),
                }
                refresh = Some(refresh_task(dir));
                dirty = true;
            }
            Some(Effect::Stop(id)) => {
                match host::call(dir, Action::Stop { id }, false) {
                    Ok(Reply::Ok) => app.message = "Session stopped.".into(),
                    Ok(_) => app.message = "Unexpected stop response".into(),
                    Err(error) => app.message = format!("Stop failed: {error:#}"),
                }
                refresh = Some(refresh_task(dir));
                dirty = true;
            }
            Some(Effect::Focus(id, zoom)) => {
                // Only the floating dashboard produces focus effects.
                if let Some(navigator) = navigator {
                    match navigator.focus_session(&id, zoom) {
                        Ok(true) => return Ok(()),
                        Ok(false) => {
                            let cwd = app
                                .board
                                .rows
                                .iter()
                                .find(|row| row.id == id)
                                .map(|row| row.cwd.clone());
                            match cwd {
                                Some(cwd) => {
                                    app.mode = Mode::Placement {
                                        flow: PlacementFlow::Existing { id, cwd },
                                        selected: default_placement_index(),
                                    };
                                    app.message.clear();
                                }
                                None => {
                                    app.message =
                                        "Session is no longer listed; r refreshes the board.".into()
                                }
                            }
                        }
                        Err(error) => app.message = format!("Workspace focus failed: {error:#}"),
                    }
                    dirty = true;
                }
            }
            Some(Effect::Move(id)) if app.popup => {
                if let Some(row) = app.board.rows.iter().find(|row| row.id == id) {
                    app.mode = Mode::Placement {
                        flow: PlacementFlow::Move {
                            id,
                            cwd: row.cwd.clone(),
                        },
                        selected: default_placement_index(),
                    };
                    app.message.clear();
                }
                dirty = true;
            }
            Some(effect @ (Effect::Open { .. } | Effect::MovePlaced { .. })) => {
                let move_here = matches!(&effect, Effect::MovePlaced { .. });
                let (id, cwd, placement) = match effect {
                    Effect::Open { id, cwd, placement }
                    | Effect::MovePlaced { id, cwd, placement } => (id, cwd, placement),
                    _ => unreachable!(),
                };
                if let Some(navigator) = navigator {
                    let result = if move_here {
                        navigator.move_session(&id, Path::new(&cwd), placement)
                    } else {
                        navigator.open_session(&id, Path::new(&cwd), placement)
                    };
                    match result {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            app.message = format!("Workspace placement failed: {error:#}");
                        }
                    }
                    dirty = true;
                }
            }
            Some(Effect::CreatePlaced {
                cwd,
                agent,
                placement,
            }) => {
                if let Some(navigator) = navigator {
                    app.message = "Starting native Kiro…".into();
                    screen.terminal.draw(|frame| {
                        view::draw(frame, &app.board, &app.mode, &app.message, app.popup)
                    })?;
                    match host::call(dir, Action::New { cwd, agent }, true) {
                        Ok(Reply::Created(row)) => {
                            let created = *row;
                            app.board.rows.push(created.clone());
                            app.board.set_query(String::new());
                            app.board.select_id(&created.id);
                            app.mode = Mode::Browse;
                            match navigator.open_session(
                                &created.id,
                                Path::new(&created.cwd),
                                placement,
                            ) {
                                Ok(()) => return Ok(()),
                                Err(error) => {
                                    app.message = format!(
                                        "Session created; workspace placement failed: {error:#}"
                                    )
                                }
                            }
                        }
                        Ok(_) => app.message = "Unexpected creation response".into(),
                        Err(error) => app.message = format!("Create failed: {error:#}"),
                    }
                    refresh = Some(refresh_task(dir));
                    last_refresh = Instant::now();
                    dirty = true;
                }
            }
            Some(effect @ (Effect::Attach(_) | Effect::Move(_) | Effect::Create { .. })) => {
                let move_here = matches!(&effect, Effect::Move(_));
                let id = match effect {
                    Effect::Attach(id) | Effect::Move(id) => Some(id),
                    Effect::Create { cwd, agent } => {
                        app.message = "Starting native Kiro…".into();
                        screen.terminal.draw(|frame| {
                            view::draw(frame, &app.board, &app.mode, &app.message, app.popup)
                        })?;
                        match host::call(dir, Action::New { cwd, agent }, true) {
                            Ok(Reply::Created(row)) => {
                                let id = row.id.clone();
                                app.board.rows.push(*row);
                                app.board.set_query(String::new());
                                app.board.select_id(&id);
                                app.mode = Mode::Browse;
                                Some(id)
                            }
                            Ok(_) => {
                                app.message = "Unexpected creation response".into();
                                None
                            }
                            Err(error) => {
                                app.message = format!("Create failed: {error:#}");
                                None
                            }
                        }
                    }
                    _ => unreachable!(),
                };
                if let Some(id) = id {
                    app.message.clear();
                    let mut handed_off = false;
                    let on_handoff = || {
                        screen.prepare_native()?;
                        handed_off = true;
                        Ok(())
                    };
                    let result = if move_here {
                        attach::run_from_board_move(
                            dir,
                            id.clone(),
                            signals.interrupted.clone(),
                            on_handoff,
                        )
                    } else {
                        attach::run_from_board(
                            dir,
                            id.clone(),
                            signals.interrupted.clone(),
                            on_handoff,
                        )
                    };
                    if handed_off {
                        screen.resume()?;
                    }
                    if let Err(error) = result {
                        if !move_here && attach::is_attached_elsewhere(&error) {
                            app.mode = Mode::Move(id);
                        } else {
                            app.message = format!("Attach failed: {error:#}");
                        }
                    }
                    if signals.interrupted.load(Ordering::Relaxed) {
                        break;
                    }
                }
                refresh = Some(refresh_task(dir));
                last_refresh = Instant::now();
                dirty = true;
            }
            None => (),
        }
        if refresh.is_none() && last_refresh.elapsed() >= Duration::from_secs(1) {
            refresh = Some(refresh_task(dir));
            last_refresh = Instant::now();
        }
    }
    Ok(())
}

// A closed PTY is a frontend exit, never an input event. Check separately from
// Crossterm so HUP-only readiness cannot turn its timed poll into a busy loop.
fn terminal_connected() -> std::io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: 0,
        revents: 0,
    };
    // SAFETY: descriptor is initialized and borrowed only for this nonblocking poll.
    let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(true);
        }
        return Err(error);
    }
    Ok(descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(state: &str) -> App {
        let mut board = Board::default();
        board.all = true;
        board.replace_rows(vec![SessionInfo {
            name: None,
            title: None,
            title_source: None,
            id: "selected".into(),
            cwd: "/tmp".into(),
            agent: None,
            generation: "v2".into(),
            pid: None,
            state: state.into(),
            exit_code: None,
            notice: None,
        }]);
        App {
            board,
            mode: Mode::Browse,
            message: String::new(),
            scan: None,
            scan_token: 0,
            popup: false,
        }
    }

    fn popup_app(state: &str) -> App {
        let mut app = app(state);
        app.popup = true;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn history_is_explained_without_attach_or_stop() {
        let mut app = app("exited");
        assert!(app.key(key(KeyCode::Enter)).is_none());
        assert!(app.message.contains("Cannot attach"));
        assert!(app
            .key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
            .is_none());
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn stop_requires_confirmation_and_cancellation_preserves_session() {
        let mut app = app("live");
        let stop_key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert!(app.key(stop_key).is_none());
        assert!(app.key(key(KeyCode::Char('n'))).is_none());
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.key(stop_key).is_none());
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(app.key(stop_key).is_none());
        assert!(
            matches!(app.key(key(KeyCode::Char('y'))), Some(Effect::Stop(id)) if id == "selected")
        );
    }

    #[test]
    fn move_is_explicit_cancellable_and_only_for_live_sessions() {
        let mut app = app("live");
        assert!(app.key(key(KeyCode::Char('m'))).is_none());
        assert!(matches!(app.mode, Mode::Move(_)));
        app.key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Browse));
        app.key(key(KeyCode::Char('m')));
        app.key(key(KeyCode::Char('n')));
        assert!(matches!(app.mode, Mode::Browse));
        app.key(key(KeyCode::Char('m')));
        assert!(
            matches!(app.key(key(KeyCode::Char('y'))), Some(Effect::Move(id)) if id == "selected")
        );
        assert!(matches!(app.mode, Mode::Browse));
        app.board.rows[0].state = "exited".into();
        app.key(key(KeyCode::Char('m')));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.message.contains("Only a live"));
    }

    #[test]
    fn invalid_directory_stays_editable_without_create() {
        let mut app = app("live");
        let mut picker = DirPicker::open("/tmp".into(), Vec::new());
        picker.query = "/nonexistent/kiro-agents-board-project".into();
        picker.rebuild();
        app.mode = Mode::Directory(picker);
        assert!(app.key(key(KeyCode::Enter)).is_none());
        assert!(matches!(app.mode, Mode::Directory(_)));
        assert!(app.message.contains("Cannot create"));
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(matches!(app.mode, Mode::Browse));
    }

    fn agent_mode() -> Mode {
        let mut directory = DirPicker::open("/tmp".into(), Vec::new());
        directory.query = "project-filter".into();
        Mode::Agent {
            directory,
            picker: AgentPicker {
                cwd: "/tmp".into(),
                query: String::new(),
                names: vec!["reviewer".into()],
                options: vec![None, Some("reviewer".into())],
                selected: 1,
                status: picker::AgentStatus::Ready,
                token: 1,
                truncated: false,
            },
        }
    }

    #[test]
    fn create_keeps_the_agent_form_until_host_success() {
        let mut app = app("live");
        app.mode = agent_mode();
        assert!(
            matches!(app.key(key(KeyCode::Enter)), Some(Effect::Create { cwd, agent: Some(agent) }) if cwd == "/tmp" && agent == "reviewer")
        );
        assert!(matches!(app.mode, Mode::Agent { .. }));
    }

    #[test]
    fn escape_preserves_directory_query_before_returning_to_board() {
        let mut app = app("live");
        app.mode = agent_mode();
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(matches!(&app.mode, Mode::Directory(picker) if picker.query == "project-filter"));
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn create_shortcut_preserves_an_active_form_or_confirmation() {
        let mut app = app("live");
        let create = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        app.mode = Mode::Agent {
            directory: DirPicker::open("/tmp".into(), Vec::new()),
            picker: AgentPicker {
                cwd: "/tmp".into(),
                query: "reviewer".into(),
                names: Vec::new(),
                options: vec![None],
                selected: 0,
                status: picker::AgentStatus::Ready,
                token: 0,
                truncated: false,
            },
        };
        assert!(app.key(create).is_none());
        assert!(matches!(&app.mode, Mode::Agent { picker, .. } if picker.query == "reviewer"));
        app.mode = Mode::Stop("selected".into());
        assert!(app.key(create).is_none());
        assert!(matches!(&app.mode, Mode::Stop(id) if id == "selected"));
    }
    #[test]
    fn both_backspace_encodings_edit_directory_agent_search_and_names() {
        for backspace in [
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        ] {
            let mut app = app("live");
            app.mode = Mode::Directory(DirPicker::open("/tmp".into(), Vec::new()));
            app.key(key(KeyCode::Char('Ü')));
            app.key(backspace);
            assert!(matches!(&app.mode, Mode::Directory(picker) if picker.query.is_empty()));
            app.mode = agent_mode();
            app.key(key(KeyCode::Char('Ü')));
            app.key(backspace);
            assert!(matches!(&app.mode, Mode::Agent {picker, ..} if picker.query.is_empty()));
            app.mode = Mode::Search;
            app.key(key(KeyCode::Char('Ü')));
            app.key(backspace);
            assert!(app.board.query.is_empty());
            app.mode = Mode::Rename {
                id: "selected".into(),
                query: "Ü".into(),
            };
            app.key(backspace);
            assert!(matches!(&app.mode, Mode::Rename {query, ..} if query.is_empty()));
        }
    }

    #[test]
    fn standalone_board_keeps_attach_escape_and_create_effects() {
        let mut app = app("live");
        assert!(
            matches!(app.key(key(KeyCode::Enter)), Some(Effect::Attach(id)) if id == "selected")
        );
        app.board.set_query("tmp".into());
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(app.board.query.is_empty());
        assert!(app.key(key(KeyCode::Char('v'))).is_none());
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn popup_enter_focuses_without_attaching_a_native_pty() {
        let mut app = popup_app("live");
        assert!(
            matches!(app.key(key(KeyCode::Enter)), Some(Effect::Focus(id, false)) if id == "selected")
        );
        let mut app = popup_app("exited");
        assert!(app.key(key(KeyCode::Enter)).is_none());
        assert!(app.message.contains("Cannot attach"));
    }

    #[test]
    fn popup_fullscreen_shortcuts_request_zoom() {
        for key in [
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
        ] {
            let mut app = popup_app("live");
            assert!(matches!(app.key(key), Some(Effect::Focus(id, true)) if id == "selected"));
        }
    }

    #[test]
    fn popup_browse_shortcuts_choose_the_explicit_placement() {
        for (code, placement) in [
            (KeyCode::Char('v'), PanePlacement::Right),
            (KeyCode::Char('h'), PanePlacement::Below),
            (KeyCode::Char('w'), PanePlacement::Window),
        ] {
            let mut app = popup_app("live");
            assert!(matches!(
                app.key(key(code)),
                Some(Effect::Open { id, cwd, placement: chosen })
                    if id == "selected" && cwd == "/tmp" && chosen == placement
            ));
        }
        let mut app = popup_app("exited");
        for code in [KeyCode::Char('v'), KeyCode::Char('h'), KeyCode::Char('w')] {
            assert!(app.key(key(code)).is_none());
        }
        assert!(app.message.contains("Only a live session"));
    }

    #[test]
    fn popup_escape_closes_the_floating_dashboard() {
        let mut app = popup_app("live");
        app.board.set_query("tmp".into());
        assert!(matches!(app.key(key(KeyCode::Esc)), Some(Effect::Quit)));
        let mut app = popup_app("live");
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(Effect::Quit)
        ));
        // Kiro's native Ctrl+G does not become a wrapper dashboard shortcut.
        let mut app = popup_app("live");
        app.mode = agent_mode();
        assert!(app
            .key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL))
            .is_none());
        assert!(matches!(&app.mode, Mode::Agent { .. }));
    }

    #[test]
    fn popup_create_shows_placement_before_the_host_call() {
        let mut app = popup_app("live");
        app.mode = agent_mode();
        assert!(app.key(key(KeyCode::Enter)).is_none());
        assert!(matches!(
            &app.mode,
            Mode::Placement { flow: PlacementFlow::Create { agent: Some(agent), .. }, selected: 0 }
                if agent == "reviewer"
        ));
        assert!(matches!(
            app.key(key(KeyCode::Char('h'))),
            Some(Effect::CreatePlaced { cwd, agent: Some(agent), placement: PanePlacement::Below })
                if cwd == "/tmp" && agent == "reviewer"
        ));
        assert!(matches!(&app.mode, Mode::Placement { .. }));
    }

    #[test]
    fn placement_escape_restores_agent_form_and_confirms_existing_rows() {
        let mut app = popup_app("live");
        app.mode = agent_mode();
        assert!(app.key(key(KeyCode::Enter)).is_none());
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(matches!(
            &app.mode,
            Mode::Agent { directory, picker }
                if directory.query == "project-filter" && picker.cwd == "/tmp"
        ));
        let existing = || Mode::Placement {
            flow: PlacementFlow::Existing {
                id: "selected".into(),
                cwd: "/tmp".into(),
            },
            selected: placement_index("window"),
        };
        app.mode = existing();
        assert!(matches!(
            app.key(key(KeyCode::Enter)),
            Some(Effect::Open { id, cwd, placement: PanePlacement::Window })
                if id == "selected" && cwd == "/tmp"
        ));
        app.mode = existing();
        assert!(app.key(key(KeyCode::Esc)).is_none());
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn placement_arrows_and_environment_default_select_a_pane() {
        assert_eq!(placement_index("right"), 0);
        assert_eq!(placement_index("BELOW"), 1);
        assert_eq!(placement_index(" window "), 2);
        assert_eq!(placement_index(""), 0);
        let mut app = popup_app("live");
        app.mode = Mode::Placement {
            flow: PlacementFlow::Existing {
                id: "selected".into(),
                cwd: "/tmp".into(),
            },
            selected: 0,
        };
        app.key(key(KeyCode::Down));
        app.key(key(KeyCode::Down));
        assert!(matches!(&app.mode, Mode::Placement { selected: 2, .. }));
        app.key(key(KeyCode::Down));
        assert!(matches!(&app.mode, Mode::Placement { selected: 2, .. }));
        app.key(key(KeyCode::Up));
        assert!(matches!(&app.mode, Mode::Placement { selected: 1, .. }));
        assert!(matches!(
            app.key(key(KeyCode::Enter)),
            Some(Effect::Open {
                placement: PanePlacement::Below,
                ..
            })
        ));
    }

    #[test]
    fn rename_is_explicit_and_clearing_restores_automatic_titles() {
        let mut app = app("live");
        app.key(key(KeyCode::F(2)));
        assert!(matches!(app.mode, Mode::Rename { .. }));
        app.key(key(KeyCode::Char('n')));
        assert!(
            matches!(app.key(key(KeyCode::Enter)), Some(Effect::Rename {name:Some(name),..}) if name == "n")
        );
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(matches!(
            app.key(key(KeyCode::Enter)),
            Some(Effect::Rename { name: None, .. })
        ));
        app.key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Browse));
    }
}
