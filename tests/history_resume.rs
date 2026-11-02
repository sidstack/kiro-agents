//! Explicit, generation-safe resume of native Kiro conversations, and the
//! rules that keep it explicit: native locks, one live child per conversation
//! id, and no replay or engine switching after a host crash.

use kiro_agents::{
    host,
    protocol::{self, Action, Reply, SessionInfo},
};
use std::{
    fs,
    io::BufReader,
    os::unix::process::CommandExt,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const NATIVE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";

struct Harness {
    dir: tempfile::TempDir,
    host: Child,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let host = spawn(dir.path());
        let harness = Self { dir, host };
        until(|| UnixStream::connect(harness.state().join("host.sock")).is_ok());
        fs::create_dir_all(harness.sessions()).unwrap();
        harness
    }
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }
    fn sessions(&self) -> PathBuf {
        self.dir.path().join("kiro/sessions/cli")
    }
    fn project(&self) -> PathBuf {
        self.dir.path().join("project")
    }
    fn call(&self, action: Action) -> Reply {
        host::call(&self.state(), action, false).unwrap()
    }
    fn attempt(&self, action: Action) -> Result<Reply, String> {
        host::call(&self.state(), action, false).map_err(|error| format!("{error:#}"))
    }
    fn rows(&self) -> Vec<SessionInfo> {
        match self.call(Action::Status) {
            Reply::Sessions(rows) => rows,
            other => panic!("Expected sessions, got {other:?}"),
        }
    }
    fn resume(&self, cwd: &Path, native: &str) -> SessionInfo {
        match self.call(Action::Resume {
            cwd: cwd.to_str().unwrap().into(),
            native_id: native.into(),
        }) {
            Reply::Created(row) => *row,
            other => panic!("Unexpected resume reply: {other:?}"),
        }
    }
    fn restart_host(&mut self) {
        let _ = self.host.kill();
        let _ = self.host.wait();
        self.host = spawn(self.dir.path());
        until(|| UnixStream::connect(self.state().join("host.sock")).is_ok());
    }
    fn attach(&self, id: &str) -> (UnixStream, BufReader<UnixStream>) {
        let mut stream = host::connect(&self.state(), false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        protocol::send(
            &mut stream,
            &protocol::request(Action::Attach {
                id: id.into(),
                cols: 80,
                rows: 24,
            }),
        )
        .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        assert!(matches!(
            protocol::receive::<Reply>(&mut reader).unwrap(),
            Reply::Ok
        ));
        (stream, reader)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = host::call(&self.state(), Action::Shutdown, false);
        let _ = self.host.kill();
        let _ = self.host.wait();
    }
}

fn spawn(dir: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_kiro-agents"))
        .arg("__host")
        .env("KIRO_AGENTS_HOME", dir.join("state"))
        .env("KIRO_HOME", dir.join("kiro"))
        .env("KIRO_SESSIONS_DIR", dir.join("kiro/sessions/cli"))
        .env("KIRO_AGENT_CONFIG_DIR", dir.join("kiro/agents"))
        .env(
            "KIRO_AGENTS_KIRO_BIN",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-kiro/kiro-cli"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn until(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "Condition timed out");
        thread::sleep(Duration::from_millis(25));
    }
}

fn output_until(reader: &mut BufReader<UnixStream>, marker: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    while !output.windows(marker.len()).any(|window| window == marker) {
        match protocol::receive::<Reply>(reader).unwrap() {
            Reply::Output(bytes) => output.extend(bytes),
            other => panic!("Unexpected {other:?}"),
        }
    }
    output
}

/// A conversation record in Kiro's own file store, as the read-only adapter
/// observes it. Nothing here is written by the wrapper.
fn native_conversation(sessions: &Path, id: &str, cwd: &Path, prompt: &str) {
    fs::create_dir_all(sessions).unwrap();
    fs::write(
        sessions.join(format!("{id}.json")),
        format!(
            r#"{{"session_id":"{id}","cwd":"{}","created_at":"2026-10-16T23:07:53Z","updated_at":"2026-10-27T10:59:37Z","title":"Repair the picker","session_created_reason":"subagent","session_state":{{"version":"v1","agent_name":"kiro_default"}}}}"#,
            cwd.display()
        ),
    )
    .unwrap();
    fs::write(sessions.join(format!("{id}.jsonl")), prompt_fixture(prompt)).unwrap();
}

