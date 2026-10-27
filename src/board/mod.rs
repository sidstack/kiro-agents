mod directories;
mod model;
mod picker;
mod screen;
mod view;

use crate::{
    attach, host, kiro,
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
    Stop(String),
    Rename {
        id: String,
        query: String,
    },
}

enum Effect {
    Quit,
    Refresh,
    Attach(String),
    Create { cwd: String, agent: Option<String> },
    Stop(String),
    Rename { id: String, name: Option<String> },
}

struct App {
    board: Board,
    mode: Mode,
    message: String,
    scan: Option<kiro::AgentScanHandle>,
    scan_token: u64,
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
                KeyCode::Esc => self.board.set_query(String::new()),
                KeyCode::Char('r') => return Some(Effect::Refresh),
                KeyCode::Enter => {
                    if let Some(row) = self.board.selected() {
                        if row.state == "live" {
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
                    let effect = Effect::Create {
                        cwd: picker.cwd.clone(),
                        agent,
                    };
                    self.scan = None;
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

pub fn run(dir: &Path) -> Result<()> {
    let signals = Signals::register()?;
    let mut app = App {
        board: Board::default(),
        mode: Mode::Browse,
        message: "Loading sessions…".into(),
        scan: None,
        scan_token: 0,
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
                .draw(|frame| view::draw(frame, &app.board, &app.mode, &app.message))?;
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
            Some(effect @ (Effect::Attach(_) | Effect::Create { .. })) => {
                let id = match effect {
                    Effect::Attach(id) => Some(id),
                    Effect::Create { cwd, agent } => {
                        app.message = "Starting native Kiro…".into();
                        screen
                            .terminal
                            .draw(|frame| view::draw(frame, &app.board, &app.mode, &app.message))?;
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
                    let result =
                        attach::run_from_board(dir, id, signals.interrupted.clone(), || {
                            screen.prepare_native()?;
                            handed_off = true;
                            Ok(())
                        });
                    if handed_off {
                        screen.resume()?;
                    }
                    if let Err(error) = result {
                        app.message = format!("Attach failed: {error:#}");
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
        }
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
