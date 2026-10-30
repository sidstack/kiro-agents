pub mod attach;
pub mod board;
pub mod custom_names;
pub mod features;
pub mod host;
pub mod kiro;
pub mod lifecycle;
pub mod protocol;
mod workspace;

use std::{env, path::PathBuf};

pub fn state_dir() -> anyhow::Result<PathBuf> {
    let path = match env::var_os("KIRO_AGENTS_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is unset"))?)
            .join(".kiro-agents"),
    };
    Ok(std::path::absolute(path)?)
}

/// Assemble optional features at one boundary; the host depends only on the trait.
pub fn session_features() -> anyhow::Result<Vec<Box<dyn features::SessionFeature>>> {
    naming_features(std::env::var("KIRO_AGENTS_AUTO_NAME").ok().as_deref())
}

fn naming_features(mode: Option<&str>) -> anyhow::Result<Vec<Box<dyn features::SessionFeature>>> {
    let first_message = match mode.unwrap_or("auto") {
        "off" | "false" | "0" => return Ok(Vec::new()),
        "kiro" => false,
        "auto" | "on" | "true" | "1" => true,
        _ => anyhow::bail!("KIRO_AGENTS_AUTO_NAME must be auto, kiro or off"),
    };
    let home = std::env::var_os("KIRO_HOME").or_else(|| {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".kiro").into_os_string())
    });
    let Some(home) = home else {
        return Ok(Vec::new());
    };
    let directory = std::env::var_os("KIRO_SESSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join("sessions/cli"));
    Ok(vec![Box::new(kiro::titles::NativeNames::new(
        directory,
        first_message,
    ))])
}

/// Workspace mode is opt-in through the CLI and may be disabled globally.
pub fn workspace_feature() -> anyhow::Result<Box<dyn features::WorkspaceFeature>> {
    match std::env::var("KIRO_AGENTS_WORKSPACES")
        .unwrap_or_else(|_| "on".into())
        .as_str()
    {
        "on" | "true" | "1" => (),
        "off" | "false" | "0" => {
            anyhow::bail!("Workspace feature is disabled by KIRO_AGENTS_WORKSPACES")
        }
        _ => anyhow::bail!("KIRO_AGENTS_WORKSPACES must be on or off"),
    }
    Ok(Box::new(workspace::TmuxWorkspace::new(
        state_dir()?,
        std::env::current_exe()?,
    )?))
}

/// Optional owned-workspace context. Ordinary and disabled attaches remain native.
pub fn workspace_context() -> anyhow::Result<Option<Box<dyn features::WorkspaceFeature>>> {
    if std::env::var_os("KIRO_AGENTS_WORKSPACE_SOCKET").is_none()
        || matches!(
            std::env::var("KIRO_AGENTS_WORKSPACES").ok().as_deref(),
            Some("off" | "false" | "0")
        )
    {
        return Ok(None);
    }
    workspace_feature().map(Some)
}

#[cfg(test)]
mod feature_tests {
    #[test]
    fn automatic_naming_can_be_disabled_and_unknown_modes_are_rejected() {
        assert!(super::naming_features(Some("off")).unwrap().is_empty());
        assert_eq!(super::naming_features(Some("kiro")).unwrap().len(), 1);
        assert_eq!(super::naming_features(Some("auto")).unwrap().len(), 1);
        assert!(super::naming_features(Some("guess")).is_err());
    }
}
