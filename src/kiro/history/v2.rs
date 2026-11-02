//! Read-only adapter for Kiro's newer per-workspace session store.
//!
//! Observed shape (docs/FINDINGS.md): `sessions/<workspace>/<session-dir>/`
//! with a `session.json` carrying `schemaVersion`, `dataModelVersion`, `id`
//! (`sess_<uuid>` or `cli_<uuid>[_<rand>]`), `title`, `agentMode`,
//! `workspacePaths`, `rootPaths`, `createdAt` and `lastModifiedAt`. Only the
//! metadata exists in this layout: no transcript file was found next to it, and
//! the observed session lock convention belongs to the other store. So this
//! adapter can list a conversation but must not claim its text was searched or
//! that its lock is free.
//!
//! Some imported rows also carry `messages.jsonl` as `{id, payload, timestamp}`
//! envelopes. That payload shape is not verified here, so the adapter does not read
//! it: such a row reports no conversation text rather than pretending to have
//! indexed it.

use super::{
    native_uuid, read_json_bounded, timestamp_ms, Budget, Conversation, LockState, Scan,
    StoreAdapter, StoreKind, MAX_METADATA_BYTES,
};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};

/// Directory prefixes Kiro uses for its own session directories.
const SESSION_PREFIXES: [&str; 2] = ["sess_", "cli_"];
/// Value accepted for `schemaVersion` in the inspected store.
const KNOWN_SCHEMA: &str = "1.0.0";

pub struct WorkspaceStore {
    root: PathBuf,
}

impl WorkspaceStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl StoreAdapter for WorkspaceStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Workspaces
    }

    fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn scan(&self, budget: &mut Budget, cancelled: &AtomicBool, into: &mut Scan) {
        let workspaces = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(_) => {
                into.note(true);
                return;
            }
        };
        let mut directories: Vec<PathBuf> = Vec::new();
        for entry in workspaces.flatten() {
            if budget.stopped(cancelled) {
                into.partial = true;
                return;
            }
            if !budget.take_entry(into) {
                // Out of entry budget: process what was collected, don't drop it.
                break;
            }
            let path = entry.path();
            // Hidden entries (`.index`) and symlinks are never walked.
            if !is_plain_directory(&path) {
                continue;
            }
            directories.push(path);
        }
        directories.sort();
        for workspace in directories {
            let Ok(entries) = fs::read_dir(&workspace) else {
                into.note(true);
                continue;
            };
            // Session directories are bounded during enumeration too, so a
            // workspace holding a huge directory listing cannot outrun the
            // entry budget, the cancellation flag or the scan deadline.
            let mut sessions: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                if budget.stopped(cancelled) {
                    into.partial = true;
                    return;
                }
                let path = entry.path();
                let shaped = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        SESSION_PREFIXES
                            .iter()
                            .any(|prefix| name.starts_with(prefix))
                    });
                if !shaped {
                    continue;
                }
                if !budget.take_entry(into) {
                    // Out of entry budget: process what was collected.
                    break;
                }
                sessions.push(path);
            }
            sessions.sort();
            for session in sessions {
                if budget.stopped(cancelled) {
                    into.partial = true;
                    return;
                }
                if !is_plain_directory(&session) {
                    continue;
                }
                match record(&session, budget, into) {
                    Some(conversation) => into.conversations.push(conversation),
                    None => into.unrecognised += 1,
                }
            }
        }
    }
}

fn is_plain_directory(path: &std::path::Path) -> bool {
    fs::symlink_metadata(path)
        .map(|info| info.is_dir())
        .unwrap_or(false)
}

