//! Native names matched to an owned process tree, never directory recency.
use crate::{
    features::{clean_title, SessionFeature, SessionTitle},
    protocol::SessionInfo,
};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_FILES: usize = 10_000;
const MAX_JSON: u64 = 64 * 1024;
const MAX_RECORD: usize = 16 * 1024;
const MAX_RECORDS: usize = 8;
const MAX_ANCESTORS: usize = 12;

pub struct NativeNames {
    directory: PathBuf,
    first_message: bool,
}

impl NativeNames {
    pub fn new(directory: PathBuf, first_message: bool) -> Self {
        Self {
            directory,
            first_message,
        }
    }

    /// The native conversation this owned process holds, as stated by its own
    /// store metadata. Never inferred from recency, cwd alone or a PID alone.
    fn native_identity(
        &self,
        session: &SessionInfo,
        started: SystemTime,
    ) -> Option<(String, PathBuf, Value)> {
        let root_pid = session.pid?;
        let mut matches = Vec::new();
        let mut entries = fs::read_dir(&self.directory).ok()?;
        for entry in entries.by_ref().take(MAX_FILES) {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("lock") {
                continue;
            }
            let Ok(info) = fs::symlink_metadata(&path) else {
                continue;
            };
            let Ok(modified) = info.modified() else {
                continue;
            };
            if !info.is_file() || modified < started {
                continue;
            }
            let Some(lock) = read_json(&path) else {
                continue;
            };
            let Some(pid) = lock
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|pid| u32::try_from(pid).ok())
            else {
                continue;
            };
            let Some(native_start) = owned_process_start(root_pid, pid, started) else {
                continue;
            };
            if modified < native_start {
                continue;
            }
            let Some(meta) = read_json(&path.with_extension("json")) else {
                continue;
            };
            let Some(id) = path.file_stem().and_then(|x| x.to_str()) else {
                continue;
            };
            if meta.get("session_id").and_then(Value::as_str) != Some(id)
                || meta.get("cwd").and_then(Value::as_str) != Some(session.cwd.as_str())
            {
                continue;
            }
            matches.push((id.to_owned(), path, meta));
            if matches.len() > 1 {
                return None;
            }
        }
        if entries.next().is_some() {
            return None;
        }
        matches.pop()
    }

    fn lookup(&self, session: &SessionInfo, started: SystemTime) -> Option<SessionTitle> {
        let (native_id, lock, meta) = self.native_identity(session, started)?;
        let _ = native_id;
        if let Some(title) = meta
            .get("title")
            .and_then(Value::as_str)
            .and_then(clean_title)
        {
            return Some(SessionTitle {
                text: title,
                source: "kiro",
            });
        }
        if self.first_message {
            let title = first_message_title(&lock.with_extension("jsonl"))?;
            return Some(SessionTitle {
                text: title,
                source: "first_message",
            });
        }
        None
    }
}

impl SessionFeature for NativeNames {
    fn title(&self, session: &SessionInfo) -> Option<SessionTitle> {
        if session.state != "live" {
            return None;
        }
        self.lookup(session, process_started(session.pid?)?)
    }

