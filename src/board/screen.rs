use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Write};

/// Own the frontend terminal for the entire board/native/board interaction.
/// Native attachment borrows the display, without restoring the shell between views.
pub(super) struct Screen {
    pub terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl Screen {
    pub fn open() -> Result<Self> {
        enable_raw_mode()?;
        let result = (|| {
            let mut output = io::stdout();
            execute!(output, EnterAlternateScreen, Hide)?;
            let terminal = Terminal::new(CrosstermBackend::new(output))?;
            let mut screen = Self { terminal };
            screen.redraw()?;
            Ok(screen)
        })();
        if result.is_err() {
            restore();
        }
        result
    }

    pub fn redraw(&mut self) -> Result<()> {
        // Terminal::clear() queries cursor position in Ratatui 0.30, which
        // bare PTYs and some SSH paths do not answer. Resize invalidates the
        // buffers without requiring a cursor reply.
        let area = self.terminal.size()?.into();
        self.terminal.resize(area)?;
        Ok(())
    }

    /// Called only after the host accepts attachment. Raw mode and the
    /// alternate screen remain owned here while native bytes are forwarded.
    pub fn prepare_native(&mut self) -> Result<()> {
        let mut output = io::stdout();
        execute!(output, Clear(ClearType::All), MoveTo(0, 0), Show)?;
        output.flush()?;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut output = io::stdout();
        // Native exit may have restored the primary screen; re-entry is
        // idempotent. Reset margins and origin before absolute board drawing.
        execute!(output, EnterAlternateScreen)?;
        output.write_all(b"\x1b[r\x1b[?6l\x1b[0m")?;
        execute!(output, Hide, Clear(ClearType::All), MoveTo(0, 0))?;
        output.flush()?;
        self.redraw()
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let mut output = io::stdout();
    let _ = execute!(output, LeaveAlternateScreen, Show);
    let _ = output.flush();
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}
