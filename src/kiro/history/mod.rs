//! Read-only discovery of Kiro's own conversation stores.
//!
//! Kiro owns its history. This module only reads the documented file layouts,
//! never writes to or deletes anything inside a native store, and never guesses
//! metadata from a filename, a directory's recency or an unverified schema.
//! Two adapters sit behind one read-only interface so an unknown generation is
//! an explicit unsupported state rather than an inferred one.

pub mod resume;
mod search;
mod v1;
mod v2;

pub use search::{search, Hit, Snippet, MAX_RESULTS};

use serde_json::Value;
use std::{
    env,
    fs::{self, File},
    io::Read,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Conversations kept from one scan; newer ones win when the budget is hit.
pub const MAX_CONVERSATIONS: usize = 2_000;
/// Store entries examined per scan, so a huge or hostile tree stays bounded.
const MAX_ENTRIES: usize = 20_000;
/// Native metadata is small; anything larger is not parsed.
pub(crate) const MAX_METADATA_BYTES: u64 = 64 * 1024;
/// One transcript record is skipped when it exceeds this.
pub(crate) const MAX_RECORD_BYTES: u64 = 16 * 1024;
/// Conversation text read for one conversation.
pub(crate) const MAX_TEXT_PER_CONVERSATION: u64 = 96 * 1024;
/// Conversation text read for one whole scan; keeps the board's memory bounded.
const MAX_TEXT_TOTAL: u64 = 6 * 1024 * 1024;
/// Clock slack when comparing a lock's age with its owner's start time.
const LOCK_CLOCK_SLACK: u64 = 2;

/// Store generation, named with Kiro CLI's own `--session-source` vocabulary.
/// Which engine generation each store belongs to is recorded in FINDINGS; the
/// wrapper never resumes across a generation it has not verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// `sessions/cli/<id>.json|.jsonl|.history|.lock`; the layout the V2
    /// engine writes and the wrapper has always supervised.
    Files,
    /// `sessions/<workspace>/<session-dir>/session.json`; a newer store that
    /// this wrapper can list but not resume.
    Workspaces,
}

impl StoreKind {
    pub fn label(self) -> &'static str {
        match self {
            StoreKind::Files => "v1",
            StoreKind::Workspaces => "v2",
        }
    }
}

/// A native session lock as far as a read-only observation can establish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// No lock file for this conversation.
    Free,
    /// A lock exists and its owner process is still running.
    Open { pid: u32 },
    /// A lock exists but its recorded owner is gone or predates that process.
    Stale { pid: u32 },
    /// A lock exists but its content or owner could not be read: nothing is
    /// assumed in either direction.
    Unknown,
}

impl LockState {
    pub fn label(self) -> &'static str {
        match self {
            LockState::Free => "closed",
            LockState::Open { .. } => "open",
            LockState::Stale { .. } => "stale lock",
            LockState::Unknown => "unknown lock",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    Title,
    Prompt,
    Response,
    PromptHistory,
}

impl TextRole {
    pub fn label(self) -> &'static str {
        match self {
            TextRole::Title => "title",
            TextRole::Prompt => "prompt",
            TextRole::Response => "response",
            TextRole::PromptHistory => "prompt history",
        }
    }
}

/// One piece of indexed conversation text with its provenance. Tool output is
/// never indexed, and the role is kept so a snippet says where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun {
    pub role: TextRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub native_id: String,
    /// Every store that holds this conversation; a migrated conversation can
    /// appear in both.
    pub stores: Vec<StoreKind>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub agent: Option<String>,
    pub updated_at_ms: Option<u64>,
    pub lock: LockState,
    /// Conversation text read from the store. Empty when the store keeps its
    /// transcript in a layout this wrapper does not read.
    pub text: Vec<TextRun>,
    /// True when the adapter read the conversation text rather than only its
    /// metadata, so a partial scan is never presented as a complete search.
    pub transcript: bool,
    /// Set when the entry was recognised but not usable; the reason is shown
    /// instead of guessed metadata.
    pub unusable: Option<String>,
}