    fn native_id(&self, session: &SessionInfo) -> Option<String> {
        if session.state != "live" {
            return None;
        }
        let (native_id, _, meta) = self.native_identity(session, process_started(session.pid?)?)?;
        // Only an id the store itself states is reported, and it must be a
        // shape the wrapper could one day resume.
        if meta.get("session_id").and_then(Value::as_str) == Some(native_id.as_str())
            && crate::kiro::history::is_resumable_native_id(&native_id)
        {
            Some(native_id)
        } else {
            None
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let info = fs::symlink_metadata(path).ok()?;
    if !info.is_file() || info.len() > MAX_JSON {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_JSON {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

// The clock gate excludes abandoned locks from an earlier incarnation of this PID.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    parent: u32,
    started: SystemTime,
}

// Walk upward from the lock owner: children spawned on runtime threads are absent
// from /proc/<root>/task/<root>/children. Never adopt a process by cwd or recency.
fn owned_process_start(root: u32, native: u32, root_start: SystemTime) -> Option<SystemTime> {
    owned_process_start_using(root, native, root_start, process_identity)
}

fn owned_process_start_using(
    root: u32,
    native: u32,
    root_start: SystemTime,
    read: impl Fn(u32) -> Option<ProcessIdentity>,
) -> Option<SystemTime> {
    let identity = read(native)?;
    let mut pid = native;
    let mut child_start = identity.started;
    for _ in 0..MAX_ANCESTORS {
        let current = read(pid)?;
        if current.started < root_start || current.started > child_start {
            return None;
        }
        if pid == root {
            // Recheck identities before trusting a chain that could have exited,
            // reparented or reused a PID while /proc was being traversed.
            if current.started != root_start || read(root)? != current || read(native)? != identity
            {
                return None;
            }
            return Some(identity.started);
        }
        if current.parent == 0 || current.parent == pid {
            return None;
        }
        child_start = current.started;
        pid = current.parent;
    }
    None
}

pub(crate) fn process_started(pid: u32) -> Option<SystemTime> {
    Some(process_identity(pid)?.started)
}

fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let parent = fields.get(1)?.parse().ok()?;
    let &(boot, hz) = process_clock()?;
    let started = UNIX_EPOCH
        .checked_add(Duration::from_secs(boot + ticks / hz))?
        .checked_add(Duration::from_nanos((ticks % hz) * 1_000_000_000 / hz))?;
    Some(ProcessIdentity { parent, started })
}

fn process_clock() -> Option<&'static (u64, u64)> {
    // These values belong to this boot, not to individual processes. Avoid
    // rereading every CPU's /proc/stat counters at each ancestor/refresh.
    static CLOCK: OnceLock<(u64, u64)> = OnceLock::new();
    if let Some(clock) = CLOCK.get() {
        return Some(clock);
    }
    // SAFETY: querying a process-independent clock constant has no side effects.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return None;
    }
    let boot = fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(CLOCK.get_or_init(|| (boot, hz as u64)))
}

fn first_message_title(path: &Path) -> Option<String> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut reader = BufReader::new(File::open(path).ok()?);
    // Only whole records inside the size bound are parsed. An oversized record
    // is skipped and the scan resynchronises on its terminating newline, so one
    // long system or context record can neither abort the scan nor swallow the
    // Prompt record after it. Each read is capped, so the total stay bounded.
    let mut at_boundary = true;
    for _ in 0..MAX_RECORDS {
        let mut line = Vec::new();
        reader
            .by_ref()
            .take(MAX_RECORD as u64 + 1)
            .read_until(b'\n', &mut line)
            .ok()?;
        if line.is_empty() {
            break;
        }
        let ended_on_newline = line.last() == Some(&b'\n');
        let usable = at_boundary && ended_on_newline && line.len() <= MAX_RECORD;
        at_boundary = ended_on_newline;
        if !usable {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        // Tagged native records only: never derive a name from assistant/tool
        // output. An untagged or oddly shaped native record is skipped so a
        // schema header or newer record shape cannot hide a later Prompt.
        let Some(kind) = record.get("kind").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "Prompt") {
            continue;
        }
        let Some(content) = record
            .get("data")
            .and_then(|data| data.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let text = content
            .iter()
            .filter_map(|part| {
                let kind = part.get("kind")?.as_str()?;
                if !matches!(kind, "text") {
                    return None;
                }
                part.get("data")?.as_str()
            })
            .collect::<Vec<_>>()
            .join(" ");
        // Bounded extractive fallback; no model call or transcript persistence.
        let title = text
            .split_whitespace()
            .take(10)
            .collect::<Vec<_>>()
            .join(" ");
        return clean_title(&title);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestry_is_bounded_and_rejects_foreign_reused_or_reparented_processes() {
        let time = |seconds| UNIX_EPOCH + Duration::from_secs(seconds);
        let graph = std::collections::HashMap::from([
            (
                10,
                ProcessIdentity {
                    parent: 1,
                    started: time(1),
                },
            ),
            (
                11,
                ProcessIdentity {
                    parent: 10,
                    started: time(2),
                },
            ),
            (
                12,
                ProcessIdentity {
                    parent: 11,
                    started: time(3),
                },
            ),
            (
                13,
                ProcessIdentity {
                    parent: 12,
                    started: time(4),
                },
            ),
        ]);
        let read = |pid| graph.get(&pid).copied();
        assert_eq!(
            owned_process_start_using(10, 13, time(1), read),
            Some(time(4))
        );
        assert!(owned_process_start_using(20, 13, time(1), read).is_none());
        assert!(owned_process_start_using(10, 13, time(2), read).is_none());
        let mut stale = graph.clone();
        stale.get_mut(&12).unwrap().started = time(5);
        assert!(
            owned_process_start_using(10, 13, time(1), |pid| stale.get(&pid).copied()).is_none()
        );
        let reads = std::cell::Cell::new(0);
        assert!(owned_process_start_using(10, 13, time(1), |pid| {
            if pid == 13 {
                reads.set(reads.get() + 1);
                if reads.get() > 2 {
                    return None;
                }
            }
            read(pid)
        })
        .is_none());
        let mut deep = graph;
        for pid in 14..40 {
            deep.insert(
                pid,
                ProcessIdentity {
                    parent: pid - 1,
                    started: time(u64::from(pid)),
                },
            );
        }
        assert!(
            owned_process_start_using(10, 39, time(1), |pid| deep.get(&pid).copied()).is_none()
        );
    }

    fn session(cwd: &Path) -> SessionInfo {
        SessionInfo {
            name: None,
            title: None,
            title_source: None,
            native_id: None,
            id: "wrapper".into(),
            cwd: cwd.to_string_lossy().into_owned(),
            agent: None,
            generation: "v2".into(),
            pid: Some(std::process::id()),
            state: "live".into(),
            exit_code: None,
            notice: None,
        }
    }

    fn fixture(root: &Path, id: &str, row: &SessionInfo, title: &str) {
        fs::write(
            root.join(format!("{id}.lock")),
            serde_json::to_vec(&serde_json::json!({"pid": row.pid, "started_at": "fixture"}))
                .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(format!("{id}.json")),
            serde_json::to_vec(&serde_json::json!({"session_id":id,"cwd":row.cwd,"title":title}))
                .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn native_title_requires_the_owned_pid_directory_and_identity() {
        let root = tempfile::tempdir().unwrap();
        let row = session(root.path());
        let feature = NativeNames::new(root.path().into(), true);
        fixture(root.path(), "native", &row, "Fix auth state");
        let title = feature.title(&row).unwrap();
        assert_eq!(title.text, "Fix auth state");
        assert_eq!(title.source, "kiro");
        let mut other = row.clone();
        other.cwd.push_str("/other");
        assert!(feature.title(&other).is_none());
        other = row.clone();
        other.pid = Some(u32::MAX);
        assert!(feature.title(&other).is_none());
        fs::write(
            root.path().join("native.json"),
            r#"{"session_id":"other","cwd":"/" ,"title":"wrong"}"#,
        )
        .unwrap();
        assert!(feature.title(&row).is_none());
    }

    #[test]
    fn stale_or_ambiguous_locks_and_exited_sessions_do_not_supply_names() {
        let root = tempfile::tempdir().unwrap();
        let mut row = session(root.path());
        let feature = NativeNames::new(root.path().into(), true);
        fixture(root.path(), "one", &row, "one");
        assert!(feature
            .lookup(&row, SystemTime::now() + Duration::from_secs(1))
            .is_none());
        fixture(root.path(), "two", &row, "two");
        assert!(feature.title(&row).is_none());
        row.state = "exited".into();
        assert!(feature.title(&row).is_none());
    }

    #[test]
    fn first_prompt_fallback_is_bounded_opt_in_and_never_uses_tool_output() {
        let root = tempfile::tempdir().unwrap();
        let row = session(root.path());
        fixture(root.path(), "native", &row, "");
        let transcript = root.path().join("native.jsonl");
        fs::write(&transcript, concat!(
            "{\"kind\":\"ToolResults\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"ignore tools\"}]}}\n",
            "{\"kind\":\"Prompt\",\"data\":{\"content\":[{\"kind\":\"text\",\"data\":\"Fix the picker backspace and fuzzy selection behavior\"}]}}\n"
        )).unwrap();
        assert!(NativeNames::new(root.path().into(), false)
            .title(&row)
            .is_none());
        let title = NativeNames::new(root.path().into(), true)
            .title(&row)
            .unwrap();
        assert_eq!(
            title.text,
            "Fix the picker backspace and fuzzy selection behavior"
        );
        assert_eq!(title.source, "first_message");
        fixture(root.path(), "native", &row, "Kiro title wins");
        assert_eq!(
            NativeNames::new(root.path().into(), true)
                .title(&row)
                .unwrap()
                .text,
            "Kiro title wins"
        );
        fs::write(transcript, "x".repeat(MAX_RECORD + 1)).unwrap();
        fixture(root.path(), "native", &row, "");
        assert!(NativeNames::new(root.path().into(), true)
            .title(&row)
            .is_none());
    }

    #[test]
    fn oddly_shaped_and_oversized_records_are_skipped_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        let row = session(root.path());
        fixture(root.path(), "native", &row, "");
        let transcript = root.path().join("native.jsonl");
        let prompt = |text: &str| {
            format!(
                "{{\"kind\":\"Prompt\",\"data\":{{\"content\":[{{\"kind\":\"text\",\"data\":\"{text}\"}}]}}}}\n"
            )
        };

        // Untagged and unexpectedly shaped native records must not end the scan.
        fs::write(
            &transcript,
            concat!(
                "{\"schemaVersion\":1}\n",
                "{\"kind\":null,\"data\":\"not a tag\"}\n",
                "{\"kind\":\"Prompt\",\"data\":\"unexpected shape\"}\n",
            ),
        )
        .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(prompt("Fix title derivation for odd records").as_bytes())
            })
            .unwrap();
        let title = NativeNames::new(root.path().into(), true)
            .title(&row)
            .unwrap();
        assert_eq!(title.text, "Fix title derivation for odd records");
        assert_eq!(title.source, "first_message");

        // A leading record past the read bound is skipped, and the scan
        // resynchronises so the next whole record is still readable.
        let oversized = format!("{{\"blob\":\"{}\"}}\n", "x".repeat(MAX_RECORD + 4096));
        fs::write(
            &transcript,
            format!("{oversized}{}", prompt("Found after a huge record")),
        )
        .unwrap();
        let title = NativeNames::new(root.path().into(), true)
            .title(&row)
            .unwrap();
        assert_eq!(title.text, "Found after a huge record");

        // Reading stays bounded: eight capped reads, whatever the file holds.
        let beyond = format!(
            "{}{}",
            "x".repeat((MAX_RECORD + 1) * 20),
            prompt("Never reached")
        );
        fs::write(&transcript, beyond).unwrap();
        assert!(NativeNames::new(root.path().into(), true)
            .title(&row)
            .is_none());
    }

    #[test]
    fn symlinked_native_metadata_is_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let row = session(root.path());
        fixture(root.path(), "native", &row, "title");
        fs::rename(
            root.path().join("native.json"),
            root.path().join("hidden.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            root.path().join("hidden.json"),
            root.path().join("native.json"),
        )
        .unwrap();
        assert!(NativeNames::new(root.path().into(), true)
            .title(&row)
            .is_none());
    }
}
