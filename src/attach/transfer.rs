//! Cooperative frontend handoff. The host and native Kiro process are untouched.
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const RELEASE_WAIT: Duration = Duration::from_secs(4);
const FRAME_LIMIT: u64 = 4096;

#[derive(Serialize, Deserialize)]
struct Lease {
    version: u8,
    id: String,
    token: String,
    socket: String,
}

struct State {
    stop: AtomicBool,
    release: Mutex<Option<Instant>>,
}

pub(super) struct Registration {
    dir: PathBuf,
    key: String,
    lease: Lease,
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}

fn key(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn lock(dir: &Path, key: &str) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(format!("{key}.lock")))?;
    file.lock_exclusive()?;
    Ok(file)
}

fn read_lease(dir: &Path, key: &str) -> Result<Lease> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(format!("{key}.json")))?;
    let mut bytes = Vec::new();
    file.take(FRAME_LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= FRAME_LIMIT,
        "Frontend registration exceeds limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn valid(lease: &Lease, id: &str) -> bool {
    lease.version == 1
        && lease.id == id
        && lease.token.len() == 32
        && lease.token.bytes().all(|b| b.is_ascii_hexdigit())
        && lease.socket == format!("{}.sock", &lease.token[..12])
}

fn receive(stream: &mut UnixStream) -> Result<Lease> {
    let mut bytes = Vec::new();
    BufReader::new(stream.take(FRAME_LIMIT + 1)).read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= FRAME_LIMIT && bytes.ends_with(b"\n"),
        "Invalid handoff frame"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

impl Registration {
    /// Called only after host ACK proves this frontend owns the attachment.
    pub fn new(state_dir: &Path, id: &str) -> Result<Self> {
        let dir = state_dir.join("frontends");
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        anyhow::ensure!(
            fs::symlink_metadata(&dir)?.is_dir(),
            "Frontend directory is not a real directory"
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let lease = Lease {
            version: 1,
            id: id.into(),
            socket: format!("{}.sock", &token[..12]),
            token,
        };
        let listener = UnixListener::bind(dir.join(&lease.socket))?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            release: Mutex::new(None),
        });
        let mut registration = Self {
            key: key(id),
            dir,
            lease,
            state,
            thread: None,
        };
        fs::set_permissions(
            registration.dir.join(&registration.lease.socket),
            fs::Permissions::from_mode(0o600),
        )?;
        listener.set_nonblocking(true)?;
        {
            let _lock = lock(&registration.dir, &registration.key)?;
            let temp = registration
                .dir
                .join(format!("{}.tmp", registration.lease.token));
            let result = (|| -> Result<()> {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temp)?;
                serde_json::to_writer(&mut file, &registration.lease)?;
                file.flush()?;
                fs::rename(
                    &temp,
                    registration.dir.join(format!("{}.json", registration.key)),
                )?;
                Ok(())
            })();
            let _ = fs::remove_file(temp);
            result?;
        }
        let state = registration.state.clone();
        let id = id.to_owned();
        let token = registration.lease.token.clone();
        registration.thread = Some(
            thread::Builder::new()
                .name("frontend-handoff".into())
                .spawn(move || {
                    while !state.stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                let result = (|| -> Result<()> {
                                    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                                    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                                    let request = receive(&mut stream)?;
                                    anyhow::ensure!(
                                        valid(&request, &id) && request.token == token,
                                        "Stale frontend registration"
                                    );
                                    let mut release = state
                                        .release
                                        .lock()
                                        .map_err(|_| anyhow::anyhow!("Handoff lock poisoned"))?;
                                    stream.write_all(b"ok\n")?;
                                    *release = Some(Instant::now() + RELEASE_WAIT);
                                    Ok(())
                                })();
                                if result.is_err() {
                                    let _ = stream.write_all(b"rejected\n");
                                }
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(20))
                            }
                            Err(_) => break,
                        }
                    }
                })?,
        );
        Ok(registration)
    }

    pub fn requested(&self) -> bool {
        self.state
            .release
            .lock()
            .ok()
            .and_then(|deadline| *deadline)
            .is_some_and(|deadline| Instant::now() < deadline)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(_lock) = lock(&self.dir, &self.key) {
            if read_lease(&self.dir, &self.key).is_ok_and(|lease| lease.token == self.lease.token) {
                let _ = fs::remove_file(self.dir.join(format!("{}.json", self.key)));
            }
        }
        let _ = fs::remove_file(self.dir.join(&self.lease.socket));
    }
}

/// User-confirmed release only: never signals processes or stops a native child.
pub(super) fn release(state_dir: &Path, id: &str) -> Result<()> {
    let dir = state_dir.join("frontends");
    let key = key(id);
    let lease = read_lease(&dir, &key).context("The other terminal cannot accept a move yet. Reattach it once using the updated frontend; no host restart is needed")?;
    anyhow::ensure!(
        valid(&lease, id),
        "Invalid frontend registration; nothing was detached"
    );
    let mut stream = UnixStream::connect(dir.join(&lease.socket))
        .context("The other frontend is unavailable; nothing was detached")?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    serde_json::to_writer(&mut stream, &lease)?;
    stream.write_all(b"\n")?;
    let mut response = [0; 3];
    stream.read_exact(&mut response)?;
    anyhow::ensure!(
        &response == b"ok\n",
        "The other frontend rejected this move"
    );
    let deadline = Instant::now() + RELEASE_WAIT;
    loop {
        match read_lease(&dir, &key) {
            Ok(current) if current.token != lease.token => {
                anyhow::bail!("Session moved to another terminal during handoff; select it again")
            }
            Err(_) if !dir.join(&lease.socket).exists() => return Ok(()),
            _ => (),
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Other terminal did not finish detaching; Kiro is still running"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_is_explicit_and_cleanup_preserves_a_successor_registration() {
        let state = tempfile::tempdir().unwrap();
        let old = Registration::new(state.path(), "session").unwrap();
        assert!(!old.requested());
        // A successor's metadata cannot be removed by a late predecessor drop.
        let new = Registration::new(state.path(), "session").unwrap();
        drop(old);
        assert_eq!(
            read_lease(&new.dir, &new.key).unwrap().token,
            new.lease.token
        );
        let path = state.path().to_owned();
        let mover = thread::spawn(move || release(&path, "session"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !new.requested() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let socket = new.dir.join(&new.lease.socket);
        drop(new);
        mover.join().unwrap().unwrap();
        assert!(!socket.exists());
    }

    #[test]
    fn malformed_or_missing_registration_never_detaches_anything() {
        let state = tempfile::tempdir().unwrap();
        assert!(release(state.path(), "old frontend").is_err());
        let registration = Registration::new(state.path(), "real").unwrap();
        let mut socket =
            UnixStream::connect(registration.dir.join(&registration.lease.socket)).unwrap();
        socket.write_all(b"{}\n").unwrap();
        let mut reply = [0; 9];
        socket.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"rejected\n");
        assert!(!registration.requested());
        let mut stale =
            UnixStream::connect(registration.dir.join(&registration.lease.socket)).unwrap();
        let request = Lease {
            version: 1,
            id: "real".into(),
            token: "0".repeat(32),
            socket: format!("{}.sock", "0".repeat(12)),
        };
        serde_json::to_writer(&mut stale, &request).unwrap();
        stale.write_all(b"\n").unwrap();
        stale.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"rejected\n");
        assert!(!registration.requested());
        assert!(release(state.path(), "wrong").is_err());
    }
}
