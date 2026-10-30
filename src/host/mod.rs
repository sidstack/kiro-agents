use crate::{
    kiro,
    protocol::{self, Action, Reply, Request, SessionInfo, VERSION},
};
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const RING_BYTES: usize = 2 * 1024 * 1024;
const MAX_SESSIONS: usize = 8;
type Shared = Arc<Mutex<Host>>;

struct Session {
    info: SessionInfo,
    pty: Option<Box<dyn MasterPty + Send>>,
    writer: Option<SyncSender<Vec<u8>>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    ring: VecDeque<u8>,
    subscriber: Option<SyncSender<Vec<u8>>>,
    attached: bool,
    output_done: bool,
}

struct Host {
    dir: PathBuf,
    sessions: BTreeMap<String, Session>,
    features: Vec<Box<dyn crate::features::SessionFeature>>,
}

impl Host {
    fn save(&self) -> Result<()> {
        let rows: Vec<_> = self.sessions.values().map(|s| &s.info).collect();
        let bytes = serde_json::to_vec_pretty(&rows)?;
        anyhow::ensure!(bytes.len() <= RING_BYTES, "Registry exceeds 2 MiB");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(self.dir.join("registry.tmp"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(
            self.dir.join("registry.tmp"),
            self.dir.join("registry.json"),
        )?;
        File::open(&self.dir)?.sync_all()?;
        Ok(())
    }

    fn new_session(
        &mut self,
        cwd: String,
        agent: Option<String>,
    ) -> Result<(SessionInfo, Box<dyn Read + Send>)> {
        let cwd = fs::canonicalize(cwd)?;
        anyhow::ensure!(cwd.is_dir(), "Project path is not a directory");
        let cwd = cwd
            .to_str()
            .context("Project path must be UTF-8")?
            .to_owned();
        anyhow::ensure!(
            self.sessions.values().filter(|s| s.child.is_some()).count() < MAX_SESSIONS,
            "MVP limit: {MAX_SESSIONS} live sessions"
        );
        // Fresh launches only. Wrapper IDs never select or resume Kiro history.
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let pair = native_pty_system().openpty(size(80, 24))?;
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let child = pair
            .slave
            .spawn_command(kiro::launch(&cwd, agent.as_deref()))?;
        drop(pair.slave);
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(16);
        thread::spawn(move || {
            let mut writer = writer;
            for bytes in input_rx {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        let info = SessionInfo {
            activity: Default::default(),
            name: None,
            title: None,
            title_source: None,
            id: id.clone(),
            cwd,
            agent,
            generation: "v2".into(),
            pid: child.process_id(),
            state: "live".into(),
            exit_code: None,
            notice: None,
        };
        self.sessions.insert(
            id.clone(),
            Session {
                info: info.clone(),
                pty: Some(pair.master),
                writer: Some(input_tx),
                child: Some(child),
                ring: VecDeque::new(),
                subscriber: None,
                attached: false,
                output_done: false,
            },
        );
        if let Err(error) = self.save() {
            if let Some(session) = self.sessions.get_mut(&id) {
                if let Some(child) = session.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            self.sessions.remove(&id);
            return Err(error);
        }
        Ok((info, reader))
    }

    fn rename(&mut self, id: &str, name: Option<String>) -> Result<()> {
        let name = name
            .map(|name| crate::features::validate_name(&name))
            .transpose()?;
        let session = self.sessions.get_mut(id).context("Unknown session ID")?;
        let previous = std::mem::replace(&mut session.info.name, name);
        if let Err(error) = self.save() {
            if let Some(session) = self.sessions.get_mut(id) {
                session.info.name = previous;
            }
            return Err(error);
        }
        Ok(())
    }

    fn refresh_titles(&mut self) -> Result<()> {
        let mut changed = false;
        for session in self
            .sessions
            .values_mut()
            .filter(|session| session.child.is_some())
        {
            if session.info.name.is_some() {
                continue;
            }
            for feature in &self.features {
                if let Some(title) = feature.title(&session.info) {
                    if session.info.title.as_deref() != Some(&title.text)
                        || session.info.title_source.as_deref() != Some(title.source)
                    {
                        session.info.title = Some(title.text);
                        session.info.title_source = Some(title.source.to_owned());
                        changed = true;
                    }
                    break;
                }
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    fn stop(&mut self, id: &str) -> Result<()> {
        let session = self.sessions.get_mut(id).context("Unknown session ID")?;
        if let Some(child) = session.child.as_mut() {
            let status = terminate(child.as_mut())?;
            session.info.exit_code = Some(status.exit_code());
        }
        session.child = None;
        session.writer = None;
        session.pty = None;
        session.info.state = "stopped".into();
        self.save()
    }
}

pub fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.clamp(1, 1000),
        rows: rows.clamp(1, 1000),
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub fn connect(dir: &Path, start: bool) -> Result<UnixStream> {
    let path = dir.join("host.sock");
    match UnixStream::connect(&path) {
        Ok(stream) => return Ok(stream),
        Err(error) if !start => return Err(error.into()),
        Err(_) => (),
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("__host")
        .env("KIRO_AGENTS_HOME", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir("/");
    // SAFETY: setsid is async-signal-safe and touches no Rust state after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut launcher = command.spawn().context("Could not start host")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(stream) = UnixStream::connect(&path) {
            // Reap a competing launcher if it exited; the successful daemon stays alive.
            let _ = launcher.try_wait();
            return Ok(stream);
        }
        if let Some(status) = launcher.try_wait()? {
            if !status.success() {
                anyhow::bail!(
                    "Host startup failed ({status}); run kiro-agents __host to see the error"
                );
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(anyhow!("Host startup timed out"))
}

pub fn call(dir: &Path, action: Action, start: bool) -> Result<Reply> {
    let mut stream = connect(dir, start)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    protocol::send(&mut stream, &protocol::request(action))?;
    let reply = protocol::receive(&mut BufReader::new(stream))?;
    match reply {
        Reply::Error(message) => Err(anyhow!(message)),
        other => Ok(other),
    }
}

pub fn run(dir: PathBuf) -> Result<()> {
    fs::create_dir_all(&dir)?;
    let metadata = fs::symlink_metadata(&dir)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "State directory must not be a symlink"
    );
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("host.lock"))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(()); // Another launcher already owns the host.
    }
    let socket = dir.join("host.sock");
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let registry = dir.join("registry.json");
    let rows: Vec<SessionInfo> = if registry.exists() {
        anyhow::ensure!(
            fs::metadata(&registry)?.len() <= RING_BYTES as u64,
            "Registry too large"
        );
        serde_json::from_slice(&fs::read(registry)?)?
    } else {
        Vec::new()
    };
    let mut host = Host {
        dir,
        sessions: BTreeMap::new(),
        features: crate::session_features()?,
    };
    for mut info in rows {
        if info.state == "live" {
            info.state = "failed".into();
            info.notice = Some("Previous host ended during this session; review Kiro history before continuing. Nothing was replayed.".into());
        }
        host.sessions.insert(
            info.id.clone(),
            Session {
                info,
                pty: None,
                writer: None,
                child: None,
                ring: VecDeque::new(),
                subscriber: None,
                attached: false,
                output_done: true,
            },
        );
    }
    host.save()?;
    let shared = Arc::new(Mutex::new(host));
    let connections = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let stream = stream?;
        if connections.fetch_add(1, Ordering::SeqCst) >= 32 {
            connections.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (shared, connections) = (shared.clone(), connections.clone());
        thread::spawn(move || {
            if let Err(error) = handle(stream, shared) {
                tracing::debug!(%error, "Connection ended");
            }
            connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

fn handle(mut stream: UnixStream, shared: Shared) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let request: Request = protocol::receive(&mut reader)?;
    if request.version != VERSION {
        protocol::send(
            &mut stream,
            &Reply::Error("Protocol mismatch; stop the old host explicitly".into()),
        )?;
        return Ok(());
    }
    let result: Result<Reply> = (|| match request.action {
        Action::Status => {
            let mut host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
            host.refresh_titles()?;
            Ok(Reply::Sessions(
                host.sessions.values().map(|s| s.info.clone()).collect(),
            ))
        }
        Action::New { cwd, agent } => {
            let (info, pty_reader) = shared
                .lock()
                .map_err(|_| anyhow!("Host lock poisoned"))?
                .new_session(cwd, agent)?;
            supervise(shared.clone(), info.id.clone(), pty_reader);
            Ok(Reply::Created(Box::new(info)))
        }
        Action::Rename { id, name } => {
            shared
                .lock()
                .map_err(|_| anyhow!("Host lock poisoned"))?
                .rename(&id, name)?;
            Ok(Reply::Ok)
        }
        Action::Stop { id } => {
            shared
                .lock()
                .map_err(|_| anyhow!("Host lock poisoned"))?
                .stop(&id)?;
            Ok(Reply::Ok)
        }
        Action::Shutdown => {
            let mut host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
            let ids: Vec<_> = host.sessions.keys().cloned().collect();
            for id in ids {
                host.stop(&id)?;
            }
            protocol::send(&mut stream, &Reply::Ok)?;
            fs::remove_file(host.dir.join("host.sock"))?;
            std::process::exit(0);
        }
        Action::Attach { id, cols, rows } => {
            attach(&mut stream, &mut reader, shared.clone(), &id, cols, rows)?;
            Ok(Reply::End)
        }
        _ => Err(anyhow!("Expected a command")),
    })();
    protocol::send(
        &mut stream,
        &result.unwrap_or_else(|error| Reply::Error(format!("{error:#}"))),
    )?;
    Ok(())
}

fn supervise(shared: Shared, id: String, mut reader: Box<dyn Read + Send>) {
    let (output_shared, output_id) = (shared.clone(), id.clone());
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let Ok(mut host) = output_shared.lock() else {
                break;
            };
            let Some(session) = host.sessions.get_mut(&output_id) else {
                break;
            };
            session.ring.extend(&buffer[..n]);
            if session.ring.len() > RING_BYTES {
                session.ring.drain(..session.ring.len() - RING_BYTES);
            }
            if let Some(subscriber) = &session.subscriber {
                if subscriber.try_send(buffer[..n].to_vec()).is_err() {
                    session.subscriber = None;
                }
            }
        }
        if let Ok(mut host) = output_shared.lock() {
            if let Some(session) = host.sessions.get_mut(&output_id) {
                session.output_done = true;
            }
        }
    });
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(50));
        let Ok(mut host) = shared.lock() else {
            break;
        };
        let Some(session) = host.sessions.get_mut(&id) else {
            break;
        };
        let Some(child) = session.child.as_mut() else {
            break;
        };
        match child.try_wait() {
            Ok(None) => continue,
            Ok(Some(status)) => {
                session.info.exit_code = Some(status.exit_code());
                session.info.state = if status.success() { "exited" } else { "failed" }.into();
            }
            Err(_) => {
                session.info.state = "failed".into();
                session.info.notice = Some("Unable to reap child; no automatic retry".into());
            }
        }
        session.child = None;
        session.writer = None;
        session.pty = None;
        if let Err(error) = host.save() {
            tracing::error!(%error, "Could not persist child exit");
        }
        break;
    });
}

fn attach(
    stream: &mut UnixStream,
    reader: &mut BufReader<UnixStream>,
    shared: Shared,
    id: &str,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let (tx, rx) = mpsc::sync_channel(256);
    let replay = {
        let mut host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
        let session = host.sessions.get_mut(id).context("Unknown session ID")?;
        anyhow::ensure!(
            session.child.is_some(),
            "Session is {}; start a fresh session or use Kiro history manually",
            session.info.state
        );
        anyhow::ensure!(
            !session.attached,
            "Session already attached; detach the other terminal first"
        );
        session
            .pty
            .as_ref()
            .context("No PTY")?
            .resize(size(cols, rows))?;
        let replay: Vec<u8> = session.ring.iter().copied().collect();
        session.subscriber = Some(tx);
        session.attached = true;
        replay
    };
    // The guard also releases the single attachment on EOF, errors, or slow consumers.
    struct Attachment(Shared, String);
    impl Drop for Attachment {
        fn drop(&mut self) {
            if let Ok(mut host) = self.0.lock() {
                if let Some(session) = host.sessions.get_mut(&self.1) {
                    session.attached = false;
                    session.subscriber = None;
                }
            }
        }
    }
    let _guard = Attachment(shared.clone(), id.to_owned());
    protocol::send(stream, &Reply::Ok)?;
    for chunk in replay.chunks(4096) {
        protocol::send(stream, &Reply::Output(chunk.to_vec()))?;
    }
    {
        let host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
        if let Some(child) = host.sessions.get(id).and_then(|s| s.child.as_ref()) {
            signal_owned(child.as_ref(), libc::SIGWINCH)?;
        }
    }
    reader.get_mut().set_read_timeout(None)?;
    loop {
        while let Ok(bytes) = rx.try_recv() {
            protocol::send(stream, &Reply::Output(bytes))?;
        }
        {
            let host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
            let session = host.sessions.get(id).context("Unknown session")?;
            if session.subscriber.is_none() {
                anyhow::bail!("Attachment fell behind; reconnect to replay retained output");
            }
            if session.child.is_none() && session.output_done {
                // Output may have arrived after the queue was drained above.
                while let Ok(bytes) = rx.try_recv() {
                    protocol::send(stream, &Reply::Output(bytes))?;
                }
                return Ok(());
            }
        }
        // Poll rather than timing out halfway through a JSON frame.
        if reader.buffer().is_empty() && !crate::attach::readable(reader.get_ref(), 20)? {
            continue;
        }
        let request: Request = protocol::receive(reader)?;
        anyhow::ensure!(request.version == VERSION, "Protocol mismatch");
        let mut host = shared.lock().map_err(|_| anyhow!("Host lock poisoned"))?;
        let session = host.sessions.get_mut(id).context("Unknown session")?;
        match request.action {
            Action::Input(bytes) => {
                anyhow::ensure!(bytes.len() <= 4096, "Input too large");
                session
                    .writer
                    .as_mut()
                    .context("Session exited")?
                    .try_send(bytes)
                    .context("Session input stalled; reconnect when Kiro accepts input")?;
            }
            Action::Resize { cols, rows } => session
                .pty
                .as_ref()
                .context("Session exited")?
                .resize(size(cols, rows))?,
            Action::Detach => return Ok(()),
            _ => anyhow::bail!("Unexpected attached command"),
        }
    }
}

// Only call while holding an unreaped Child handle, never using a persisted PID.
fn signal_owned(child: &dyn Child, signal: i32) -> Result<()> {
    let pid = child.process_id().context("Owned child has no PID")?;
    // SAFETY: portable-pty starts the child as its own process group leader.
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error.into());
        }
    }
    Ok(())
}

fn terminate(child: &mut dyn Child) -> Result<portable_pty::ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    signal_owned(child, libc::SIGTERM)?;
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(10));
    }
    signal_owned(child, libc::SIGKILL)?;
    Ok(child.wait()?)
}
