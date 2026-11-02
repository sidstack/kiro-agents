//! Read-only adapter for Kiro's file-based session store.
//!
//! Shape confirmed by read-only inspection (see docs/FINDINGS.md): one
//! `<uuid>.json` metadata file per conversation with `session_id`, `cwd`,
//! `created_at`, `updated_at`, `title` and `session_state`, a `<uuid>.jsonl`
//! transcript whose records carry `kind` discriminators, a `<uuid>.history`
//! prompt list and a `<uuid>.lock` holding `pid`/`started_at`.

use super::{
    lock_state, native_uuid, read_json_bounded, timestamp_ms, Budget, Conversation, Scan,
    StoreAdapter, StoreKind, TextRole, TextRun, MAX_METADATA_BYTES, MAX_RECORD_BYTES,
    MAX_TEXT_PER_CONVERSATION,
};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Transcript runs kept per conversation.
const MAX_RUNS: usize = 120;
/// Characters kept from one transcript record.
const MAX_RUN_CHARS: usize = 2_000;
/// Prompt-history lines kept when a conversation has no transcript file.
const MAX_HISTORY_LINES: usize = 64;

pub struct FileStore {
    directory: PathBuf,
}

impl FileStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

impl StoreAdapter for FileStore {
    fn kind(&self) -> StoreKind {
        StoreKind::Files
    }

    fn root(&self) -> &std::path::Path {
        &self.directory
    }

    fn scan(&self, budget: &mut Budget, cancelled: &AtomicBool, into: &mut Scan) {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(_) => {
                into.note(true);
                return;
            }
        };
        // Sorted so a bounded scan is deterministic rather than directory
        // order. Enumeration itself stays inside the entry budget and honours
        // cancellation and the deadline, so a huge directory cannot outrun
        // either bound before parsing starts.
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            if budget.stopped(cancelled) {
                into.partial = true;
                return;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if !budget.take_entry(into) {
                // Out of entry budget: process what was collected, don't drop it.
                break;
            }
            paths.push(path);
        }
        paths.sort();
        for path in paths {
            if budget.stopped(cancelled) {
                into.partial = true;
                return;
            }
            if !fs::symlink_metadata(&path)
                .map(|info| info.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            match record(&self.directory, &path, budget, cancelled, into) {
                Some(conversation) => into.conversations.push(conversation),
                None => into.unrecognised += 1,
            }
        }
    }
}

fn record(
    directory: &Path,
    path: &Path,
    budget: &mut Budget,
    cancelled: &AtomicBool,
    into: &mut Scan,
) -> Option<Conversation> {
    let metadata = read_json_bounded(path, MAX_METADATA_BYTES)?;
    // Identity must be stated by the store and match its own file: a filename
    // alone is never treated as a conversation.
    let native_id = metadata.get("session_id").and_then(Value::as_str)?;
    let stem = path.file_stem()?.to_str()?;
    if native_id != stem || native_uuid(native_id).is_none() {
        return None;
    }
    if !budget.take_conversation(into) {
        return None;
    }
    let clean = |value: Option<&Value>| -> Option<String> {
        value
            .and_then(Value::as_str)
            .and_then(crate::features::clean_title)
    };
    let title = clean(metadata.get("title")).filter(|title| !title.is_empty());
    let cwd = metadata
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty() && !cwd.chars().any(char::is_control))
        .map(str::to_owned);
    let agent = clean(
        metadata
            .get("session_state")
            .and_then(|state| state.get("agent_name")),
    )
    .filter(|agent| !agent.contains('/'));
    let updated_at_ms = metadata
        .get("updated_at")
        .and_then(Value::as_str)
        .and_then(timestamp_ms)
        .or_else(|| {
            metadata
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(timestamp_ms)
        })
        .or_else(|| file_time_ms(path));
    let (text, transcript) = conversation_text(directory, native_id, budget, cancelled, into);
    Some(Conversation {
        native_id: native_id.to_owned(),
        stores: vec![StoreKind::Files],
        title,
        cwd,
        agent,
        updated_at_ms,
        lock: lock_state(directory, native_id),
        text,
        transcript,
        unusable: None,
    })
}

