//! Where Tundra keeps its own files (cache, config, data directories) and
//! how it reads and saves them.

use crate::safe_write::{sidecar, write_atomic};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};

/// `<base>/tundra`, created if possible.
fn app_dir(base: Option<PathBuf>) -> Option<PathBuf> {
    let dir = base?.join("tundra");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Rebuildable data: directory listings, the tag index, preferences.
pub fn cache_dir() -> Option<PathBuf> {
    app_dir(dirs::cache_dir())
}

/// User settings: allowed directories, favorites.
pub fn config_dir() -> Option<PathBuf> {
    app_dir(dirs::config_dir())
}

/// Data that cannot be rebuilt (the tag store). `None` unless the directory exists.
// Tests point the tag store at a scratch database instead.
#[cfg_attr(test, allow(dead_code))]
pub fn data_dir() -> Option<PathBuf> {
    app_dir(dirs::data_dir()).filter(|dir| dir.is_dir())
}

pub fn cache_file(name: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(name))
}

pub fn config_file(name: &str) -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(name))
}

pub fn read_bincode<T: DeserializeOwned>(path: &Path) -> Option<T> {
    bincode::deserialize(&std::fs::read(path).ok()?).ok()
}

/// Saves `value` atomically, logging failures under `label`. True on success.
pub fn write_bincode<T: Serialize>(path: &Path, value: &T, label: &str) -> bool {
    let result = match bincode::serialize(value) {
        Ok(bytes) => write_atomic(path, &bytes).map_err(|err| err.to_string()),
        Err(err) => Err(err.to_string()),
    };
    result
        .inspect_err(|err| eprintln!("Failed to write {label}: {err}"))
        .is_ok()
}

/// Load user data such as settings or favorites.
///
/// A file that cannot be decoded is moved aside (`.unreadable-<ts>`) so the
/// next save cannot destroy the user's only copy. A file that exists but
/// cannot be read right now (locked by antivirus or a sync client) is left
/// alone and `writable` comes back false: the caller must not save over it
/// this session.
pub fn load_user_data<T: Default + DeserializeOwned>(path: &Path, label: &str) -> (T, bool) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return (T::default(), true),
        Err(err) => {
            eprintln!(
                "Failed to read {label} ({}): {err}; changes will not be saved this session",
                path.display()
            );
            return (T::default(), false);
        }
    };
    let err = match bincode::deserialize(&bytes) {
        Ok(value) => return (value, true),
        Err(err) => format!("Failed to load {label} ({}): {err}", path.display()),
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let backup = sidecar(path, &format!(".unreadable-{stamp}"));
    let moved = std::fs::rename(path, &backup);
    match &moved {
        Ok(()) => eprintln!("{err}. Kept the file as {}", backup.display()),
        Err(move_err) => eprintln!("{err}; could not move it aside ({move_err}), so it will not be overwritten"),
    }
    (T::default(), moved.is_ok())
}

/// Move a file from an old location once, atomically, keeping the source
/// until the copy is durable.
pub fn migrate_file(src: &Path, dest: &Path) {
    if dest.exists() || !src.exists() {
        return;
    }
    match std::fs::read(src).and_then(|bytes| write_atomic(dest, &bytes)) {
        Ok(()) => {
            let _ = std::fs::remove_file(src);
        }
        Err(err) => eprintln!("Failed to move {} to {}: {err}", src.display(), dest.display()),
    }
}
