use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kiro_agents::{
    attach, board, host, kiro,
    protocol::{Action, Reply, SessionInfo},
    state_dir,
};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    version,
    about = "Persistent native Kiro V2 sessions with a selection-first session board"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open optional tmux workspaces (requires tmux; sessions remain host-owned)
    Workspace {
        #[command(subcommand)]
        command: Option<WorkspaceCommand>,
    },
    /// Open the session board (plain list when piped)
    Board,
    /// Start a fresh V2 session and attach (Ctrl+Q detaches)
    New {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        agent: Option<String>,
        /// Display name stored only in wrapper metadata
        #[arg(long)]
        name: Option<String>,
        /// Explicit V2 selection (V2 is the only supported generation)
        #[arg(long)]
        v2: bool,
        #[arg(long)]
        no_attach: bool,
    },
    /// Reconnect to the same live process
    Attach { id: String },
    /// List metadata; no transcript is stored
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Rename wrapper metadata, or clear the override to use an automatic title
    Rename {
        id: String,
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        clear: bool,
    },
    /// Stop one owned session
    Stop { id: String },
    /// Stop all owned sessions and the host
    Shutdown,
    /// Let Kiro list its native profiles for a directory
    Agents {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Verify Kiro's V2/TUI flags without making a model call
    Doctor,
    #[command(name = "__host", hide = true)]
    Host,
    #[command(name = "__workspace-board", hide = true)]
    WorkspaceBoard,
    #[command(name = "__workspace-session", hide = true)]
    WorkspaceSession { id: String },
    #[command(name = "__workspace-home", hide = true)]
    WorkspaceHome,
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    /// Open the centered session dashboard in this workspace
    Dashboard,
    /// Open/create a named workspace; each Kiro pane starts at the session board
    Open {
        #[arg(default_value = "main")]
        name: String,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        detach: bool,
    },
    /// Split the current owned workspace pane; --terminal opens a shell
    Split {
        #[arg(long, conflicts_with = "vertical")]
        horizontal: bool,
        #[arg(long)]
        vertical: bool,
        #[arg(long)]
        terminal: bool,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Create a window containing a session picker or a terminal
    Window {
        #[arg(long)]
        terminal: bool,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// List this application's tmux workspaces
    List,
    /// Close a workspace and its panes; managed Kiro sessions keep running
    Close { name: String },
}

fn workspace(command: Option<WorkspaceCommand>) -> Result<()> {
    let feature = kiro_agents::workspace_feature()?;
    match command.unwrap_or(WorkspaceCommand::Open {
        name: "main".into(),
        dir: PathBuf::from("."),
        detach: false,
    }) {
        WorkspaceCommand::Dashboard => feature.dashboard(),
        WorkspaceCommand::Open { name, dir, detach } => feature.open(&name, &dir, detach),
        WorkspaceCommand::Split {
            vertical,
            terminal,
            dir,
            ..
        } => {
            let direction = if vertical {
                kiro_agents::features::SplitDirection::Vertical
            } else {
                kiro_agents::features::SplitDirection::Horizontal
            };
            feature.split(direction, terminal, &dir)
        }
        WorkspaceCommand::Window { terminal, dir } => feature.window(terminal, &dir),
        WorkspaceCommand::List => {
            write!(io::stdout(), "{}", feature.list()?)?;
            Ok(())
        }
        WorkspaceCommand::Close { name } => feature.close(&name),
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();
    let cli = Cli::parse();
    let dir = state_dir()?;
    match cli.command.unwrap_or(Command::Board) {
        Command::Host => host::run(dir),
        Command::WorkspaceBoard => {
            let feature = kiro_agents::workspace_feature()?;
            board::run_popup(&dir, feature.as_ref())
        }
        Command::WorkspaceSession { id } => attach::run(&dir, id),
        Command::WorkspaceHome => {
            anyhow::ensure!(
                kiro_agents::workspace_context()?.is_some(),
                "Open this launcher through a workspace"
            );
            writeln!(
                io::stdout(),
                "Kiro workspace · Ctrl+Q opens the session dashboard"
            )?;
            writeln!(
                io::stdout(),
                "Choose a session to fill this empty launcher. Ctrl+B T opens a shell."
            )?;
            loop {
                std::thread::park();
            }
        }
        Command::Workspace { command } => workspace(command),
        Command::Doctor => kiro::doctor(),
        Command::Agents { dir } => kiro::agents(&dir.canonicalize()?),
        Command::New {
            dir: cwd,
            agent,
            name,
            no_attach,
            ..
        } => {
            let cwd = cwd
                .canonicalize()
                .context("Project directory does not exist")?;
            let cwd = cwd
                .to_str()
                .context("Project path must be UTF-8")?
                .to_owned();
            let name = name
                .map(|name| kiro_agents::features::validate_name(&name))
                .transpose()?;
            let reply = host::call(&dir, Action::New { cwd, agent }, true)?;
            if let Reply::Created(session) = reply {
                if let Some(name) = name {
                    kiro_agents::custom_names::set(&dir, &session.id, Some(name)).with_context(
                        || {
                            format!(
                                "Session {} was created, but saving its name failed",
                                session.id
                            )
                        },
                    )?;
                }
                if !no_attach && io::stdin().is_terminal() && io::stdout().is_terminal() {
                    attach::run(&dir, session.id.clone()).with_context(|| {
                        format!(
                            "Session {} was created; reconnect with kiro-agents attach {}",
                            session.id, session.id
                        )
                    })?;
                } else {
                    writeln!(
                        io::stdout(),
                        "Created {} · v2 · {}",
                        session.id,
                        session.cwd
                    )?;
                    writeln!(io::stdout(), "Attach: kiro-agents attach {}", session.id)?;
                }
            } else {
                anyhow::bail!("Unexpected host response");
            }
            Ok(())
        }
        Command::Attach { id } => attach::run(&dir, id),
        Command::Status { json } => {
            let rows = sessions(&dir)?;
            if json {
                writeln!(io::stdout(), "{}", serde_json::to_string_pretty(&rows)?)?;
            } else {
                display(&rows)?;
            }
            Ok(())
        }
        Command::Rename { id, name, clear } => {
            anyhow::ensure!(name.is_some() || clear, "Supply a name or --clear");
            let name = name
                .map(|name| kiro_agents::features::validate_name(&name))
                .transpose()?;
            anyhow::ensure!(
                sessions(&dir)?.iter().any(|row| row.id == id),
                "Unknown session ID"
            );
            kiro_agents::custom_names::set(&dir, &id, name)?;
            Ok(())
        }
        Command::Stop { id } => {
            host::call(&dir, Action::Stop { id }, false)?;
            Ok(())
        }
        Command::Shutdown => {
            host::call(&dir, Action::Shutdown, false)?;
            Ok(())
        }
        Command::Board if io::stdin().is_terminal() && io::stdout().is_terminal() => {
            if let Some(workspace) = kiro_agents::workspace_context()? {
                workspace.dashboard()
            } else {
                board::run(&dir)
            }
        }
        Command::Board => display(&sessions(&dir)?),
    }
}

fn sessions(dir: &Path) -> Result<Vec<SessionInfo>> {
    // A fresh install needs no daemon just to show an empty list. Saved state
    // starts a host if necessary so crash recovery also works from the board.
    if !dir.join("host.sock").exists() && !dir.join("registry.json").exists() {
        return Ok(Vec::new());
    }
    match host::call(dir, Action::Status, true)? {
        Reply::Sessions(mut rows) => {
            kiro_agents::custom_names::enrich(dir, &mut rows)?;
            Ok(rows)
        }
        _ => anyhow::bail!("Unexpected host response"),
    }
}

fn display(rows: &[SessionInfo]) -> Result<()> {
    let mut output = io::stdout().lock();
    writeln!(
        output,
        "Kiro V2 sessions · Ctrl+Q detaches · quitting leaves sessions alive"
    )?;
    if rows.is_empty() {
        writeln!(
            output,
            "No sessions. Start one: kiro-agents new <directory>"
        )?;
    }
    for session in rows {
        writeln!(
            output,
            "{}  {:<8}  {}  ·  v2  {}  {}",
            session.id,
            session.state,
            session.display_name(),
            session.agent.as_deref().unwrap_or("(Kiro default)"),
            session.cwd
        )?;
        if let Some(notice) = &session.notice {
            writeln!(output, "  {notice}")?;
        }
    }
    Ok(())
}
