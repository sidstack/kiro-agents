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
    Rename { id: String, name: Option<String> },
    Shutdown,
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Reply {
    Sessions(Vec<SessionInfo>),
    Created(Box<SessionInfo>),
    Ok,
    Output(Vec<u8>),
    End,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    #[serde(
        default,
        skip_serializing_if = "crate::lifecycle::Activity::is_unknown"
    )]
    pub activity: crate::lifecycle::Activity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<String>,
    pub id: String,
    pub cwd: String,
    pub agent: Option<String>,
    pub generation: String,
    pub pid: Option<u32>,
    pub state: String,
    pub exit_code: Option<u32>,
    pub notice: Option<String>,
}

impl SessionInfo {
    pub fn display_name(&self) -> String {
        self.name
            .as_deref()
            .or(self.title.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let project = std::path::Path::new(&self.cwd)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&self.cwd);
                format!(
                    "{} · {} · #{}",
                    project,
                    self.agent.as_deref().unwrap_or("Kiro default"),
                    self.id
                )
            })
    }
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

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn old_host_without_activity_is_unknown_and_future_activity_is_not_ready() {
        let old = serde_json::json!({
            "id": "old", "cwd": "/tmp", "agent": null, "generation": "v2",
            "pid": 42, "state": "live", "exit_code": null, "notice": null
        });
        let row: SessionInfo = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(row.activity, crate::lifecycle::Activity::Unknown);
        let mut future = old;
        future["activity"] = "unrecognized".into();
        let row: SessionInfo = serde_json::from_value(future).unwrap();
        assert_eq!(row.activity, crate::lifecycle::Activity::Unknown);
    }
}