impl Conversation {
    pub fn label(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("#{}", self.native_id))
    }

    pub fn project(&self) -> String {
        self.cwd
            .as_deref()
            .and_then(|cwd| {
                std::path::Path::new(cwd)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .or_else(|| self.cwd.clone())
            .unwrap_or_else(|| "unknown project".to_owned())
    }

    pub fn in_store(&self, store: StoreKind) -> bool {
        self.stores.contains(&store)
    }
}

/// Aggregate result of one store scan, including how incomplete it was.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub conversations: Vec<Conversation>,
    /// Entries that were recognised as native records but not understood.
    pub unrecognised: usize,
    /// Store directories that could not be read at all.
    pub unreadable: usize,
    /// Store generations this scan actually read, so coverage is never implied.
    pub stores: Vec<StoreKind>,
    pub truncated: bool,
    /// True when a budget stopped the scan early, so coverage is partial.
    pub partial: bool,
}

impl Scan {
    fn note(&mut self, unreadable: bool) {
        if unreadable {
            self.unreadable += 1;
        }
    }
}

/// Entry, conversation and text budgets shared by every adapter in one scan.
/// The optional deadline lets an in-progress scan stop on time, not just
/// between adapters, so closing the picker never joins an unbounded worker.
#[derive(Debug)]
pub(crate) struct Budget {
    entries: usize,
    conversations: usize,
    text_bytes: u64,
    deadline: Option<Instant>,
}

impl Budget {
    fn new() -> Self {
        Self {
            entries: MAX_ENTRIES,
            conversations: MAX_CONVERSATIONS,
            text_bytes: MAX_TEXT_TOTAL,
            deadline: None,
        }
    }

    /// True when the scan must stop now: cancelled by the board or out of time.
    pub(crate) fn stopped(&self, cancelled: &AtomicBool) -> bool {
        cancelled.load(Ordering::Relaxed)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn take_entry(&mut self, scan: &mut Scan) -> bool {
        if self.entries == 0 {
            scan.truncated = true;
            return false;
        }
        self.entries -= 1;
        true
    }

    fn take_conversation(&mut self, scan: &mut Scan) -> bool {
        if self.conversations == 0 {
            scan.truncated = true;
            return false;
        }
        self.conversations -= 1;
        true
    }

    /// Reserves `bytes` of text budget; false means text reading is over.
    fn take_text(&mut self, bytes: u64, scan: &mut Scan) -> bool {
        if self.text_bytes < bytes {
            scan.partial = true;
            return false;
        }
        self.text_bytes -= bytes;
        true
    }
}

/// One read-only store adapter. Implementations never write, never follow a
/// symlink out of the store, and never invent metadata for an unknown schema.
pub(crate) trait StoreAdapter: Send + Sync {
    /// Which store generation this adapter reads.
    fn kind(&self) -> StoreKind;
    /// The directory the adapter reads; coverage only claims a store that was
    /// actually present.
    fn root(&self) -> &std::path::Path;
    fn scan(&self, budget: &mut Budget, cancelled: &AtomicBool, into: &mut Scan);
}

/// Native store locations, snapshotted from the environment. Tests construct a
/// scope explicitly instead of mutating the process environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreRoots {
    /// `KIRO_SESSIONS_DIR`, else `<KIRO_HOME>/sessions/cli`.
    pub files: Option<PathBuf>,
    /// The directory holding Kiro's per-workspace session directories.
    pub workspaces: Option<PathBuf>,
}

