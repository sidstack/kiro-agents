use crate::{
    host,
    protocol::{self, Action, Reply},
};
use anyhow::{Context, Result};
use std::{
    io::{self, BufReader, IsTerminal, Read, Write},
    os::fd::{AsRawFd, RawFd},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub fn readable(fd: &impl AsRawFd, timeout_ms: i32) -> io::Result<bool> {
    poll_fd(fd.as_raw_fd(), timeout_ms)
}

fn poll_fd(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor is a valid initialized pollfd for the duration of poll.
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(result > 0)
}

// Polls both directions in one syscall so typed input wakes the loop
// immediately instead of waiting out the host-socket timeout first.
fn poll_pair(first: RawFd, second: RawFd, timeout_ms: i32) -> io::Result<(bool, bool)> {
    let mut descriptors = [
        libc::pollfd {
            fd: first,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: second,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: descriptors is a valid initialized array for the duration of poll.
    let result = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as u64,
            timeout_ms,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok((false, false));
        }
        return Err(error);
    }
    let ready = |descriptor: &libc::pollfd| {
        descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
    };
    Ok((ready(&descriptors[0]), ready(&descriptors[1])))
}

#[derive(Default)]
pub struct Leader {}

impl Leader {
    // Ctrl+Q detaches without conflicting with tmux's Ctrl+B prefix.
    pub fn filter(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        if let Some(index) = bytes.iter().position(|byte| *byte == 17) {
            (bytes[..index].to_vec(), true)
        } else {
            (bytes.to_vec(), false)
        }
    }
}

// How the terminal is left when attach ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cleanup {
    // Standalone attach: hand a cooked, primary-screen shell back to the caller.
    Standalone,
    // Board handoff: the board keeps raw mode; native output owns its screen;
    // cleanup may only undo the child's private modes, never the screen or
    // input mode the board is still using.
    Board,
}

// Bytes that reset child-owned private modes on the way back to the board.
// No DECRST 1049 (no shell flash), no raw-mode disable, no cursor
// repositioning and no newline: the board redraws after re-entering its alternate screen.
// A natively exited child may have left the alternate screen, so re-enter it
// (idempotent when the child is still holding it) for the board's redraw.
fn board_cleanup_bytes(child_exited: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        b"\x1b[?2026l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[0m\x1b[?25h",
    );
    if child_exited {
        bytes.extend_from_slice(b"\x1b[?1049h");
    }
    bytes
}

struct Terminal {
    child_exited: bool,
    cleanup: Cleanup,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        match self.cleanup {
            Cleanup::Board => {
                let mut output = io::stdout().lock();
                let _ = output.write_all(&board_cleanup_bytes(self.child_exited));
                let _ = output.flush();
            }
            Cleanup::Standalone => {
                let _ = crossterm::terminal::disable_raw_mode();
                let mut output = io::stdout().lock();
                // Kiro has already cleaned up its screen on normal exit. Repeating
                // DECRST 1049 would restore an old saved cursor over the transcript.
                // A still-running child cannot clean up our terminal when we detach.
                let _ = output.write_all(
                    b"\x1b[?2026l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l",
                );
                if !self.child_exited {
                    let _ = output.write_all(b"\x1b[?1049l");
                    // We do not track the child's screen. Leave room for the shell
                    // below retained content even when alternate-screen reset restores
                    // a stale cursor on an inline terminal such as Guacamole SSH.
                    if let Ok((_, rows)) = crossterm::terminal::size() {
                        let _ = write!(output, "\x1b[r\x1b[?6l\x1b[{};1H", rows.max(1));
                    }
                }
                let _ = output.write_all(b"\x1b[0m\x1b[?25h\r\n");
                let _ = output.flush();
            }
        }
    }
}

const ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn run(dir: &Path, id: String) -> Result<()> {
    run_with_interrupt(dir, id, Arc::new(AtomicBool::new(false)))
}

/// Share a caller's termination flag across terminal handoffs so a signal
/// between leaving the board and installing attach handlers cannot be lost.
/// Standalone cleanup (cooked mode, primary screen, shell cursor) runs here.
pub fn run_with_interrupt(dir: &Path, id: String, interrupted: Arc<AtomicBool>) -> Result<()> {
    handoff(
        dir,
        id,
        interrupted,
        Cleanup::Standalone,
        None::<fn() -> Result<()>>,
    )
}

/// Temporarily hand the terminal to a native session from a live board.
///
/// The host acknowledges the attach before `on_handoff` runs, so the board
/// can keep raw mode alive across the handoff: it
/// suspends rendering in the callback (e.g. one clear for native output),
/// the raw byte pump runs, then the callback returns and the board redraws.
/// Terminal cleanup only resets child private input/screen modes; it never
/// leaves the alternate screen, disables raw mode, repositions the shell
/// cursor or adds a newline. A board redraw must follow immediately.
pub fn run_from_board(
    dir: &Path,
    id: String,
    interrupted: Arc<AtomicBool>,
    on_handoff: impl FnOnce() -> Result<()>,
) -> Result<()> {
    handoff(dir, id, interrupted, Cleanup::Board, Some(on_handoff))
}

