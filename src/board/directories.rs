//! Read-only recursive directory discovery and a private, one-day metadata cache.
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    fs::{self, DirBuilder, OpenOptions},
    hash::{Hash, Hasher},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TTL: u64 = 24 * 60 * 60;
const MAX_DIRS: usize = 100_000;
const MAX_ENTRIES: usize = 500_000;
const CACHE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Listing {
    pub paths: Vec<String>,
    pub limited: bool,
    pub scanning: bool,
}

#[derive(Debug, Clone)]
pub(super) struct DirectoryScan(Arc<ScanOwner>);

#[derive(Debug)]
struct ScanOwner {
    receiver: Mutex<mpsc::Receiver<Listing>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for ScanOwner {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
struct Cache {
    schema: u8,
    roots: Vec<PathBuf>,
    saved_at: u64,
    limited: bool,
    paths: Vec<String>,
}

impl DirectoryScan {
    pub fn start(base: &str, force: bool) -> Self {
        let mut roots = Vec::new();
        if let Some(home) =
            std::env::var_os("HOME").and_then(|h| PathBuf::from(h).canonicalize().ok())
        {
            roots.push(home);
        }
        let base = PathBuf::from(base);
        if !roots.iter().any(|root| base.starts_with(root)) {
            roots.push(base);
        }
        let cache = crate::state_dir().ok().map(|dir| {
            let mut hasher = DefaultHasher::new();
            roots.hash(&mut hasher);
            dir.join("directory-cache")
                .join(format!("{:016x}.json", hasher.finish()))
        });
        Self::spawn(roots, cache, force)
    }

    fn spawn(roots: Vec<PathBuf>, cache: Option<PathBuf>, force: bool) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        std::thread::spawn(move || {
            if !force {
                if let Some(cached) = cache.as_deref().and_then(|path| load(path, &roots)) {
                    let fresh = now()
                        .checked_sub(cached.saved_at)
                        .is_some_and(|age| age < TTL);
                    if sender
                        .send(Listing {
                            paths: cached.paths,
                            limited: cached.limited,
                            scanning: !fresh,
                        })
                        .is_err()
                        || fresh
                    {
                        return;
                    }
                }
            }
            let listing = walk(&roots, &stop);
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if let Some(path) = cache {
                let record = Cache {
                    schema: 1,
                    roots,
                    saved_at: now(),
                    limited: listing.limited,
                    paths: listing.paths.clone(),
                };
                let _ = save(&path, &record);
            }
            let _ = sender.send(listing);
        });
        Self(Arc::new(ScanOwner {
            receiver: Mutex::new(receiver),
            cancelled,
        }))
    }

    pub fn poll(&self) -> Option<Listing> {
        self.0.receiver.lock().ok()?.try_recv().ok()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load(path: &Path, roots: &[PathBuf]) -> Option<Cache> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > CACHE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(CACHE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > CACHE_BYTES {
        return None;
    }
    let record: Cache = serde_json::from_slice(&bytes).ok()?;
    (record.schema == 1
        && record.roots == roots
        && record.paths.len() <= MAX_DIRS
        && record.paths.iter().all(|p| {
            Path::new(p).is_absolute() && roots.iter().any(|r| Path::new(p).starts_with(r))
        }))
    .then_some(record)
}

fn save(path: &Path, cache: &Cache) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let bytes = serde_json::to_vec(cache)?;
    if bytes.len() as u64 > CACHE_BYTES {
        return Ok(());
    }
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    let _ = fs::remove_file(temp);
    result
}

fn excluded(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "venv" | "__pycache__" | "backup" | "snap" | "target"
        )
}

fn walk(roots: &[PathBuf], stop: &AtomicBool) -> Listing {
    let start = Instant::now();
    let mut pending: VecDeque<PathBuf> = roots.iter().cloned().collect();
    let mut paths = Vec::new();
    let mut entries = 0;
    let mut limited = false;
    'walk: while let Some(dir) = pending.pop_front() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if start.elapsed() > Duration::from_secs(15) {
            limited = true;
            break;
        }
        let Ok(children) = fs::read_dir(&dir) else {
            continue;
        };
        for child in children.flatten() {
            if stop.load(Ordering::Relaxed) {
                break 'walk;
            }
            entries += 1;
            if entries > MAX_ENTRIES
                || paths.len() >= MAX_DIRS
                || start.elapsed() > Duration::from_secs(15)
            {
                limited = true;
                break 'walk;
            }
            let Some(name) = child.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if excluded(&name) || !child.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = child.path();
            let Some(text) = path.to_str() else {
                continue;
            };
            paths.push(text.to_owned());
            pending.push_back(path);
        }
    }
    paths.sort();
    paths.dedup();
    Listing {
        paths,
        limited,
        scanning: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn wait(scan: &DirectoryScan) -> Listing {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(listing) = scan.poll() {
                return listing;
            }
            assert!(Instant::now() < deadline, "directory discovery timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn recursive_scan_excludes_dependencies_and_never_follows_symlinks() {
        let root = tempfile::tempdir().unwrap();
        for dir in [
            "projects/team/deep/app",
            ".hidden/secret",
            "node_modules/pkg",
            "target/debug",
            "venv/bin",
            "backup/old",
        ] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        symlink(root.path(), root.path().join("loop")).unwrap();
        let listing = walk(&[root.path().to_owned()], &AtomicBool::new(false));
        assert_eq!(listing.paths.len(), 4);
        assert!(listing.paths.iter().any(|p| p.ends_with("/deep/app")));
        assert!(!listing.limited);
        assert!(walk(&[root.path().to_owned()], &AtomicBool::new(true))
            .paths
            .is_empty());
    }

    #[test]
    fn cache_reused_until_one_day_then_refreshed_and_force_bypasses_it() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(home.join("old")).unwrap();
        let roots = vec![home.clone()];
        let file = root.path().join("state/cache.json");
        let first = DirectoryScan::spawn(roots.clone(), Some(file.clone()), false);
        assert_eq!(wait(&first).paths.len(), 1);
        fs::create_dir(home.join("new")).unwrap();
        let fresh = DirectoryScan::spawn(roots.clone(), Some(file.clone()), false);
        let listing = wait(&fresh);
        assert_eq!(listing.paths.len(), 1);
        assert!(!listing.scanning);
        let mut cache = load(&file, &roots).unwrap();
        cache.saved_at = now() - TTL;
        save(&file, &cache).unwrap();
        let stale = DirectoryScan::spawn(roots.clone(), Some(file.clone()), false);
        assert!(wait(&stale).scanning);
        assert_eq!(wait(&stale).paths.len(), 2);
        fs::create_dir(home.join("forced")).unwrap();
        let forced = DirectoryScan::spawn(roots.clone(), Some(file.clone()), true);
        assert_eq!(wait(&forced).paths.len(), 3);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(load(&file, &[root.path().to_owned()]).is_none());
        fs::write(&file, b"corrupt").unwrap();
        let recovered = DirectoryScan::spawn(roots, Some(file), false);
        assert_eq!(wait(&recovered).paths.len(), 3);
    }
}