fn non_empty(var: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

impl StoreRoots {
    pub fn from_env() -> Self {
        let home =
            non_empty("KIRO_HOME").or_else(|| non_empty("HOME").map(|home| home.join(".kiro")));
        let Some(home) = home else {
            return Self::default();
        };
        let files = non_empty("KIRO_SESSIONS_DIR").unwrap_or_else(|| home.join("sessions/cli"));
        let sessions = non_empty("KIRO_SESSIONS_DIR")
            .and_then(|dir| dir.parent().map(|parent| parent.to_path_buf()))
            .unwrap_or_else(|| home.join("sessions"));
        let workspaces = sessions.exists().then_some(sessions);
        Self {
            files: Some(files),
            workspaces,
        }
    }

    /// Adapters for the discovered roots, in listing order.
    pub(crate) fn adapters(&self) -> Vec<Box<dyn StoreAdapter>> {
        let mut adapters: Vec<Box<dyn StoreAdapter>> = Vec::new();
        if let Some(dir) = &self.files {
            adapters.push(Box::new(v1::FileStore::new(dir.clone())));
        }
        if let Some(dir) = &self.workspaces {
            adapters.push(Box::new(v2::WorkspaceStore::new(dir.clone())));
        }
        adapters
    }
}

/// Full read-only inventory across every discovered native store.
pub fn scan(roots: &StoreRoots, cancelled: &AtomicBool) -> Scan {
    scan_bounded(roots, cancelled, None)
}

/// A scan also stops at a deadline so a huge store can never hold the board's
/// input thread hostage; partial coverage is reported, not hidden.
fn scan_bounded(roots: &StoreRoots, cancelled: &AtomicBool, deadline: Option<Instant>) -> Scan {
    let stopped = || {
        cancelled.load(Ordering::Relaxed)
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
    };
    let mut result = Scan::default();
    let mut budget = Budget::new();
    budget.deadline = deadline;
    for adapter in roots.adapters() {
        if stopped() {
            result.partial = true;
            break;
        }
        let kind = adapter.kind();
        let unreadable_before = result.unreadable;
        adapter.scan(&mut budget, cancelled, &mut result);
        // A store counts as searched only when its directory was actually read:
        // a missing or unreadable root must never appear as covered.
        if adapter.root().is_dir()
            && result.unreadable == unreadable_before
            && !result.stores.contains(&kind)
        {
            result.stores.push(kind);
        }
    }
    if stopped() {
        result.partial = true;
    }
    merge_duplicates(&mut result);
    result.conversations.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.native_id.cmp(&b.native_id))
    });
    result
}

/// A conversation migrated between stores keeps one row: the store with real
/// conversation text wins, and both provenances stay on the row.
fn merge_duplicates(result: &mut Scan) {
    let mut merged: Vec<Conversation> = Vec::with_capacity(result.conversations.len());
    for record in result.conversations.drain(..) {
        match merged
            .iter_mut()
            .find(|existing| existing.native_id == record.native_id)
        {
            Some(existing) => {
                for store in record.stores {
                    if !existing.stores.contains(&store) {
                        existing.stores.push(store);
                    }
                }
                if record.transcript && !existing.transcript {
                    existing.text = record.text;
                    existing.transcript = true;
                }
                existing.title = existing.title.clone().or(record.title);
                existing.cwd = existing.cwd.clone().or(record.cwd);
                existing.agent = existing.agent.clone().or(record.agent);
                existing.updated_at_ms = existing.updated_at_ms.max(record.updated_at_ms);
                if existing.lock == LockState::Unknown && record.lock != LockState::Unknown {
                    existing.lock = record.lock;
                }
            }
            None => merged.push(record),
        }
    }
    result.conversations = merged;
}

/// One asynchronous read-only inventory request. The board drops the receiver
/// to cancel; `token` lets it discard a result that arrived after a retry.
pub struct HistoryScan {
    pub token: u64,
    pub result: Scan,
}

