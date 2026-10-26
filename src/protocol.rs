use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{BufRead, Write};

pub const VERSION: u8 = 1;
const MAX_FRAME: usize = 128 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub version: u8,
    pub action: Action,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Action {
    Status,
    New { cwd: String, agent: Option<String> },
    Attach { id: String, cols: u16, rows: u16 },
    Stop { id: String },
    Shutdown,
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reply {
    Sessions(Vec<SessionInfo>),
    Created(SessionInfo),
    Ok,
    Output(Vec<u8>),
    End,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub cwd: String,
    pub agent: Option<String>,
    pub generation: String,
    pub pid: Option<u32>,
    pub state: String,
    pub exit_code: Option<u32>,
    pub notice: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("IPC frame exceeds limit")]
    TooLarge,
    #[error("IPC connection closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub fn send(writer: &mut impl Write, value: &impl Serialize) -> Result<(), FrameError> {
    let mut bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(FrameError::TooLarge);
    }
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    Ok(())
}

pub fn receive<T: DeserializeOwned>(reader: &mut impl BufRead) -> Result<T, FrameError> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(FrameError::Closed);
        }
        let count = available.iter().position(|&b| b == b'\n').map(|n| n + 1);
        let n = count.unwrap_or(available.len());
        if frame.len() + n > MAX_FRAME {
            return Err(FrameError::TooLarge);
        }
        frame.extend_from_slice(&available[..n]);
        reader.consume(n);
        if count.is_some() {
            return Ok(serde_json::from_slice(&frame)?);
        }
    }
}

pub fn request(action: Action) -> Request {
    Request {
        version: VERSION,
        action,
    }
}