fn prompt_fixture(prompt: &str) -> Vec<u8> {
    format!(
        "{{\"version\":\"v1\",\"kind\":\"Prompt\",\"data\":{{\"content\":[{{\"kind\":\"text\",\"data\":\"{prompt}\"}}]}}}}\n"
    )
        .into_bytes()
}

#[test]
fn resume_hands_kiro_the_exact_id_and_directory_and_supervises_the_child() {
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "fix ranking",
    );
    let session = harness.resume(&harness.project(), NATIVE);
    assert_eq!(session.native_id.as_deref(), Some(NATIVE));
    assert_eq!(session.generation, "v2");
    assert_eq!(session.state, "live");
    // Kiro owns the resumed profile: the wrapper passes no --agent of its own.
    assert_eq!(session.agent, None);
    let (_stream, mut reader) = harness.attach(&session.id);
    let startup = String::from_utf8_lossy(&output_until(&mut reader, b"READY:")).into_owned();
    assert!(startup.contains(&format!("resume:{NATIVE}")), "{startup}");
    assert!(startup.contains("project"), "{startup}");
    // The registry keeps identifiers only: no prompt, title source or transcript.
    let registry = fs::read_to_string(harness.state().join("registry.json")).unwrap();
    assert!(registry.contains(NATIVE));
    assert!(!registry.contains("fix ranking"));
    // The native store is untouched by the wrapper.
    let transcript = harness.sessions().join(format!("{NATIVE}.jsonl"));
    assert_eq!(
        fs::read(&transcript).unwrap(),
        prompt_fixture("fix ranking")
    );
    assert!(!harness.sessions().join(format!("{NATIVE}.lock")).exists());
}

#[test]
fn resume_needs_the_project_directory_that_kiro_recorded() {
    let harness = Harness::new();
    let gone = harness.dir.path().join("deleted-project");
    native_conversation(&harness.sessions(), NATIVE, &gone, "old work");
    let error = harness
        .attempt(Action::Resume {
            cwd: gone.to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("no longer available"), "{error}");
    // A row the store does not state a uuid for is refused before any spawn.
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: "--resume-id".into(),
        })
        .unwrap_err();
    assert!(error.contains("resume-capable"), "{error}");
    assert!(harness.rows().is_empty());
}

#[test]
fn an_open_or_unreadable_native_lock_blocks_resume() {
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    native_conversation(&harness.sessions(), NATIVE, &harness.project(), "open work");
    // A live foreign Kiro process holds the conversation.
    let mut foreign = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = foreign.id();
    fs::write(
        harness.sessions().join(format!("{NATIVE}.lock")),
        format!(r#"{{"pid":{pid},"started_at":"fixture"}}"#),
    )
    .unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(
        error.contains("already has this conversation open"),
        "{error}"
    );
    assert!(error.contains(&pid.to_string()), "{error}");
    // The wrapper never signals or cleans a foreign process.
    assert!(Path::new(&format!("/proc/{pid}")).exists());

    // An unreadable lock state stays unknown rather than assumed free.
    fs::write(
        harness.sessions().join(format!("{NATIVE}.lock")),
        "not json at all",
    )
    .unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("could not be read"), "{error}");

    // A stale lock (owner gone) is Kiro's own business: the wrapper does not
    // delete it and still lets Kiro answer.
    let stale = harness.sessions().join(format!("{NATIVE}.lock"));
    fs::write(&stale, r#"{"pid":4294967295,"started_at":"fixture"}"#).unwrap();
    let session = harness.resume(&harness.project(), NATIVE);
    assert_eq!(session.native_id.as_deref(), Some(NATIVE));
    assert!(stale.exists(), "the wrapper never writes into Kiro's store");
    harness.call(Action::Stop { id: session.id });
    let _ = foreign.kill();
    let _ = foreign.wait();
}

#[test]
fn one_live_child_per_native_conversation() {
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "single owner",
    );
    let first = harness.resume(&harness.project(), NATIVE);
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("already supervised as session"), "{error}");
    assert!(error.contains(&first.id), "{error}");
    assert_eq!(
        harness
            .rows()
            .iter()
            .filter(|row| row.native_id.as_deref() == Some(NATIVE) && row.state == "live")
            .count(),
        1
    );
}

