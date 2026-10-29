use super::{model::Board, picker::AgentStatus, Mode, PlacementFlow, PLACEMENTS};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

const ACCENT: Color = Color::Cyan;

pub(super) fn draw(frame: &mut Frame, board: &Board, mode: &Mode, message: &str, popup: bool) {
    let area = frame.area();
    if area.width < 35 || area.height < 12 {
        frame.render_widget(
            Paragraph::new(
                "Kiro sessions\nEnlarge terminal to browse.\nCtrl+Q quits; sessions stay alive.",
            )
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let areas = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(area);
    let live = board.rows.iter().filter(|row| row.state == "live").count();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if board.all { " ALL " } else { " LIVE " },
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {live}/8 live  ·  {} historical  ·  Search: {}",
                board.rows.len() - live,
                board.query
            )),
        ]))
        .block(Block::bordered().title(format!(
            "Kiro sessions{} · v{}",
            if popup { " · floating" } else { "" },
            env!("CARGO_PKG_VERSION")
        ))),
        areas[0],
    );
    let visible = board.visible_indices();
    let items = visible
        .iter()
        .map(|&index| {
            let row = &board.rows[index];
            let name = row.display_name();
            let color = match row.state.as_str() {
                "live" => Color::Green,
                "failed" => Color::Red,
                _ => Color::DarkGray,
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{name}  "),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" · "),
                    Span::styled(row.state.clone(), Style::default().fg(color)),
                ]),
                Line::styled(
                    format!(
                        "{} · {} · {} · {}",
                        row.agent.as_deref().unwrap_or("Kiro default"),
                        row.generation,
                        row.id,
                        row.cwd
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        })
        .collect::<Vec<_>>();
    let selected = board.selected().and_then(|row| {
        visible
            .iter()
            .position(|&index| board.rows[index].id == row.id)
    });
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title("Projects and sessions"))
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        areas[1],
        &mut state,
    );
    let details = match mode {
        Mode::Search => format!("Search: {}\nEnter keeps filter · Esc clears", board.query),
        _ => board.selected().map_or_else(
            || "No matching sessions. Ctrl+N creates a fresh session.".into(),
            |row| if row.state == "live" {
                if popup {
                    format!("{}\nEnter focuses · Shift+Enter / f focuses fullscreen · v split right · h split below · w new window", row.cwd)
                } else {
                    format!("{}\nEnter attaches · Ctrl+Q returns here", row.cwd)
                }
            } else {
                format!("Cannot attach: session is {} (exit {}). Use Kiro history manually or Ctrl+N for a fresh session.\n{}",
                    row.state, row.exit_code.map_or_else(|| "unknown".into(), |code| code.to_string()), row.notice.as_deref().unwrap_or(""))
            },
        ),
    };
    frame.render_widget(Paragraph::new(details).wrap(Wrap { trim: false }), areas[2]);
    let (select_line, actions_line) = if popup {
        (
            "↑/↓ j/k select · Enter focus · Shift+Enter / f fullscreen · v/h split · w window",
            "Ctrl+N new · m move here · F2 rename · Ctrl+K stop · / search · Esc closes popup",
        )
    } else {
        (
            "↑/↓ j/k select · Enter attach · Tab Live/All · / search · r refresh",
            "Ctrl+N create · m move here · F2 rename · Ctrl+K stop · q/Ctrl+Q quit",
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(select_line),
            Line::raw(actions_line),
            Line::styled(message, Style::default().fg(Color::Yellow)),
        ]),
        areas[3],
    );

    if matches!(
        mode,
        Mode::Directory(_)
            | Mode::Agent { .. }
            | Mode::Move(_)
            | Mode::Stop(_)
            | Mode::Rename { .. }
            | Mode::Placement { .. }
    ) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("Kiro sessions"),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }
    match mode {
        Mode::Directory(picker) => {
            let steps = if popup { "1/3" } else { "1/2" };
            let area = modal(frame, &format!("New session: directory · {steps}"));
            let parts = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(4),
            ])
            .split(area);
            frame.render_widget(
                Paragraph::new(format!(
                    "Browse: {}\nSearch or path: {}",
                    picker.base, picker.query
                )),
                parts[0],
            );
            input_cursor(frame, parts[0], "Search or path: ", &picker.query, 1);
            // Materialize only the viewport, even for a six-figure cached list.
            let visible = usize::from(parts[1].height).max(1);
            let start = picker.selected.saturating_sub(visible.saturating_sub(1));
            let items = picker
                .candidates
                .iter()
                .skip(start)
                .take(visible)
                .map(|row| ListItem::new(format!("{:<7} {}", row.origin.label(), row.path)))
                .collect::<Vec<_>>();
            let mut state = ListState::default()
                .with_selected((!items.is_empty()).then_some(picker.selected - start));
            frame.render_stateful_widget(choice_list(items), parts[1], &mut state);
            let notice = if !message.is_empty() {
                message
            } else {
                picker.notice.as_deref().unwrap_or(if picker.scanning {
                    "Discovering directories recursively… Type to search while loading."
                } else if picker.truncated {
                    "Directory scan limit reached; Ctrl+R retries or type an explicit path."
                } else {
                    "Choose a directory, then choose its native Kiro agent."
                })
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw("Type a fuzzy query · Enter chooses best match · ↑/↓ optional"),
                    Line::raw(
                        "Tab/→ browse · ← parent · Backspace edit · Ctrl+U clear · Esc cancel",
                    ),
                    Line::styled(notice, Style::default().fg(Color::Yellow)),
                ])
                .wrap(Wrap { trim: false }),
                parts[2],
            );
        }
        Mode::Agent { picker, .. } => {
            let steps = if popup { "2/3" } else { "2/2" };
            let area = modal(frame, &format!("New session: agent (optional) · {steps}"));
            let parts = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(4),
            ])
            .split(area);
            frame.render_widget(
                Paragraph::new(format!(
                    "Directory: {}\nFilter: {}",
                    picker.cwd, picker.query
                )),
                parts[0],
            );
            input_cursor(frame, parts[0], "Filter: ", &picker.query, 1);
            let items = picker
                .options
                .iter()
                .map(|option| {
                    ListItem::new(match option {
                        None => "Kiro default (no --agent)".into(),
                        Some(name) if picker.names.contains(name) => name.clone(),
                        Some(name) => format!("Use typed agent: {name} (unverified)"),
                    })
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(picker.selected));
            frame.render_stateful_widget(choice_list(items), parts[1], &mut state);
            let status = if !message.is_empty() {
                message
            } else {
                match &picker.status {
                    AgentStatus::Loading => {
                        "Discovering agents… Keyboard and Esc remain available."
                    }
                    AgentStatus::Ready if picker.truncated => {
                        "Listing limited to 200 agents. Type a name if it is absent."
                    }
                    AgentStatus::Ready => "Choose a native agent or let Kiro apply its default.",
                    AgentStatus::Unrecognized if picker.truncated => {
                        "Scoped profile scan limited. Refine the query or type an exact agent name."
                    }
                    AgentStatus::Unrecognized => {
                        "Native listing unavailable. Scoped profiles shown when found; Kiro validates on launch."
                    }
                    AgentStatus::Failed(error) => error,
                }
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw("Type a fuzzy query · Enter creates · ↑/↓ optional"),
                    Line::raw("Backspace edits · Ctrl+R retry · Ctrl+U clear · Esc back"),
                    Line::styled(status, Style::default().fg(Color::Yellow)),
                ])
                .wrap(Wrap { trim: false }),
                parts[2],
            );
        }
        Mode::Placement { flow, selected } => {
            let (title, context) = match flow {
                PlacementFlow::Move { id, cwd } => (
                    "Move session to this workspace",
                    format!("{id}\nDirectory: {cwd}"),
                ),
                PlacementFlow::Existing { id, cwd } => (
                    "Place session in the workspace",
                    format!("{id}\nDirectory: {cwd}"),
                ),
                PlacementFlow::Create { cwd, agent, .. } => (
                    "New session: placement · 3/3",
                    format!(
                        "Directory: {cwd}\nAgent: {}",
                        agent.as_deref().unwrap_or("Kiro default")
                    ),
                ),
            };
            let area = modal(frame, title);
            let parts = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(4),
            ])
            .split(area);
            frame.render_widget(Paragraph::new(context).wrap(Wrap { trim: false }), parts[0]);
            let items = PLACEMENTS
                .iter()
                .map(|(label, _)| ListItem::new(*label))
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(choice_list(items), parts[1], &mut state);
            let notice = if message.is_empty() {
                "The chosen pane receives the session; no active pane or shell is replaced."
            } else {
                message
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw("↑/↓ choose · Enter confirms · v right · h below · w window"),
                    Line::raw("Esc goes back without creating"),
                    Line::styled(notice, Style::default().fg(Color::Yellow)),
                ])
                .wrap(Wrap { trim: false }),
                parts[2],
            );
        }
        Mode::Rename { id, query } => {
            let area = modal(frame, "Name session");
            frame.render_widget(
                Paragraph::new(format!("{id}\nName: {query}\n\nEnter saves · empty name restores automatic title\nCtrl+U clears · Esc cancels\n{message}"))
                    .wrap(Wrap { trim: false }), area);
            input_cursor(frame, area, "Name: ", query, 1);
        }
        Mode::Move(id) => {
            let move_area = modal(frame, "Move session here? y/n");
            let action = if popup {
                "y chooses a destination pane"
            } else {
                "y moves here"
            };
            frame.render_widget(
                Paragraph::new(format!("{id}\nDetaches the other terminal; its dashboard or shell returns.\nThe same Kiro process keeps running.\n{action} · n/Esc cancels"))
                    .wrap(Wrap { trim: false }), move_area);
        }
        Mode::Stop(id) => {
            let area = modal(frame, "Stop session? y/n");
            frame.render_widget(
                Paragraph::new(format!(
                    "{id}\nStops only this owned session.\ny confirms · n/Esc cancels"
                ))
                .wrap(Wrap { trim: false }),
                area,
            );
        }
        _ => (),
    }
}