fn file_time_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|age| age.as_millis() as u64)
}

/// Transcript text with provenance. Only Kiro's own tagged user/assistant
/// records are read; tool results and unknown record kinds are skipped.
fn conversation_text(
    directory: &Path,
    native_id: &str,
    budget: &mut Budget,
    cancelled: &AtomicBool,
    into: &mut Scan,
) -> (Vec<TextRun>, bool) {
    let transcript_path = directory.join(format!("{native_id}.jsonl"));
    let size = match fs::symlink_metadata(&transcript_path) {
        Ok(info) if info.is_file() => info.len().min(MAX_TEXT_PER_CONVERSATION),
        _ => 0,
    };
    if size > 0 && budget.take_text(size, into) {
        let runs = transcript_runs(&transcript_path, cancelled);
        if !runs.is_empty() {
            return (runs, true);
        }
    }
    // A store without a readable transcript still lists its prompt history,
    // which is Kiro's own prompt record rather than a wrapper guess.
    let history = directory.join(format!("{native_id}.history"));
    let Ok(info) = fs::symlink_metadata(&history) else {
        return (Vec::new(), size == 0);
    };
    if !info.is_file() {
        return (Vec::new(), size == 0);
    }
    if !budget.take_text(info.len().min(MAX_TEXT_PER_CONVERSATION), into) {
        return (Vec::new(), false);
    }
    let Ok(file) = File::open(&history) else {
        return (Vec::new(), false);
    };
    let mut runs = Vec::new();
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    for _ in 0..MAX_HISTORY_LINES {
        line.clear();
        match reader
            .by_ref()
            .take(MAX_RECORD_BYTES)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => (),
        }
        if let Some(text) = std::str::from_utf8(&line)
            .ok()
            .and_then(|text| crate::features::clean_title(text.trim()))
        {
            if !text.is_empty() {
                runs.push(TextRun {
                    role: TextRole::PromptHistory,
                    text,
                });
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
    }
    (runs, true)
}

fn transcript_runs(path: &Path, cancelled: &AtomicBool) -> Vec<TextRun> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut runs = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) => (),
        }
        if line.len() as u64 > MAX_RECORD_BYTES {
            // A record bigger than the budget is skipped, never truncated into
            // a partial snippet.
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let role = match record.get("kind").and_then(Value::as_str) {
            Some("Prompt") => TextRole::Prompt,
            Some("AssistantMessage") => TextRole::Response,
            // Tool results, plans and unknown kinds are not conversation text.
            _ => continue,
        };
        let Some(parts) = record
            .get("data")
            .and_then(|data| data.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let text = parts
            .iter()
            .filter_map(|part| {
                if part.get("kind").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                part.get("data").and_then(Value::as_str)
            })
            .collect::<Vec<_>>()
            .join(" ");
        let Some(text) = crate::features::clean_title(&text) else {
            continue;
        };
        let text: String = text.chars().take(MAX_RUN_CHARS).collect();
        if text.is_empty() {
            continue;
        }
        runs.push(TextRun { role, text });
        if runs.len() >= MAX_RUNS || cancelled.load(Ordering::Relaxed) {
            break;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{lock_state, LockState, Scan, StoreRoots};
    use std::{fs, os::unix::fs::PermissionsExt};

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    fn metadata(id: &str, cwd: &str, title: Option<&str>) -> String {
        format!(
            r#"{{"session_id":"{id}","cwd":"{cwd}","created_at":"2026-10-16T23:07:53Z","updated_at":"2026-10-27T10:59:23Z","title":{},"session_created_reason":"subagent","session_state":{{"version":"v1","agent_name":"kiro_default"}}}}"#,
            title.map_or("null".to_owned(), |title| format!(r#""{title}""#))
        )
    }

    fn write(directory: &Path, name: &str, body: &str) {
        fs::write(directory.join(name), body).unwrap();
    }

    fn scan_one(directory: &Path) -> Scan {
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        let mut budget = Budget::new();
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        scan
    }

    #[test]
    fn a_record_comes_from_kiro_s_own_fields_and_transcript() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        write(
            directory,
            &format!("{ID}.json"),
            &metadata(ID, "/tmp/project", Some("Fix the board")),
        );
        write(
            directory,
            &format!("{ID}.jsonl"),
            concat!(
                "{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"fix the picker ranking\"}]}}\n",
                "{\"version\":\"v1\",\"kind\":\"AssistantMessage\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"ranking now prefers exact words\"}]}}\n",
                "{\"version\":\"v1\",\"kind\":\"ToolResults\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"SECRET TOOL OUTPUT\"}]}}\n",
            ),
        );
        let scan = scan_one(directory);
        assert_eq!(scan.conversations.len(), 1);
        let record = &scan.conversations[0];
        assert_eq!(record.native_id, ID);
        assert_eq!(record.stores, vec![StoreKind::Files]);
        assert_eq!(record.title.as_deref(), Some("Fix the board"));
        assert_eq!(record.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(record.agent.as_deref(), Some("kiro_default"));
        assert_eq!(record.updated_at_ms, Some(1_793_098_763_000));
        assert!(record.transcript);
        let roles: Vec<_> = record.text.iter().map(|run| run.role).collect();
        assert_eq!(roles, vec![TextRole::Prompt, TextRole::Response]);
        assert!(!scan
            .conversations
            .iter()
            .any(|row| row.text.iter().any(|run| run.text.contains("SECRET"))));
        assert_eq!(scan.unrecognised, 0);
        assert!(!scan.partial && !scan.truncated);
    }

    #[test]
    fn a_missing_transcript_falls_back_to_kiro_s_prompt_history() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        write(
            directory,
            &format!("{ID}.json"),
            &metadata(ID, "/tmp/project", None),
        );
        write(
            directory,
            &format!("{ID}.history"),
            "hello hello\nsecond prompt\n",
        );
        let scan = scan_one(directory);
        let record = &scan.conversations[0];
        assert!(record.transcript);
        assert_eq!(record.title, None);
        assert_eq!(record.text.len(), 2);
        assert_eq!(record.text[0].role, TextRole::PromptHistory);
        assert_eq!(record.text[0].text, "hello hello");
    }

    #[test]
    fn identity_mismatch_corruption_and_symlinks_are_never_guessed() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        // Identity stated in the file disagrees with the file name.
        write(
            directory,
            "22222222-2222-4222-8222-222222222222.json",
            &metadata(ID, "/tmp", Some("x")),
        );
        // Truncated/corrupt metadata.
        write(
            directory,
            "33333333-3333-4333-8333-333333333333.json",
            "{\"session_id\":",
        );
        // No session_id at all.
        write(
            directory,
            "44444444-4444-4444-8444-444444444444.json",
            r#"{"cwd":"/tmp"}"#,
        );
        // A non-UUID stem is not a conversation this wrapper can act on.
        write(
            directory,
            "not-a-session.json",
            r#"{"session_id":"not-a-session"}"#,
        );
        // A symlinked metadata file is never followed.
        let outside = tempfile::tempdir().unwrap();
        write(
            outside.path(),
            "target.json",
            &metadata(
                "55555555-5555-4555-8555-555555555555",
                "/tmp",
                Some("linked"),
            ),
        );
        std::os::unix::fs::symlink(
            outside.path().join("target.json"),
            directory.join("55555555-5555-4555-8555-555555555555.json"),
        )
        .unwrap();
        let scan = scan_one(directory);
        assert!(scan.conversations.is_empty());
        // Four understood-shaped files with unusable content; the symlinked
        // metadata is not even opened, so it contributes nothing.
        assert_eq!(scan.unrecognised, 4, "{scan:?}");
    }

    #[test]
    fn an_unreadable_store_reports_itself_as_unreadable_not_empty_success() {
        let store = tempfile::tempdir().unwrap();
        let blocked = store.path().join("locked");
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let scan = scan_one(&blocked);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(scan.unreadable, 1);
        assert!(scan.conversations.is_empty());
    }

    #[test]
    fn locks_are_read_without_ever_touching_them() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        write(
            directory,
            &format!("{ID}.json"),
            &metadata(ID, "/tmp", Some("t")),
        );
        assert_eq!(scan_one(directory).conversations[0].lock, LockState::Free);
        write(
            directory,
            &format!("{ID}.lock"),
            &format!(r#"{{"pid":{},"started_at":"fixture"}}"#, std::process::id()),
        );
        assert_eq!(
            scan_one(directory).conversations[0].lock,
            lock_state(directory, ID)
        );
        assert_eq!(
            scan_one(directory).conversations[0].lock,
            LockState::Open {
                pid: std::process::id()
            }
        );
        write(directory, &format!("{ID}.lock"), "{}");
        assert_eq!(
            scan_one(directory).conversations[0].lock,
            LockState::Unknown
        );
        // Nothing here wrote, renamed or deleted a native file.
        assert!(directory.join(format!("{ID}.lock")).exists());
    }

    #[test]
    fn budgets_truncate_entries_and_report_partial_text_coverage() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        for index in 0..5 {
            let id = format!("{:08}-0000-4000-8000-000000000000", index);
            write(
                directory,
                &format!("{id}.json"),
                &metadata(&id, "/tmp", Some("row")),
            );
            let record = format!(
                "{{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{{\"content\":[{{\"kind\":\"text\",\"data\":\"{}\"}}]}}}}",
                "x".repeat(40_000)
            );
            write(directory, &format!("{id}.jsonl"), &format!("{record}\n"));
        }
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        let mut budget = Budget {
            entries: 3,
            conversations: 3,
            text_bytes: 100 * 1024,
            deadline: None,
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        assert_eq!(scan.conversations.len(), 3, "{scan:?}");
        assert!(scan.truncated);
        assert!(
            scan.partial,
            "text budget exhaustion must be visible, not a silent full scan"
        );
        assert!(scan.conversations.iter().any(|row| !row.transcript));
    }

    #[test]
    fn an_expired_deadline_stops_a_scan_in_progress() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        for index in 0..5 {
            let id = format!("{:08}-0000-4000-8000-000000000000", index);
            write(
                directory,
                &format!("{id}.json"),
                &metadata(&id, "/tmp", Some("row")),
            );
        }
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        let mut budget = Budget {
            entries: 20_000,
            conversations: 2_000,
            text_bytes: 6 * 1024 * 1024,
            deadline: Some(std::time::Instant::now() - std::time::Duration::from_secs(1)),
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        // The deadline stops the scan inside the adapter, not just between
        // adapters, and the partial coverage is stated rather than hidden.
        assert!(scan.conversations.is_empty(), "{scan:?}");
        assert!(scan.partial);
    }

    #[test]
    fn huge_records_are_skipped_rather_than_partially_indexed() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        write(
            directory,
            &format!("{ID}.json"),
            &metadata(ID, "/tmp", Some("t")),
        );
        let record = format!(
            "{{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{{\"content\":[{{\"kind\":\"text\",\"data\":\"{}\"}}]}}}}",
            "y".repeat(40_000)
        );
        write(directory, &format!("{ID}.jsonl"), &format!("{record}\n"));
        let scan = scan_one(directory);
        let record = &scan.conversations[0];
        assert!(record.text.is_empty());
        assert!(!record.transcript);
    }

    #[test]
    fn the_file_adapter_is_selected_only_for_its_own_root() {
        let store = tempfile::tempdir().unwrap();
        let roots = StoreRoots {
            files: Some(store.path().join("sessions/cli")),
            workspaces: Some(store.path().join("sessions")),
        };
        let scan = crate::kiro::history::scan(&roots, &AtomicBool::new(false));
        assert!(scan.conversations.is_empty());
        // A root that does not exist contributes no store to the coverage list.
        assert!(scan.stores.is_empty(), "{scan:?}");
    }
}
