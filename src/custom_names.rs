//! Shared persistent display metadata, independent of dashboard and host versions.
use crate::{features::validate_name, protocol::SessionInfo};
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

type Names = BTreeMap<String, Option<String>>;
const LIMIT: u64 = 512 * 1024;
#[derive(serde::Serialize, serde::Deserialize)]
struct NamesFile {
    version: u32,
    names: Names,
}

fn load(dir: &Path) -> Result<Names> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("names.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Names::new()),
        Err(error) => return Err(error).context("Cannot read custom session names"),
    };
    let info = file.metadata()?;
    anyhow::ensure!(
        info.is_file() && info.len() <= LIMIT,
        "Custom session names exceed their metadata budget"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= LIMIT,
        "Custom session names exceed their metadata budget"
    );
    let store: NamesFile =
        serde_json::from_slice(&bytes).context("Custom session names file is invalid")?;
    anyhow::ensure!(store.version == 1, "Unsupported custom names state version");
    let names = store.names;
    for (id, name) in &names {
        anyhow::ensure!(
            !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
            "Invalid session name key"
        );
        if let Some(name) = name {
            validate_name(name)?;
        }
    }
    Ok(names)
}

pub fn set(dir: &Path, id: &str, name: Option<String>) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control),
        "Invalid session ID"
    );
    let name = name.map(|name| validate_name(&name)).transpose()?;
    fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("names.lock"))?;
    lock.lock_exclusive()?;
    let mut names = load(dir)?;
    // A null entry explicitly clears a name supplied by a newer host, too.
    names.insert(id.to_owned(), name);
    let bytes = serde_json::to_vec_pretty(&NamesFile { version: 1, names })?;
    anyhow::ensure!(
        bytes.len() as u64 <= LIMIT,
        "Custom session names exceed their metadata budget"
    );
    let temporary = dir.join("names.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temporary, dir.join("names.json"))?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub fn enrich(dir: &Path, rows: &mut [SessionInfo]) -> Result<()> {
    let names = match load(dir) {
        Ok(names) => names,
        Err(_) => {
            tracing::warn!("Custom session names unavailable; using automatic labels");
            Names::new()
        }
    };
    let features = crate::session_features()?;
    for row in rows {
        if let Some(name) = names.get(&row.id) {
            row.name = name.clone();
        }
        if row.name.is_none() {
            for feature in &features {
                if let Some(title) = feature.title(row) {
                    row.title = Some(title.text);
                    row.title_source = Some(title.source.into());
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_clear_overrides_are_private_atomic_metadata() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "session", Some("Fix picker".into())).unwrap();
        assert_eq!(
            load(dir.path()).unwrap()["session"].as_deref(),
            Some("Fix picker")
        );
        assert_eq!(
            fs::metadata(dir.path().join("names.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::create_dir(dir.path().join("names.tmp")).unwrap();
        assert!(set(dir.path(), "session", Some("unsaved".into())).is_err());
        assert_eq!(
            load(dir.path()).unwrap()["session"].as_deref(),
            Some("Fix picker")
        );
        fs::remove_dir(dir.path().join("names.tmp")).unwrap();
        set(dir.path(), "session", None).unwrap();
        assert!(load(dir.path()).unwrap()["session"].is_none());
    }
    #[test]
    fn invalid_labels_and_symlink_targets_do_not_overwrite_anything() {
        let dir = tempfile::tempdir().unwrap();
        assert!(set(dir.path(), "session", Some("bad\nname".into())).is_err());
        let outside = dir.path().join("outside");
        fs::write(&outside, "do not overwrite").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("names.json")).unwrap();
        assert!(set(dir.path(), "session", Some("safe".into())).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "do not overwrite");
    }
    #[test]
    fn an_unknown_future_store_version_cannot_be_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let original = r#"{"version":2,"names":{"session":"future name"}}"#;
        fs::write(dir.path().join("names.json"), original).unwrap();
        assert!(set(dir.path(), "session", Some("replacement".into())).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("names.json")).unwrap(),
            original
        );
    }
}