fn input_cursor(frame: &mut Frame, rect: Rect, prefix: &str, text: &str, line: u16) {
    if rect.width == 0 || rect.height <= line {
        return;
    }
    let column = Line::from(format!("{prefix}{text}"))
        .width()
        .min(usize::from(rect.width.saturating_sub(1))) as u16;
    frame.set_cursor_position((rect.x + column, rect.y + line));
}

fn choice_list(items: Vec<ListItem<'_>>) -> List<'_> {
    List::new(items)
        .highlight_symbol("› ")
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
}

fn modal(frame: &mut Frame, title: &str) -> Rect {
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(104);
    let height = area.height.saturating_sub(2).min(24);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::picker::{AgentPicker, DirPicker};
    use crate::protocol::SessionInfo;
    use ratatui::{backend::TestBackend, Terminal};

    fn render(board: &Board, mode: &Mode, width: u16, height: u16, popup: bool) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, board, mode, "", popup))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .replace(env!("CARGO_PKG_VERSION"), "<version>")
    }

    fn directory() -> DirPicker {
        DirPicker {
            base: "/work".into(),
            query: String::new(),
            candidates: vec![super::super::picker::Candidate {
                path: "/work/project".into(),
                origin: super::super::picker::Origin::Recent,
            }],
            selected: 0,
            truncated: false,
            notice: None,
            recents: Vec::new(),
            discovery: None,
            indexed: Vec::new(),
            scanning: false,
            scan_limited: false,
        }
    }

    #[test]
    fn board_rendering() {
        let mut board = Board::default();
        insta::assert_snapshot!("empty", render(&board, &Mode::Browse, 100, 20, false));
        board.replace_rows(vec![SessionInfo {
            name: None,
            title: None,
            title_source: None,
            id: "session-one".into(),
            cwd: "/work/project".into(),
            agent: Some("reviewer".into()),
            generation: "v2".into(),
            pid: Some(42),
            state: "live".into(),
            exit_code: None,
            notice: None,
        }]);
        insta::assert_snapshot!("live", render(&board, &Mode::Browse, 100, 20, false));
        board.rows[0].state = "exited".into();
        board.rows[0].exit_code = Some(0);
        board.toggle_filter();
        insta::assert_snapshot!("history", render(&board, &Mode::Browse, 100, 20, false));
        insta::assert_snapshot!(
            "create",
            render(&board, &Mode::Directory(directory()), 100, 20, false)
        );
        let picker = AgentPicker {
            cwd: "/work/project".into(),
            query: String::new(),
            names: vec!["reviewer".into()],
            options: vec![None, Some("reviewer".into())],
            selected: 1,
            status: AgentStatus::Ready,
            token: 1,
            truncated: false,
        };
        insta::assert_snapshot!(
            "agents",
            render(
                &board,
                &Mode::Agent {
                    directory: directory(),
                    picker
                },
                100,
                20,
                false
            )
        );
        insta::assert_snapshot!(
            "move",
            render(&board, &Mode::Move("session-one".into()), 100, 20, false)
        );
        insta::assert_snapshot!(
            "stop",
            render(&board, &Mode::Stop("session-one".into()), 100, 20, false)
        );
    }

    #[test]
    fn floating_dashboard_and_placement_picker_render() {
        let mut board = Board::default();
        board.replace_rows(vec![SessionInfo {
            name: None,
            title: None,
            title_source: None,
            id: "session-one".into(),
            cwd: "/work/project".into(),
            agent: Some("reviewer".into()),
            generation: "v2".into(),
            pid: Some(42),
            state: "live".into(),
            exit_code: None,
            notice: None,
        }]);
        insta::assert_snapshot!("floating", render(&board, &Mode::Browse, 100, 20, true));
        let create = Mode::Placement {
            flow: PlacementFlow::Create {
                cwd: "/work/project".into(),
                agent: Some("reviewer".into()),
                directory: directory(),
                picker: Box::new(AgentPicker {
                    cwd: "/work/project".into(),
                    query: String::new(),
                    names: vec!["reviewer".into()],
                    options: vec![None, Some("reviewer".into())],
                    selected: 1,
                    status: AgentStatus::Ready,
                    token: 1,
                    truncated: false,
                }),
            },
            selected: 0,
        };
        insta::assert_snapshot!("placement-create", render(&board, &create, 100, 20, true));
        let existing = Mode::Placement {
            flow: PlacementFlow::Existing {
                id: "session-one".into(),
                cwd: "/work/project".into(),
            },
            selected: 2,
        };
        insta::assert_snapshot!(
            "placement-existing",
            render(&board, &existing, 100, 20, true)
        );
        let small = Mode::Placement {
            flow: PlacementFlow::Existing {
                id: "session-one".into(),
                cwd: "/work/project".into(),
            },
            selected: 0,
        };
        assert!(render(&board, &small, 60, 15, true).contains("Split right"));
    }

    #[test]
    fn tiny_terminal_and_scrolling_picker_render_without_panic() {
        let board = Board::default();
        assert!(render(&board, &Mode::Browse, 20, 5, false).contains("Kiro sessions"));
        let mut picker = directory();
        picker.candidates = (0..40)
            .map(|n| super::super::picker::Candidate {
                path: format!("/work/project-{n:02}"),
                origin: super::super::picker::Origin::Directory,
            })
            .collect();
        picker.selected = 39;
        assert!(render(&board, &Mode::Directory(picker), 70, 15, false).contains("project-39"));
    }
}
