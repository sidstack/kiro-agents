use crate::protocol::SessionInfo;

pub struct SessionTitle {
    pub text: String,
    pub source: &'static str,
}

/// Optional read-only enrichment; implementations hold no live child handles.
pub trait SessionFeature: Send + Sync {
    fn title(&self, session: &SessionInfo) -> Option<SessionTitle>;
}

pub fn clean_title(text: &str) -> Option<String> {
    // Generated labels must not turn pasted credentials/auth links into registry metadata.
    if text.contains("://")
        || text.contains("token=")
        || text.split_whitespace().any(|word| {
            ["sk-", "ghp_", "github_pat_", "AKIA", "Bearer"]
                .iter()
                .any(|prefix| word.starts_with(prefix))
        })
    {
        return None;
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect();
    (!text.is_empty()).then_some(text)
}

pub fn validate_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty(),
        "Session name cannot be empty; clear it explicitly"
    );
    anyhow::ensure!(
        name.chars().count() <= 80,
        "Session name must be at most 80 characters"
    );
    anyhow::ensure!(
        !name.chars().any(char::is_control),
        "Session name cannot contain control characters"
    );
    Ok(name.to_owned())
}

/// Optional external terminal workspace; the session host never owns tmux or shell panes.
#[derive(Debug, Clone, Copy)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanePlacement {
    Right,
    Below,
    Window,
}

/// One step through the deterministic live session pane order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneStep {
    Next,
    Previous,
}

pub fn pane_step(value: &str) -> anyhow::Result<PaneStep> {
    match value {
        "next" => Ok(PaneStep::Next),
        "previous" => Ok(PaneStep::Previous),
        other => anyhow::bail!("Workspace navigation step must be next or previous ({other:?})"),
    }
}

/// Shared parsing for optional feature toggles; unknown values are refused.
pub fn toggle(name: &str, value: Option<&str>) -> anyhow::Result<bool> {
    match value.unwrap_or("on") {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        _ => anyhow::bail!("{name} must be on or off"),
    }
}

/// Dropping a registration removes only this frontend's pane association.
pub trait WorkspaceAttachment {}

pub trait WorkspaceFeature: Send + Sync {
    fn reload(&self) -> anyhow::Result<()>;
    fn dashboard(&self) -> anyhow::Result<()>;
    fn focus_session(&self, id: &str, zoom: bool) -> anyhow::Result<bool>;
    fn open_session(
        &self,
        id: &str,
        cwd: &std::path::Path,
        placement: PanePlacement,
    ) -> anyhow::Result<()>;
    fn move_session(
        &self,
        id: &str,
        cwd: &std::path::Path,
        placement: PanePlacement,
    ) -> anyhow::Result<()>;
    /// Focus the next or previous valid live session pane. False means the
    /// client already holds the only reachable pane and nothing was changed.
    fn step_live_session(&self, step: PaneStep) -> anyhow::Result<bool>;
    fn register_attachment(&self, id: &str)
        -> anyhow::Result<Option<Box<dyn WorkspaceAttachment>>>;
    fn open(&self, name: &str, cwd: &std::path::Path, detached: bool) -> anyhow::Result<()>;
    fn split(
        &self,
        direction: SplitDirection,
        terminal: bool,
        cwd: &std::path::Path,
    ) -> anyhow::Result<()>;
    fn window(&self, terminal: bool, cwd: &std::path::Path) -> anyhow::Result<()>;
    fn list(&self) -> anyhow::Result<String>;
    fn close(&self, name: &str) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_are_short_single_line_terminal_safe_metadata() {
        assert_eq!(
            super::validate_name("  Fix sessions  ").unwrap(),
            "Fix sessions"
        );
        for bad in ["", " ", "line\nbreak", "\x1b[31mred"] {
            assert!(super::validate_name(bad).is_err());
        }
        assert!(super::validate_name(&"x".repeat(81)).is_err());
        assert!(super::clean_title("Open https://login.invalid/?token=example").is_none());
        assert!(super::clean_title("Use ghp_examplecredential").is_none());
        assert_eq!(
            super::clean_title("Ünicode\n title\tworks").unwrap(),
            "Ünicode title works"
        );
    }

    #[test]
    fn navigation_steps_and_toggles_are_explicit() {
        assert_eq!(super::pane_step("next").unwrap(), super::PaneStep::Next);
        assert_eq!(
            super::pane_step("previous").unwrap(),
            super::PaneStep::Previous
        );
        for bad in ["", "nextt", "next,previous", "up"] {
            assert!(super::pane_step(bad).is_err(), "{bad:?}");
        }
        assert!(super::toggle("KIRO_AGENTS_WORKSPACE_NAV", None).unwrap());
        assert!(super::toggle("KIRO_AGENTS_WORKSPACE_NAV", Some("on")).unwrap());
        assert!(!super::toggle("KIRO_AGENTS_WORKSPACE_NAV", Some("off")).unwrap());
        assert!(!super::toggle("KIRO_AGENTS_WORKSPACE_NAV", Some("0")).unwrap());
        assert!(super::toggle("KIRO_AGENTS_WORKSPACE_NAV", Some("maybe")).is_err());
    }
}
