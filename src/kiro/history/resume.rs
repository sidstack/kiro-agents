//! Generation-safe resume decisions for native conversations.
//!
//! The wrapper only ever launches the engine generation it has verified for a
//! store, and it never takes over a Kiro process it does not own. An unknown
//! lock or store stays blocked with the reason shown to the user.

use super::{is_resumable_native_id, Conversation, LockState, StoreKind};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Resumable,
    Blocked(String),
}

impl Decision {
    pub fn resumable(&self) -> bool {
        matches!(self, Decision::Resumable)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Decision::Resumable => None,
            Decision::Blocked(reason) => Some(reason),
        }
    }
}

/// Whether a conversation may be resumed by this wrapper, and why not when it
/// may not. Nothing here guesses at a store's meaning.
pub fn decide(conversation: &Conversation) -> Decision {
    if !conversation.in_store(StoreKind::Files) {
        return Decision::Blocked(
            "Stored only in Kiro's newer v2 session store. This wrapper launches the V2 agent engine and never resumes across a generation; open it with `kiro-cli chat --sessions` or `/chat resume` inside Kiro."
                .into(),
        );
    }
    if !is_resumable_native_id(&conversation.native_id) {
        return Decision::Blocked(
            "Kiro's store did not state a resume-capable session id for this row; nothing is invented."
                .into(),
        );
    }
    match conversation.lock {
        LockState::Open { pid } => {
            return Decision::Blocked(format!(
                "Kiro already has this conversation open (pid {pid}). Use it there or close it first; a foreign process is never taken over."
            ))
        }
        LockState::Unknown => {
            return Decision::Blocked(
                "Kiro's lock for this conversation could not be read; resuming could duplicate a live session."
                    .into(),
            )
        }
        LockState::Free | LockState::Stale { .. } => (),
    }
    let Some(cwd) = conversation.cwd.as_deref() else {
        return Decision::Blocked(
            "Kiro did not record a project directory for this conversation; choose a directory and start a fresh session instead."
                .into(),
        );
    };
    if !Path::new(cwd).is_dir() {
        return Decision::Blocked(format!(
            "Project directory {cwd} is no longer available; restore it or start fresh elsewhere."
        ));
    }
    Decision::Resumable
}

/// Extra badge text for a resumable row that carries an unusual state.
pub fn notice(conversation: &Conversation) -> Option<String> {
    match conversation.lock {
        LockState::Stale { pid } => Some(format!(
            "Kiro left a stale lock (pid {pid}) here; Kiro itself decides how to reopen it."
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{TextRole, TextRun};

    fn conversation(stores: Vec<StoreKind>, lock: LockState, cwd: Option<&str>) -> Conversation {
        Conversation {
            native_id: "11111111-1111-4111-8111-111111111111".into(),
            stores,
            title: Some("Fix the picker".into()),
            cwd: cwd.map(str::to_owned),
            agent: None,
            updated_at_ms: Some(1),
            lock,
            text: vec![TextRun {
                role: TextRole::Prompt,
                text: "picker search ordering".into(),
            }],
            transcript: true,
            unusable: None,
        }
    }

    #[test]
    fn a_file_store_conversation_with_a_free_lock_resumes() {
        let record = conversation(vec![StoreKind::Files], LockState::Free, Some("/tmp"));
        assert_eq!(decide(&record), Decision::Resumable);
        assert_eq!(notice(&record), None);
    }

    #[test]
    fn a_stale_lock_resumes_with_an_explanation() {
        let record = conversation(
            vec![StoreKind::Files],
            LockState::Stale { pid: 42 },
            Some("/tmp"),
        );
        assert_eq!(decide(&record), Decision::Resumable);
        assert!(notice(&record).unwrap().contains("stale lock"));
    }

    #[test]
    fn an_open_foreign_lock_blocks_and_names_the_process() {
        let record = conversation(
            vec![StoreKind::Files],
            LockState::Open { pid: 7 },
            Some("/tmp"),
        );
        let reason = decide(&record).reason().unwrap().to_owned();
        assert!(reason.contains("pid 7"), "{reason}");
        assert!(reason.contains("never taken over"), "{reason}");
    }

    #[test]
    fn an_unreadable_lock_blocks_instead_of_assuming_freedom() {
        let record = conversation(vec![StoreKind::Files], LockState::Unknown, Some("/tmp"));
        assert!(decide(&record)
            .reason()
            .unwrap()
            .contains("could not be read"));
    }

    #[test]
    fn a_newer_generation_store_is_never_resumed() {
        let record = conversation(vec![StoreKind::Workspaces], LockState::Free, Some("/tmp"));
        let reason = decide(&record).reason().unwrap().to_owned();
        assert!(
            reason.contains("never resumes across a generation"),
            "{reason}"
        );
        // Even a record present in both stores stays resumable on its v1 side.
        let both = conversation(
            vec![StoreKind::Files, StoreKind::Workspaces],
            LockState::Free,
            Some("/tmp"),
        );
        assert_eq!(decide(&both), Decision::Resumable);
    }

    #[test]
    fn missing_or_gone_project_directories_are_explained() {
        let record = conversation(vec![StoreKind::Files], LockState::Free, None);
        assert!(decide(&record)
            .reason()
            .unwrap()
            .contains("project directory"));
        let record = conversation(
            vec![StoreKind::Files],
            LockState::Free,
            Some("/definitely/not/here"),
        );
        assert!(decide(&record)
            .reason()
            .unwrap()
            .contains("no longer available"));
    }
}
