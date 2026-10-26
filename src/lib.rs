pub mod attach;
pub mod board;
pub mod host;
pub mod kiro;
pub mod protocol;

use std::{env, path::PathBuf};

pub fn state_dir() -> anyhow::Result<PathBuf> {
    let path = match env::var_os("KIRO_AGENTS_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is unset"))?)
            .join(".kiro-agents"),
    };
    Ok(std::path::absolute(path)?)
}