#[test]
fn a_live_supervised_session_reports_the_conversation_kiro_states() {
    let harness = Harness::new();
    let work = fs::canonicalize(harness.dir.path()).unwrap();
    let fresh = match harness.call(Action::New {
        cwd: work.to_str().unwrap().into(),
        agent: None,
    }) {
        Reply::Created(row) => *row,
        other => panic!("Unexpected {other:?}"),
    };
    assert_eq!(fresh.native_id, None, "a fresh launch claims no history");
    let pid = fresh.pid.expect("live child has a pid");
    fs::write(
        harness.sessions().join(format!("{OTHER}.lock")),
        format!(r#"{{"pid":{pid},"started_at":"fixture"}}"#),
    )
    .unwrap();
    fs::write(
        harness.sessions().join(format!("{OTHER}.json")),
        format!(
            r#"{{"session_id":"{OTHER}","cwd":"{}","title":"Native title","session_state":{{"agent_name":"kiro_default"}}}}"#,
            work.display()
        ),
    )
    .unwrap();
    until(|| {
        harness
            .rows()
            .iter()
            .any(|row| row.native_id.as_deref() == Some(OTHER))
    });
    // The same conversation is then a duplicate to resume.
    let error = harness
        .attempt(Action::Resume {
            cwd: work.to_str().unwrap().into(),
            native_id: OTHER.into(),
        })
        .unwrap_err();
    assert!(
        error.contains("already has this conversation open")
            || error.contains("already supervised as session"),
        "{error}"
    );
}

#[test]
fn a_host_crash_never_resumes_or_replays_a_conversation() {
    let mut harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "do not repeat me",
    );
    let session = harness.resume(&harness.project(), NATIVE);
    harness.restart_host();
    let rows = harness.rows();
    let row = rows
        .iter()
        .find(|row| row.id == session.id)
        .expect("metadata survives");
    assert_eq!(row.state, "failed");
    assert_eq!(row.native_id.as_deref(), Some(NATIVE));
    assert!(row
        .notice
        .as_deref()
        .unwrap()
        .contains("Nothing was replayed"));
    // No second live child for the same conversation appeared by itself.
    assert_eq!(
        rows.iter()
            .filter(|row| row.native_id.as_deref() == Some(NATIVE) && row.state == "live")
            .count(),
        0
    );
    // And the new host does not attach to a dead process.
    let error = harness
        .attempt(Action::Attach {
            id: session.id.clone(),
            cols: 80,
            rows: 24,
        })
        .unwrap_err();
    assert!(error.contains("start a fresh session"), "{error}");
}

#[test]
fn resume_is_refused_for_a_store_this_wrapper_cannot_verify() {
    // A newer per-workspace store row has no verified transcript or lock, so
    // discovery lists it and the resume decision blocks it. That decision lives
    // in the library; the host additionally refuses an id it cannot state.
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    let workspace = harness.dir.path().join("kiro/sessions/874b7102139062fc");
    fs::create_dir_all(workspace.join(format!("sess_{NATIVE}"))).unwrap();
    fs::write(
        workspace.join(format!("sess_{NATIVE}/session.json")),
        format!(
            r#"{{"schemaVersion":"1.0.0","dataModelVersion":1,"id":"sess_{NATIVE}","title":"Newer store","agentMode":"vibe","rootPaths":["{}"],"lastModifiedAt":"2026-10-27T10:59:37Z"}}"#,
            harness.project().display()
        ),
    )
    .unwrap();
    let root = kiro_agents::kiro::history::StoreRoots {
        files: Some(harness.sessions()),
        workspaces: Some(harness.dir.path().join("kiro/sessions")),
    };
    let scan = kiro_agents::kiro::history::scan(&root, &std::sync::atomic::AtomicBool::new(false));
    assert_eq!(
        scan.stores,
        vec![
            kiro_agents::kiro::history::StoreKind::Files,
            kiro_agents::kiro::history::StoreKind::Workspaces,
        ]
    );
    let record = scan
        .conversations
        .iter()
        .find(|record| record.native_id == NATIVE)
        .expect("the newer store is discoverable");
    assert!(!record.transcript);
    let verdict = kiro_agents::kiro::history::resume::decide(record);
    assert!(verdict
        .reason()
        .unwrap()
        .contains("never resumes across a generation"));
    // The host never takes a native id from a caller it cannot verify either.
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: format!("sess_{NATIVE}"),
        })
        .unwrap_err();
    assert!(error.contains("resume-capable"), "{error}");
}