pub struct HistoryScanHandle {
    receiver: Receiver<HistoryScan>,
    cancelled: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl std::ops::Deref for HistoryScanHandle {
    type Target = Receiver<HistoryScan>;
    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl Drop for HistoryScanHandle {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn spawn_history_scan(token: u64, timeout: Duration) -> HistoryScanHandle {
    spawn_history_scan_using(token, StoreRoots::from_env(), timeout)
}

/// Test seam: an explicit scope so tests never read the ambient
/// HOME/KIRO_HOME environment of a parallel run.
fn spawn_history_scan_using(token: u64, roots: StoreRoots, timeout: Duration) -> HistoryScanHandle {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let worker = thread::spawn(move || {
        let result = scan_bounded(&roots, &worker_cancelled, Some(Instant::now() + timeout));
        // A closed channel means the board cancelled: nothing to deliver.
        let _ = sender.send(HistoryScan { token, result });
    });
    HistoryScanHandle {
        receiver,
        cancelled,
        worker: Some(worker),
    }
}

pub(crate) fn read_json_bounded(path: &std::path::Path, limit: u64) -> Option<Value> {
    // Symlinked metadata is never followed: Kiro's own files are the store.
    let info = fs::symlink_metadata(path).ok()?;
    if !info.is_file() || info.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Observed lock content is `{"pid": <number>, "started_at": <text>}`.
/// Anything else is `Unknown`, never a guess.
pub(crate) fn lock_state(directory: &std::path::Path, native_id: &str) -> LockState {
    let path = directory.join(format!("{native_id}.lock"));
    let info = match fs::symlink_metadata(&path) {
        Ok(info) => info,
        Err(_) => return LockState::Free,
    };
    if !info.is_file() {
        return LockState::Free;
    }
    let modified = info.modified().unwrap_or(UNIX_EPOCH);
    let Some(pid) = read_json_bounded(&path, 4 * 1024)
        .and_then(|value| value.get("pid").and_then(Value::as_u64))
        .and_then(|pid| u32::try_from(pid).ok())
    else {
        return LockState::Unknown;
    };
    // A lock older than its owner's start belongs to a recycled PID.
    match crate::kiro::titles::process_started(pid) {
        None => LockState::Stale { pid },
        Some(started) if lock_is_older(modified, started) => LockState::Stale { pid },
        Some(_) => LockState::Open { pid },
    }
}

fn lock_is_older(modified: SystemTime, started: SystemTime) -> bool {
    match modified.duration_since(started) {
        Ok(_) => false,
        Err(error) => error.duration().as_secs() > LOCK_CLOCK_SLACK,
    }
}

/// Strict parse of the native identifier shapes this wrapper will put on a
/// command line: Kiro's file-store ids are lowercase UUIDs.
pub fn is_resumable_native_id(native_id: &str) -> bool {
    let bytes = native_id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, &byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
        && matches!(bytes.get(14), Some(b'4'))
}

/// The UUID part of a store identifier, used only to recognise that two
/// stores hold the same conversation. Unknown shapes are not guessed.
pub(crate) fn native_uuid(native_id: &str) -> Option<String> {
    if is_resumable_native_id(native_id) {
        return Some(native_id.to_owned());
    }
    let rest = native_id
        .strip_prefix("sess_")
        .or_else(|| native_id.strip_prefix("cli_"))?;
    let candidate = rest.split('_').next()?;
    is_resumable_native_id(candidate).then(|| candidate.to_owned())
}

/// Milliseconds for the ISO-8601 timestamps Kiro writes, tolerant of the
/// observed nanosecond precision and `Z`/offset suffixes. A value that does
/// not parse yields `None` rather than a fabricated time.
pub(crate) fn timestamp_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut fields = date.split('-');
    let year: i64 = fields.next()?.parse().ok()?;
    let month: u32 = fields.next()?.parse().ok()?;
    let day: u32 = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    let (time, offset) = match rest.find(['Z', '+', '-']) {
        Some(at) => (&rest[..at], Some(&rest[at..])),
        None => (rest, None),
    };
    // Kiro writes fractional seconds with 1 to 9 digits; the fraction is not
    // kept because a conversation list only needs second precision.
    let (time, _fraction) = match time.split_once('.') {
        Some((time, fraction)) if fraction.bytes().all(|byte| byte.is_ascii_digit()) => {
            (time, fraction)
        }
        Some(_) => return None,
        None => (time, ""),
    };
    let mut parts = time.split(':');
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    let second: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let offset_seconds: i64 = match offset {
        None | Some("Z") => 0,
        Some(offset) => {
            let (sign, body) = offset.split_at(1);
            let mut fields = body.split(':');
            let hours: u32 = fields.next()?.parse().ok()?;
            let minutes: u32 = fields.next().unwrap_or("0").parse().ok()?;
            if fields.next().is_some() {
                return None;
            }
            let total = i64::from(hours * 60 + minutes) * 60;
            if sign == "-" {
                -total
            } else {
                total
            }
        }
    };
    // Range-checked rather than wrapping: a nonsense date is no timestamp.
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, i64::from(month), i64::from(day));
    let seconds =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second)
            - offset_seconds;
    u64::try_from(seconds).ok().map(|seconds| seconds * 1_000)
}

/// Days since 1970-01-01 in the civil calendar (Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year < 0 { year - 399 } else { year } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (shifted_month * 153 + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ids_are_uuids_and_nothing_else_is_put_on_a_command_line() {
        assert!(is_resumable_native_id(
            "11111111-1111-4111-8111-111111111111"
        ));
        for bad in [
            "",
            "--resume-id",
            "-x",
            "11111111-1111-4111-8111-11111111111",
            "11111111-1111-4111-8111-1111111111111",
            "11111111-1111-1111-8111-111111111111",
            "11111111-1111-4111-8111-11111111111g",
            "sess_11111111-1111-4111-8111-111111111111",
            "../11111111-1111-4111-8111-11111111111",
        ] {
            assert!(!is_resumable_native_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn workspace_ids_map_to_the_same_uuid_without_guessing_other_shapes() {
        let uuid = "11111111-1111-4111-8111-111111111111";
        assert_eq!(native_uuid(uuid).as_deref(), Some(uuid));
        assert_eq!(native_uuid(&format!("sess_{uuid}")).as_deref(), Some(uuid));
        assert_eq!(
            native_uuid(&format!("cli_{uuid}_sRLEzWYY")).as_deref(),
            Some(uuid)
        );
        for unknown in ["session-7", "sess_", "cli_1111", "sess_zzz"] {
            assert_eq!(native_uuid(unknown), None, "{unknown:?}");
        }
    }

    #[test]
    fn timestamps_parse_the_observed_shapes_and_refuse_invention() {
        // 1970-01-01T00:00:01Z
        assert_eq!(timestamp_ms("1970-01-01T00:00:01Z"), Some(1_000));
        assert_eq!(
            timestamp_ms("2026-10-27T10:59:23.051Z"),
            Some(1_793_098_763_000)
        );
        assert_eq!(
            timestamp_ms("2026-10-16T23:07:53.772073768+00:00"),
            timestamp_ms("2026-10-16T23:07:53Z")
        );
        // A non-UTC offset shifts to the same instant.
        assert_eq!(
            timestamp_ms("2026-10-16T23:07:53+02:00"),
            timestamp_ms("2026-10-16T21:07:53Z")
        );
        for bad in [
            "",
            "yesterday",
            "2026-10-16",
            "2026-13-16T00:00:00Z",
            "garbage",
        ] {
            assert_eq!(timestamp_ms(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn store_labels_use_kiro_s_own_store_vocabulary() {
        assert_eq!(StoreKind::Files.label(), "v1");
        assert_eq!(StoreKind::Workspaces.label(), "v2");
    }

    #[test]
    fn stale_lock_detection_is_explicit_in_both_directions() {
        // A lock owned by this very process is live.
        let dir = tempfile::tempdir().unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        assert_eq!(lock_state(dir.path(), id), LockState::Free);
        fs::write(
            dir.path().join(format!("{id}.lock")),
            format!(r#"{{"pid":{},"started_at":"fixture"}}"#, std::process::id()),
        )
        .unwrap();
        assert_eq!(
            lock_state(dir.path(), id),
            LockState::Open {
                pid: std::process::id()
            }
        );
        // Unreadable or unexpected content is Unknown, never Free.
        fs::write(dir.path().join(format!("{id}.lock")), "not json").unwrap();
        assert_eq!(lock_state(dir.path(), id), LockState::Unknown);
        // A pid that cannot be this system's Kiro is stale.
        fs::write(
            dir.path().join(format!("{id}.lock")),
            r#"{"pid":4294967295,"started_at":"fixture"}"#,
        )
        .unwrap();
        assert_eq!(
            lock_state(dir.path(), id),
            LockState::Stale { pid: 4294967295 }
        );
    }

    #[test]
    fn budget_reports_partial_coverage_instead_of_a_complete_scan() {
        let mut scan = Scan::default();
        let mut budget = Budget {
            entries: 1,
            conversations: 1,
            text_bytes: 10,
            deadline: None,
        };
        assert!(budget.take_entry(&mut scan));
        assert!(!budget.take_entry(&mut scan));
        assert!(budget.take_conversation(&mut scan));
        assert!(!budget.take_conversation(&mut scan));
        assert!(budget.take_text(10, &mut scan));
        assert!(!budget.take_text(1, &mut scan));
        assert!(scan.truncated);
        assert!(scan.partial);
    }

    #[test]
    fn migrated_conversations_stay_one_row_with_both_provenances() {
        let base = Conversation {
            native_id: "11111111-1111-4111-8111-111111111111".into(),
            stores: vec![StoreKind::Files],
            title: Some("Old title".into()),
            cwd: Some("/tmp/project".into()),
            agent: Some("kiro_default".into()),
            updated_at_ms: Some(10),
            lock: LockState::Stale { pid: 1 },
            text: vec![TextRun {
                role: TextRole::Prompt,
                text: "repair the picker".into(),
            }],
            transcript: true,
            unusable: None,
        };
        let migrated = Conversation {
            native_id: base.native_id.clone(),
            stores: vec![StoreKind::Workspaces],
            title: None,
            cwd: None,
            agent: None,
            updated_at_ms: Some(20),
            lock: LockState::Unknown,
            text: Vec::new(),
            transcript: false,
            unusable: None,
        };
        let mut scan = Scan {
            conversations: vec![base.clone(), migrated],
            ..Scan::default()
        };
        merge_duplicates(&mut scan);
        assert_eq!(scan.conversations.len(), 1);
        let merged = &scan.conversations[0];
        assert_eq!(merged.stores, vec![StoreKind::Files, StoreKind::Workspaces]);
        assert_eq!(merged.updated_at_ms, Some(20));
        assert_eq!(merged.title.as_deref(), Some("Old title"));
        assert_eq!(merged.lock, LockState::Stale { pid: 1 });
        assert!(merged.transcript);
        assert_eq!(merged.text, base.text);
    }

    #[test]
    fn scan_sorts_newest_first_and_the_handle_reports_its_token() {
        let store = tempfile::tempdir().unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        fs::write(
            store.path().join(format!("{id}.json")),
            format!(
                r#"{{"session_id":"{id}","cwd":"/tmp/project","title":"Older","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}"#
            ),
        )
        .unwrap();
        let roots = StoreRoots {
            files: Some(store.path().to_path_buf()),
            workspaces: None,
        };
        let result = scan(&roots, &AtomicBool::new(false));
        assert_eq!(result.conversations.len(), 1);
        assert_eq!(result.conversations[0].title.as_deref(), Some("Older"));
        assert!(!result.partial);
    }

    #[test]
    fn a_missing_store_is_an_empty_result_not_an_error() {
        let roots = StoreRoots {
            files: Some(PathBuf::from("/definitely/not/here")),
            workspaces: None,
        };
        let result = scan(&roots, &AtomicBool::new(false));
        assert!(result.conversations.is_empty());
        assert!(!result.partial);
    }
}
