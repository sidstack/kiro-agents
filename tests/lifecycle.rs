use kiro_agents::{
    host,
    protocol::{self, Action, Reply, SessionInfo},
};
use std::{
    fs,
    io::BufReader,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Harness {
    dir: tempfile::TempDir,
    host: Child,
}
impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let host = spawn(dir.path());
        let harness = Self { dir, host };
        harness.wait_socket();
        harness
    }
    fn wait_socket(&self) {
        until(|| UnixStream::connect(self.state().join("host.sock")).is_ok());
    }
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }
    fn call(&self, action: Action) -> Reply {
        host::call(&self.state(), action, false).unwrap()
    }
    fn new_session(&self, agent: Option<&str>) -> SessionInfo {
        match self.call(Action::New {
            cwd: self.dir.path().to_str().unwrap().into(),
            agent: agent.map(str::to_owned),
        }) {
            Reply::Created(info) => info,
            other => panic!("Unexpected: {other:?}"),
        }
    }
    fn rows(&self) -> Vec<SessionInfo> {
        match self.call(Action::Status) {
            Reply::Sessions(rows) => rows,
            _ => panic!("Expected sessions"),
        }
    }
    fn attach(&self, id: &str) -> (UnixStream, BufReader<UnixStream>) {
        let mut stream = host::connect(&self.state(), false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        protocol::send(
            &mut stream,
            &protocol::request(Action::Attach {
                id: id.into(),
                cols: 103,
                rows: 31,
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
    while !output.windows(marker.len()).any(|w| w == marker) {
        match protocol::receive::<Reply>(reader).unwrap() {
            Reply::Output(bytes) => output.extend(bytes),
            other => panic!("Unexpected {other:?}"),
        }
    }
    output
}

#[test]
fn detach_reconnect_drain_resize_and_stop_owned_child() {
    let harness = Harness::new();
    let session = harness.new_session(Some("reviewer"));
    let (mut stream, mut reader) = harness.attach(&session.id);
    let startup = output_until(&mut reader, b"READY:");
    assert!(String::from_utf8_lossy(&startup).contains(":reviewer"));
    let duplicate = host::call(
        &harness.state(),
        Action::Attach {
            id: session.id.clone(),
            cols: 80,
            rows: 24,
        },
        false,
    );
    assert!(duplicate
        .unwrap_err()
        .to_string()
        .contains("already attached"));
    protocol::send(
        &mut stream,
        &protocol::request(Action::Input(b"\x03\xff\xe6\x97\xa5".to_vec())),
    )
    .unwrap();
    output_until(&mut reader, b"BYTES:03ffe697a5");
    protocol::send(
        &mut stream,
        &protocol::request(Action::Resize {
            cols: 121,
            rows: 43,
        }),
    )
    .unwrap();
    output_until(&mut reader, b"SIZE:121x43");
    // Disconnect mid-output. The host must drain >2 MiB without a frontend.
    protocol::send(
        &mut stream,
        &protocol::request(Action::Input(b"flood".to_vec())),
    )
    .unwrap();
    drop(stream);
    drop(reader);
    thread::sleep(Duration::from_millis(300));
    let (mut stream, mut reader) = harness.attach(&session.id);
    let replay = output_until(&mut reader, b"FLOOD_DONE");
    assert!(replay.len() <= 2 * 1024 * 1024 + 4096);
    assert_eq!(harness.rows()[0].pid, session.pid);
    let registry = fs::read_to_string(harness.state().join("registry.json")).unwrap();
    assert!(
        !registry.contains("READY") && !registry.contains("BYTES") && !registry.contains("FLOOD")
    );
    assert_eq!(
        fs::metadata(harness.state()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(harness.state().join("host.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    protocol::send(
        &mut stream,
        &protocol::request(Action::Input(b"exit".to_vec())),
    )
    .unwrap();
    output_until(&mut reader, b"GOODBYE");
    until(|| harness.rows()[0].state == "exited");
    let second = harness.new_session(None);
    harness.call(Action::Stop {
        id: second.id.clone(),
    });
    assert!(harness
        .rows()
        .iter()
        .any(|s| s.id == second.id && s.state == "stopped"));
}

#[test]
fn crash_recovers_metadata_without_spawning_or_replaying() {
    let mut harness = Harness::new();
    let original = harness.new_session(None);
    let mut competitor = spawn(harness.dir.path());
    assert!(competitor.wait().unwrap().success());
    assert_eq!(harness.rows().len(), 1);
    harness.host.kill().unwrap();
    harness.host.wait().unwrap();
    harness.host = spawn(harness.dir.path());
    harness.wait_socket();
    until(|| harness.rows()[0].state == "failed");
    let rows = harness.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pid, original.pid);
    assert!(rows[0]
        .notice
        .as_ref()
        .unwrap()
        .contains("Nothing was replayed"));
    assert!(host::call(
        &harness.state(),
        Action::Attach {
            id: original.id,
            cols: 80,
            rows: 24
        },
        false
    )
    .is_err());
}

#[test]
fn missing_agent_exit_is_visible_and_invalid_cwd_does_not_spawn() {
    let harness = Harness::new();
    let missing = harness.new_session(Some("missing"));
    until(|| harness.rows()[0].state == "failed");
    assert_eq!(harness.rows()[0].exit_code, Some(4));
    assert_eq!(harness.rows()[0].id, missing.id);
    assert!(host::call(
        &harness.state(),
        Action::New {
            cwd: "/nonexistent/kiro-mvp".into(),
            agent: None
        },
        false
    )
    .is_err());
    assert_eq!(harness.rows().len(), 1);
}