fn handoff(
    dir: &Path,
    id: String,
    interrupted: Arc<AtomicBool>,
    cleanup: Cleanup,
    on_handoff: Option<impl FnOnce() -> Result<()>>,
) -> Result<()> {
    anyhow::ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "Attach requires an interactive terminal"
    );
    let mut stream = host::connect(dir, false)?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(2)))?;
    // A stalled host must fail recoverably, not hang the board between effects.
    stream.set_read_timeout(Some(ACK_TIMEOUT))?;
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    protocol::send(
        &mut stream,
        &protocol::request(Action::Attach {
            id: id.clone(),
            cols,
            rows,
        }),
    )?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let ack = protocol::receive(&mut reader);
    // The bounded wait covers only the ACK; the byte pump must not time out
    // mid-frame while a session is legitimately quiet.
    stream.set_read_timeout(None)?;
    reader.get_mut().set_read_timeout(None)?;
    match ack {
        Ok(Reply::Ok) => (),
        Ok(Reply::Error(error)) => anyhow::bail!(error),
        Ok(_) => anyhow::bail!("Unexpected attach response"),
        Err(protocol::FrameError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            let _ = protocol::send(&mut stream, &protocol::request(Action::Detach));
            anyhow::bail!("Host did not acknowledge attach in time; sessions stay alive")
        }
        Err(error) => return Err(error.into()),
    }
    let _workspace_attachment = match crate::workspace_context() {
        Ok(Some(feature)) => match feature.register_attachment(&id) {
            Ok(registration) => registration,
            Err(_) => {
                tracing::warn!(
                    "Workspace pane association unavailable; native attachment continues"
                );
                None
            }
        },
        Ok(None) => None,
        Err(_) => {
            tracing::warn!("Workspace context unavailable; native attachment continues");
            None
        }
    };
    let mut signals = Vec::new();
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        signals.push(signal_hook::flag::register(signal, interrupted.clone())?);
    }
    let result = (|| {
        // Idempotent when the board already owns raw mode; required standalone.
        crossterm::terminal::enable_raw_mode()?;
        let mut terminal = Terminal {
            child_exited: false,
            cleanup,
        };
        if let Some(on_handoff) = on_handoff {
            // The host owns the screen from here; the board suspends rendering.
            on_handoff()?;
        }
        let mut input = io::stdin();
        let mut output = io::stdout();
        let mut leader = Leader::default();
        let mut bytes = [0u8; 4096];
        while !interrupted.load(Ordering::Relaxed) {
            // One poll covers host output and typed input, so input latency
            // is the wake-up itself rather than the tail of a host-only wait.
            let (host_ready, input_ready) = if reader.buffer().is_empty() {
                poll_pair(reader.get_ref().as_raw_fd(), input.as_raw_fd(), 20)?
            } else {
                // Drain buffered frames first but still interleave input.
                (true, readable(&input, 0)?)
            };
            if host_ready {
                match protocol::receive(&mut reader)? {
                    Reply::Output(bytes) => {
                        output.write_all(&bytes)?;
                        output.flush()?;
                    }
                    Reply::End => {
                        terminal.child_exited = true;
                        return Ok(());
                    }
                    Reply::Error(error) => anyhow::bail!(error),
                    _ => anyhow::bail!("Unexpected attached response"),
                }
            }
            if input_ready {
                let n = input.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                let (bytes, detach) = leader.filter(&bytes[..n]);
                if !bytes.is_empty() {
                    protocol::send(&mut stream, &protocol::request(Action::Input(bytes)))?;
                }
                if detach {
                    break;
                }
            }
            let dimensions =
                crossterm::terminal::size().context("Cannot read terminal dimensions")?;
            if dimensions != (cols, rows) {
                (cols, rows) = dimensions;
                protocol::send(
                    &mut stream,
                    &protocol::request(Action::Resize { cols, rows }),
                )?;
            }
        }
        let _ = protocol::send(&mut stream, &protocol::request(Action::Detach));
        Ok(())
    })();
    for signal in signals {
        signal_hook::low_level::unregister(signal);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_cleanup_resets_child_modes_without_shell_flash() {
        for child_exited in [false, true] {
            let bytes = board_cleanup_bytes(child_exited);
            let text = String::from_utf8(bytes).expect("cleanup bytes are ASCII");
            assert!(
                !text.contains("\x1b[?1049l"),
                "board handoff must never leave the alternate screen"
            );
            assert!(!text.contains('\n'), "board handoff must not add newlines");
            assert!(
                !text.contains(";1H") && !text.contains("\x1b[r"),
                "board handoff must not reposition the shell cursor"
            );
            assert!(text.contains("\x1b[?2004l"), "bracketed paste must reset");
            assert!(text.contains("\x1b[?1006l"), "mouse modes must reset");
            assert!(text.contains("\x1b[0m"), "SGR must reset");
            if child_exited {
                assert!(
                    text.contains("\x1b[?1049h"),
                    "an exited child may have left the alternate screen"
                );
            } else {
                assert!(
                    !text.contains("1049"),
                    "a detached child keeps whatever screen it holds"
                );
            }
        }
    }
}
