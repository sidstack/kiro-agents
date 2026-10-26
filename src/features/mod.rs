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
}