#[test]
fn resume_is_refused_for_a_directory_kiro_did_not_record_for_that_conversation() {
    // The board sends the directory it read from the store; a caller may not
    // point the same conversation somewhere else, where a resumed session
    // would run against a different project.
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    let elsewhere = harness.dir.path().join("other-project");
    fs::create_dir_all(&elsewhere).unwrap();
    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "recorded elsewhere",
    );
    let error = harness
        .attempt(Action::Resume {
            cwd: elsewhere.to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("is not that directory"), "{error}");
    assert!(
        error.contains(&harness.project().display().to_string()),
        "{error}"
    );
    assert!(
        harness.rows().is_empty(),
        "a refused resume spawns no child"
    );
    // The same conversation does resume in the directory Kiro recorded.
    let session = harness.resume(&harness.project(), NATIVE);
    assert_eq!(session.native_id.as_deref(), Some(NATIVE));
    harness.call(Action::Stop { id: session.id });
}

#[test]
fn resume_is_refused_when_only_the_newer_store_mentions_the_conversation() {
    // A per-workspace row carries the same UUID shape. Discovery lists it, the
    // library blocks it, and the host must block it too: nothing unverified
    // from another generation reaches a command line, however the id arrives.
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    let workspace = harness.dir.path().join("kiro/sessions/874b7102139062fc");
    fs::create_dir_all(workspace.join(format!("sess_{NATIVE}"))).unwrap();
    fs::write(
        workspace.join(format!("sess_{NATIVE}/session.json")),
        format!(
            r#"{{"schemaVersion":"1.0.0","dataModelVersion":1,"id":"sess_{NATIVE}","title":"Newer store","agentMode":"vibe","rootPaths":["{}"],"lastModifiedAt":"2026-10-27T10:59:37Z"}}"#,
            harness.project().display()
        ),
    )
    .unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("no readable record"), "{error}");
    assert!(
        harness.rows().is_empty(),
        "a refused resume spawns no child"
    );
    // The wrapper still wrote nothing into either native store.
    assert!(!harness.sessions().join(format!("{NATIVE}.json")).exists());
    assert!(!harness.sessions().join(format!("{NATIVE}.lock")).exists());
}

#[test]
fn an_unverifiable_record_or_an_unusual_lock_blocks_resume() {
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    // A file named after a conversation whose own record states a different
    // id is not that conversation, so nothing is resumed on it.
    fs::write(
        harness.sessions().join(format!("{NATIVE}.json")),
        format!(
            r#"{{"session_id":"{OTHER}","cwd":"{}"}}"#,
            harness.project().display()
        ),
    )
    .unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("unverified identity"), "{error}");
    assert!(
        harness.rows().is_empty(),
        "a refused resume spawns no child"
    );

    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "verified record",
    );
    // A lock that is not Kiro's regular lock file says nothing about whether a
    // Kiro process holds the conversation: resuming could duplicate one.
    let lock = harness.sessions().join(format!("{NATIVE}.lock"));
    std::os::unix::fs::symlink(harness.project(), &lock).unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    assert!(harness.rows().is_empty(), "an unusual lock spawns no child");
    // A lock path that is not a regular file (here a directory in the lock's
    // place) also fails closed instead of reading as unlocked.
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    let error = harness
        .attempt(Action::Resume {
            cwd: harness.project().to_str().unwrap().into(),
            native_id: NATIVE.into(),
        })
        .unwrap_err();
    assert!(error.contains("could not be read"), "{error}");
    // The wrapper never removes or replaces what it found in Kiro's store.
    assert!(lock.is_dir(), "the wrapper never edits a native lock");
    let _ = fs::remove_dir(&lock);
}

#[test]
fn registry_and_socket_stay_metadata_only_and_private() {
    let harness = Harness::new();
    fs::create_dir_all(harness.project()).unwrap();
    native_conversation(
        &harness.sessions(),
        NATIVE,
        &harness.project(),
        "secret-ish prompt text",
    );
    let session = harness.resume(&harness.project(), NATIVE);
    let registry = fs::read_to_string(harness.state().join("registry.json")).unwrap();
    for secret in ["secret-ish", "Repair the picker"] {
        assert!(!registry.contains(secret), "{secret} leaked into metadata");
    }
    assert_eq!(
        fs::metadata(harness.state()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    harness.call(Action::Stop { id: session.id });
}
