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
    sync::atomic::AtomicBool,
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

/// What one candidate turned out to be. Only `Unrecognised` may raise the
/// scan's unrecognised count: a conversation the budgets ran out of room for
/// is a truncated scan, never a record this wrapper failed to understand.
enum Outcome {
    /// A conversation was read.
    Found(Conversation),
    /// Shaped like a native record but its own content disagreed or was
    /// unreadable, so nothing is guessed about it.
    Unrecognised,
    /// A valid conversation dropped because a budget was exhausted.
    Budgeted,
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
            // Every inspected directory entry is charged, not just the
            // matching ones: a huge directory of irrelevant files cannot
            // outrun the advertised bound before parsing starts.
            if !budget.take_entry(into) {
                // Out of entry budget: process what was collected, don't drop it.
                break;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
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
                Outcome::Found(conversation) => into.conversations.push(conversation),
                Outcome::Unrecognised => into.unrecognised += 1,
                Outcome::Budgeted => {
                    // The conversation budget is shared by the whole scan, so
                    // no later candidate could be added either. Stop here and
                    // let truncation speak, instead of charging the remaining
                    // valid rows to being unrecognised.
                    into.truncated = true;
                    break;
                }
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
) -> Outcome {
    let Some(metadata) = read_json_bounded(path, MAX_METADATA_BYTES) else {
        return Outcome::Unrecognised;
    };
    // Identity must be stated by the store and match its own file: a filename
    // alone is never treated as a conversation.
    let Some(native_id) = metadata.get("session_id").and_then(Value::as_str) else {
        return Outcome::Unrecognised;
    };
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Outcome::Unrecognised;
    };
    if native_id != stem || native_uuid(native_id).is_none() {
        return Outcome::Unrecognised;
    }
    if !budget.take_conversation(into) {
        // Recognised, valid and unaffordable: truncation, not a mystery row.
        return Outcome::Budgeted;
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
    Outcome::Found(Conversation {
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
    let transcript_bytes = match fs::symlink_metadata(&transcript_path) {
        Ok(info) if info.is_file() => Some(info.len()),
        _ => None,
    };
    if let Some(total) = transcript_bytes {
        let limit = total.min(MAX_TEXT_PER_CONVERSATION);
        // What is charged is what is read: the byte budget bounds the actual
        // read, not only the accounting done before it.
        if budget.take_text(limit, into) {
            if total > limit {
                // Only the first `limit` bytes of this transcript can ever be
                // searched, so the coverage footer says the scan is partial
                // rather than implying the whole transcript was indexed.
                into.partial = true;
            }
            let runs = transcript_runs(&transcript_path, limit, budget, cancelled);
            if !runs.is_empty() {
                return (runs, true);
            }
        }
    }
    // A store without a readable transcript still lists its prompt history,
    // which is Kiro's own prompt record rather than a wrapper guess.
    let history = directory.join(format!("{native_id}.history"));
    let Ok(info) = fs::symlink_metadata(&history) else {
        return (Vec::new(), transcript_bytes.is_none());
    };
    if !info.is_file() {
        return (Vec::new(), transcript_bytes.is_none());
    }
    let limit = info.len().min(MAX_TEXT_PER_CONVERSATION);
    if !budget.take_text(limit, into) {
        return (Vec::new(), false);
    }
    let Ok(file) = File::open(&history) else {
        return (Vec::new(), false);
    };
    (history_runs(file, limit, budget, cancelled), true)
}

/// One `\n`-terminated record, never more than `MAX_RECORD_BYTES + 1` of it.
/// `None` is end of input or a read error; `Some(true)` means the record is
/// over budget and only its head was seen, so the caller must drop the rest.
fn read_record(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> Option<bool> {
    buf.clear();
    match reader
        .by_ref()
        .take(MAX_RECORD_BYTES + 1)
        .read_until(b'\n', buf)
    {
        Ok(0) | Err(_) => None,
        // Only a read that ran to the cap without finding its terminator ran
        // over budget. A record of exactly `MAX_RECORD_BYTES` plus its `\n`
        // fills the cap and is complete, so calling it oversized would make
        // the caller skip the following record as if it were this one's tail.
        Ok(_) => Some(buf.len() as u64 > MAX_RECORD_BYTES && !buf.ends_with(b"\n")),
    }
}

/// Discards the rest of an over-budget record so its tail can never be read as
/// a separate record. Bounded by the caller's byte limit.
fn skip_record_tail(reader: &mut impl BufRead) -> bool {
    let mut sink = Vec::new();
    loop {
        sink.clear();
        match reader
            .by_ref()
            .take(MAX_RECORD_BYTES + 1)
            .read_until(b'\n', &mut sink)
        {
            Ok(0) | Err(_) => return false,
            Ok(_) => {
                if sink.ends_with(b"\n") {
                    return true;
                }
            }
        }
    }
}

fn transcript_runs(
    path: &Path,
    limit: u64,
    budget: &mut Budget,
    cancelled: &AtomicBool,
) -> Vec<TextRun> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    read_transcript_runs(file, limit, budget, cancelled)
}

fn read_transcript_runs(
    source: impl Read,
    limit: u64,
    budget: &mut Budget,
    cancelled: &AtomicBool,
) -> Vec<TextRun> {
    // The limit is enforced on the read path itself: a transcript many times
    // the conversation budget is never streamed past it.
    let mut reader = BufReader::new(source.take(limit));
    let mut runs = Vec::new();
    let mut line = Vec::new();
    loop {
        // Checked before a record is judged, not after one is accepted, so a
        // store made only of records this wrapper skips stays cancellable and
        // still stops when the deadline passes.
        if budget.stopped(cancelled) {
            return runs;
        }
        match read_record(&mut reader, &mut line) {
            None => return runs,
            Some(true) => {
                // A record bigger than the budget is skipped whole, never
                // truncated into a partial snippet, and its tail never
                // resurfaces as a record of its own.
                if !skip_record_tail(&mut reader) {
                    return runs;
                }
                continue;
            }
            Some(false) => (),
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
        if runs.len() >= MAX_RUNS {
            break;
        }
    }
    runs
}

fn history_runs(
    source: impl Read,
    limit: u64,
    budget: &mut Budget,
    cancelled: &AtomicBool,
) -> Vec<TextRun> {
    let mut reader = BufReader::new(source.take(limit));
    let mut runs = Vec::new();
    let mut line = Vec::new();
    for _ in 0..MAX_HISTORY_LINES {
        if budget.stopped(cancelled) {
            break;
        }
        match read_record(&mut reader, &mut line) {
            None => break,
            Some(true) => {
                if !skip_record_tail(&mut reader) {
                    break;
                }
                continue;
            }
            Some(false) => (),
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
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::history::{lock_state, LockState, Scan, StoreRoots};
    use std::sync::atomic::Ordering;
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
            entries: 20_000,
            conversations: 3,
            text_bytes: 100 * 1024,
            deadline: None,
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        assert_eq!(scan.conversations.len(), 3, "{scan:?}");
        assert!(
            scan.partial,
            "text budget exhaustion must be visible, not a silent full scan"
        );
        assert!(scan.conversations.iter().any(|row| !row.transcript));
    }

    #[test]
    fn an_exhausted_conversation_budget_truncates_rather_than_calling_valid_rows_unrecognised() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        // One genuinely unrecognised file sorts first, so the count that stays
        // honest is visible next to the count that must not inflate.
        write(
            directory,
            "00000000-0000-4000-8000-000000000000.json",
            r#"{"cwd":"/tmp"}"#,
        );
        for index in 1..=5 {
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
            conversations: 2,
            text_bytes: 6 * 1024 * 1024,
            deadline: None,
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        // Three valid conversations were dropped for want of budget. They are
        // not unrecognised: the scan ran out of room and says so.
        assert_eq!(scan.conversations.len(), 2, "{scan:?}");
        assert_eq!(scan.unrecognised, 1, "{scan:?}");
        assert!(scan.truncated, "{scan:?}");
    }

    #[test]
    fn an_exhausted_text_budget_never_reclassifies_a_valid_row_as_unrecognised() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        for index in 0..3 {
            let id = format!("{:08}-0000-4000-8000-000000000000", index);
            write(
                directory,
                &format!("{id}.json"),
                &metadata(&id, "/tmp", Some("row")),
            );
            write(
                directory,
                &format!("{id}.jsonl"),
                &skipped_records_text(2_000),
            );
        }
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        // Enough for no transcript at all: every row is still a conversation.
        let mut budget = Budget {
            entries: 20_000,
            conversations: 2_000,
            text_bytes: 1,
            deadline: None,
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        assert_eq!(scan.conversations.len(), 3, "{scan:?}");
        assert_eq!(scan.unrecognised, 0, "{scan:?}");
        assert!(scan.partial, "{scan:?}");
        assert!(scan.conversations.iter().all(|row| !row.transcript));
    }

    #[test]
    fn the_entry_budget_charges_every_inspected_directory_entry() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        for index in 0..5 {
            let id = format!("{:08}-0000-4000-8000-00000000000{}", index, index);
            write(
                directory,
                &format!("{id}.json"),
                &metadata(&id, "/tmp", Some("row")),
            );
            // Unrelated files must still cost budget: a directory full of
            // them cannot outrun the advertised entry bound.
            write(directory, &format!("notes-{index}.txt"), "not a session");
        }
        let cancelled = AtomicBool::new(false);
        let mut scan = Scan::default();
        let mut budget = Budget {
            entries: 5,
            conversations: 2_000,
            text_bytes: 6 * 1024 * 1024,
            deadline: None,
        };
        FileStore::new(directory.to_path_buf()).scan(&mut budget, &cancelled, &mut scan);
        // Ten entries were inspected, only five were affordable, whatever
        // the directory order: truncation is always visible.
        assert!(scan.truncated, "{scan:?}");
        assert!(scan.conversations.len() <= 5, "{scan:?}");
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
    fn a_transcript_is_never_read_past_the_bytes_it_charged() {
        // Records this adapter does not index still cost the read they take.
        let body = skipped_records(40_000);
        let mut source = std::io::Cursor::new(body);
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new();
        let runs = read_transcript_runs(
            &mut source,
            MAX_TEXT_PER_CONVERSATION,
            &mut budget,
            &cancelled,
        );
        assert!(runs.is_empty());
        assert!(
            source.position() <= MAX_TEXT_PER_CONVERSATION,
            "read {} bytes against a {MAX_TEXT_PER_CONVERSATION} byte budget",
            source.position()
        );
    }

    #[test]
    fn prompt_history_is_never_read_past_the_bytes_it_charged() {
        let line = format!("{}\n", "p".repeat(4_000));
        let body = line.repeat(2_000);
        let mut source = std::io::Cursor::new(body);
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new();
        let runs = history_runs(
            &mut source,
            MAX_TEXT_PER_CONVERSATION,
            &mut budget,
            &cancelled,
        );
        assert!(!runs.is_empty());
        assert!(
            runs.len() < MAX_HISTORY_LINES,
            "the byte limit never bound the read: {} lines",
            runs.len()
        );
        assert!(
            source.position() <= MAX_TEXT_PER_CONVERSATION,
            "read {} bytes against a {MAX_TEXT_PER_CONVERSATION} byte budget",
            source.position()
        );
    }

    #[test]
    fn cancellation_stops_a_transcript_whose_records_are_all_skipped() {
        // Nothing here is ever indexed, so the only thing that can bound the
        // read is the cancellation check on the skip path.
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let mut source = CancelAfter {
            body: skipped_records(100_000),
            position: 0,
            after: 64 * 1024,
            flag: flag.clone(),
        };
        let mut budget = Budget::new();
        let runs = read_transcript_runs(&mut source, 8 * 1024 * 1024, &mut budget, &flag);
        assert!(runs.is_empty());
        assert!(flag.load(Ordering::Relaxed));
        assert!(
            source.position <= 160 * 1024,
            "kept reading {} bytes after cancellation",
            source.position
        );
    }

    #[test]
    fn an_expired_deadline_stops_before_reading_skipped_records() {
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget {
            entries: 20_000,
            conversations: 2_000,
            text_bytes: 6 * 1024 * 1024,
            deadline: Some(std::time::Instant::now() - std::time::Duration::from_secs(1)),
        };
        let mut source = std::io::Cursor::new(skipped_records(40_000));
        let runs = read_transcript_runs(&mut source, 8 * 1024 * 1024, &mut budget, &cancelled);
        assert!(runs.is_empty());
        assert_eq!(source.position(), 0, "the deadline was not honoured");
    }

    #[test]
    fn the_tail_of_an_oversized_record_is_never_a_record_of_its_own() {
        // The head is over budget and gets skipped; the tail is valid JSON
        // that must not be indexed as a separate conversation record.
        let tail = concat!(
            r#"{"version":"v1","kind":"Prompt","data":{"content":[{"kind":"text","data":"tail of an oversized record"}]}}"#,
            "\n",
        );
        let body = format!("{}{tail}", " ".repeat(20_000));
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new();
        let runs = read_transcript_runs(
            std::io::Cursor::new(body),
            MAX_TEXT_PER_CONVERSATION,
            &mut budget,
            &cancelled,
        );
        assert!(runs.is_empty(), "{runs:?}");
    }

    #[test]
    fn a_transcript_record_of_exactly_the_limit_keeps_the_record_after_it() {
        // `MAX_RECORD_BYTES` of JSON plus its `\n` fills the bounded read but
        // is still one whole record: it must be indexed, and the record that
        // follows it must not be thrown away as an oversized tail.
        let exact = transcript_record_of_len(MAX_RECORD_BYTES as usize);
        let next = concat!(
            r#"{"version":"v1","kind":"AssistantMessage","data":{"content":[{"kind":"text","data":"the record after the limit"}]}}"#,
            "\n",
        );
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new();
        let runs = read_transcript_runs(
            std::io::Cursor::new(format!("{exact}\n{next}")),
            MAX_TEXT_PER_CONVERSATION,
            &mut budget,
            &cancelled,
        );
        assert_eq!(
            runs.iter().map(|run| run.role).collect::<Vec<_>>(),
            vec![TextRole::Prompt, TextRole::Response],
            "{runs:?}"
        );
        assert!(runs[1].text.contains("the record after the limit"));
    }

    #[test]
    fn a_history_line_of_exactly_the_limit_keeps_the_line_after_it() {
        let line = "h".repeat(MAX_RECORD_BYTES as usize);
        let cancelled = AtomicBool::new(false);
        let mut budget = Budget::new();
        let runs = history_runs(
            std::io::Cursor::new(format!("{line}\nthe line after the limit\n")),
            MAX_TEXT_PER_CONVERSATION,
            &mut budget,
            &cancelled,
        );
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[0].text.chars().count(), 80);
        assert_eq!(runs[1].role, TextRole::PromptHistory);
        assert_eq!(runs[1].text, "the line after the limit");
    }

    #[test]
    fn a_transcript_larger_than_the_limit_reports_partial_coverage() {
        let store = tempfile::tempdir().unwrap();
        let directory = store.path();
        write(
            directory,
            &format!("{ID}.json"),
            &metadata(ID, "/tmp", Some("t")),
        );
        write(
            directory,
            &format!("{ID}.jsonl"),
            &skipped_records_text(2_000),
        );
        let scan = scan_one(directory);
        // The indexed text came from the first 96 KiB of a bigger transcript,
        // so the scan must not claim complete coverage.
        assert!(scan.partial, "{scan:?}");
    }

    /// One `Prompt` transcript record padded to exactly `bytes` of JSON.
    fn transcript_record_of_len(bytes: usize) -> String {
        let empty = format!(
            concat!(
                r#"{{"version":"v1","kind":"Prompt","data":{{"content":[{{"kind":"text","data":"{}"}}]}}}}"#,
            ),
            ""
        );
        let record = format!(
            r#"{{"version":"v1","kind":"Prompt","data":{{"content":[{{"kind":"text","data":"{}"}}]}}}}"#,
            "x".repeat(bytes - empty.len())
        );
        assert_eq!(record.len(), bytes);
        record
    }

    /// `count` transcript records that carry a kind this adapter never indexes.
    fn skipped_records(count: usize) -> Vec<u8> {
        skipped_records_text(count).into_bytes()
    }

    fn skipped_records_text(count: usize) -> String {
        (0..count)
            .map(|index| {
                format!(
                    "{{\"version\":\"v1\",\"kind\":\"ToolResults\",\"data\":{{\"content\":[{{\"kind\":\"text\",\"data\":\"tool output {index}\"}}]}}}}\n"
                )
            })
            .collect()
    }

    /// A source that flips the cancellation flag once it has handed over
    /// `after` bytes, standing in for a cancellation that arrives mid-read.
    struct CancelAfter {
        body: Vec<u8>,
        position: usize,
        after: u64,
        flag: std::sync::Arc<AtomicBool>,
    }

    impl std::io::Read for CancelAfter {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let taken = (self.body.len() - self.position).min(buf.len());
            buf[..taken].copy_from_slice(&self.body[self.position..self.position + taken]);
            self.position += taken;
            if self.position as u64 >= self.after {
                self.flag.store(true, Ordering::Relaxed);
            }
            Ok(taken)
        }
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