fn record(session: &std::path::Path, budget: &mut Budget, into: &mut Scan) -> Option<Conversation> {
    let metadata = read_json_bounded(&session.join("session.json"), MAX_METADATA_BYTES)?;
    if metadata.get("schemaVersion").and_then(Value::as_str) != Some(KNOWN_SCHEMA) {
        // An unknown schema gets no guessed metadata.
        return None;
    }
    // The store states its own identity; the directory name has to agree with
    // it, so an unrelated directory is never read as a conversation.
    let native_id = metadata.get("id").and_then(Value::as_str)?;
    let directory_name = session.file_name()?.to_str()?;
    if !directory_name.starts_with(native_id) {
        return None;
    }
    let uuid = native_uuid(native_id)?;
    if !budget.take_conversation(into) {
        return None;
    }
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .and_then(crate::features::clean_title)
        .filter(|title| !title.is_empty());
    let cwd = ["rootPaths", "workspacePaths"]
        .iter()
        .find_map(|key| {
            metadata
                .get(*key)
                .and_then(Value::as_array)
                .and_then(|paths| paths.first())
                .and_then(Value::as_str)
        })
        .filter(|path| !path.is_empty() && !path.chars().any(char::is_control))
        .map(str::to_owned);
    let updated_at_ms = metadata
        .get("lastModifiedAt")
        .and_then(Value::as_str)
        .and_then(timestamp_ms)
        .or_else(|| {
            metadata
                .get("createdAt")
                .and_then(Value::as_str)
                .and_then(timestamp_ms)
        })
        .or_else(|| {
            fs::metadata(session.join("session.json"))
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|age| age.as_millis() as u64)
        });
    Some(Conversation {
        // The row keeps the UUID so a conversation that also lives in the
        // file store merges into one result.
        native_id: uuid,
        stores: vec![StoreKind::Workspaces],
        title,
        cwd,
        // `agentMode` is a mode, not a profile name: never presented as one.
        agent: None,
        updated_at_ms,
        lock: LockState::Unknown,
        text: Vec::new(),
        transcript: false,
        unusable: Some("conversation text is not in this store layout".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{self, Scan, StoreRoots};
    use std::fs;

    const UUID: &str = "11111111-1111-4111-8111-111111111111";

    fn write_session(root: &std::path::Path, workspace: &str, directory: &str, body: &str) {
        let session = root.join(workspace).join(directory);
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("session.json"), body).unwrap();
    }

    fn scan(root: &std::path::Path) -> Scan {
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        let mut budget = Budget::new();
        WorkspaceStore::new(root.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        scan
    }

    #[test]
    fn a_workspace_conversation_lists_metadata_without_inventing_a_transcript() {
        let root = tempfile::tempdir().unwrap();
        write_session(
            root.path(),
            "874b7102139062fc",
            &format!("sess_{UUID}"),
            &format!(
                r#"{{"schemaVersion":"1.0.0","dataModelVersion":1,"id":"sess_{UUID}","title":"New Session","agentMode":"vibe","workspacePaths":["/home/siddu/project"],"rootPaths":["/home/siddu/project"],"createdAt":"2026-10-27T10:59:23.051Z","lastModifiedAt":"2026-10-27T10:59:37.689Z"}}"#
            ),
        );
        let scan = scan(root.path());
        assert_eq!(scan.conversations.len(), 1);
        let record = &scan.conversations[0];
        assert_eq!(record.native_id, UUID);
        assert_eq!(record.stores, vec![StoreKind::Workspaces]);
        assert_eq!(record.title.as_deref(), Some("New Session"));
        assert_eq!(record.cwd.as_deref(), Some("/home/siddu/project"));
        assert_eq!(record.updated_at_ms, Some(1_793_098_777_000));
        // `agentMode` is a mode, never presented as a Kiro profile.
        assert_eq!(record.agent, None);
        // No transcript was read, so a search must not claim text coverage.
        assert!(!record.transcript);
        assert!(record.text.is_empty());
        assert_eq!(record.lock, LockState::Unknown);
        assert!(record.unusable.is_some());
    }

    #[test]
    fn a_disambiguated_import_keeps_the_identity_kiro_states_for_it() {
        let root = tempfile::tempdir().unwrap();
        // The real shape Kiro writes for a conversation imported into a workspace:
        // the id states `cli_<uuid>` plus a disambiguating suffix. The UUID is the
        // identity Kiro itself recorded, so the row keeps it; resume still requires
        // that conversation to exist in the file store, which this row does not.
        write_session(
            root.path(),
            "1be30a73b134ae51",
            &format!("cli_{UUID}_sRLEzWYY"),
            &format!(
                r#"{{"schemaVersion":"1.0.0","id":"cli_{UUID}_sRLEzWYY","title":"Imported work","rootPaths":["/srv/app"]}}"#
            ),
        );
        let scan = scan(root.path());
        assert_eq!(scan.conversations.len(), 1, "{scan:?}");
        assert_eq!(scan.conversations[0].native_id, UUID);
        assert_eq!(scan.conversations[0].stores, vec![StoreKind::Workspaces]);
        // With no file-store row, the suffixed import is never resumed.
        assert!(matches!(
            crate::kiro::history::resume::decide(&scan.conversations[0]),
            crate::kiro::history::resume::Decision::Blocked(_)
        ));
    }

    #[test]
    fn unknown_schemas_and_disagreeing_directory_names_are_never_guessed() {
        let root = tempfile::tempdir().unwrap();
        // Future/unknown schema.
        write_session(
            root.path(),
            "aaaa",
            &format!("sess_{UUID}"),
            r#"{"schemaVersion":"2.0.0","id":"x"}"#,
        );
        // Identity does not match the directory that holds it.
        write_session(
            root.path(),
            "bbbb",
            &format!("sess_{UUID}"),
            r#"{"schemaVersion":"1.0.0","id":"sess_22222222-2222-4222-8222-222222222222"}"#,
        );
        // No schema at all.
        write_session(
            root.path(),
            "cccc",
            "sess_33333333-3333-4333-8333-333333333333",
            "{}",
        );
        // A directory that is not a session directory is ignored silently.
        fs::create_dir_all(root.path().join("dddd/.index")).unwrap();
        fs::write(root.path().join("dddd/.index/index.json"), "{}").unwrap();
        // A symlinked session directory is never walked.
        let outside = tempfile::tempdir().unwrap();
        write_session(outside.path(), "eee", &format!("sess_{UUID}"), "x");
        std::os::unix::fs::symlink(outside.path().join("eee"), root.path().join("ffff_sess"))
            .unwrap();
        let scan = scan(root.path());
        assert!(scan.conversations.is_empty(), "{:?}", scan.conversations);
        assert_eq!(scan.unrecognised, 3, "{scan:?}");
    }

    #[test]
    fn a_migrated_conversation_stays_one_resumable_row_with_both_provenances() {
        let root = tempfile::tempdir().unwrap();
        let files = root.path().join("sessions/cli");
        let workspaces = root.path().join("sessions");
        fs::create_dir_all(&files).unwrap();
        fs::write(
            files.join(format!("{UUID}.jsonl")),
            "{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"hello hello\"}]}}\n",
        )
        .unwrap();
        // The recorded cwd must exist for a resume to be offered at all.
        let project = root.path().to_str().unwrap().replace('"', "");
        let metadata = format!(
            r#"{{"session_id":"{UUID}","cwd":"{project}","created_at":"2026-10-16T23:07:53Z","updated_at":"2026-10-27T10:59:37Z","title":"hello hello","session_state":{{"agent_name":"kiro_default"}}}}"#
        );
        fs::write(files.join(format!("{UUID}.json")), metadata).unwrap();
        write_session(
            &workspaces,
            "1be30a73b134ae51",
            &format!("cli_{UUID}_sRLEzWYY"),
            &format!(
                r#"{{"schemaVersion":"1.0.0","id":"cli_{UUID}","title":"hello hello","agentMode":"vibe","rootPaths":["{project}"],"lastModifiedAt":"2026-10-27T10:59:37Z"}}"#
            ),
        );
        let roots = StoreRoots {
            files: Some(files.clone()),
            workspaces: Some(workspaces.clone()),
        };
        let scan = history::scan(&roots, &AtomicBool::new(false));
        assert_eq!(scan.conversations.len(), 1, "{:?}", scan.conversations);
        let record = &scan.conversations[0];
        assert_eq!(record.stores, vec![StoreKind::Files, StoreKind::Workspaces]);
        assert!(record.transcript);
        assert_eq!(scan.stores, vec![StoreKind::Files, StoreKind::Workspaces]);
        // The v1 side keeps it resumable; the v2 row alone never would be.
        assert!(history::resume::decide(record).resumable());
    }
}
