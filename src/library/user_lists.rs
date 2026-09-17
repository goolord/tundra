//! User-owned path lists: the allowed search directories and favorites.

use crate::app_data::{cache_file, config_file, load_user_data, write_bincode};
use crate::path_util::canonical_path;
use std::path::{Path, PathBuf};

/// A sorted path list saved as bincode. User data, so loading never loses it:
/// an unreadable file is moved aside, or else left alone and never saved over.
#[derive(Debug, Clone, Default)]
struct SavedPaths {
    paths: Vec<PathBuf>,
    file: Option<PathBuf>,
    label: &'static str,
    /// Set when the file on disk could not be read; saving would destroy it.
    read_only: bool,
}

impl SavedPaths {
    fn load(file: Option<PathBuf>, label: &'static str) -> Self {
        let Some(path) = file else {
            return Self { label, ..Self::default() };
        };
        let (paths, writable) = load_user_data(&path, label);
        Self {
            paths,
            file: Some(path),
            label,
            read_only: !writable,
        }
    }

    fn persist(&self) {
        if let Some(path) = self.file.as_ref().filter(|_| !self.read_only) {
            write_bincode(path, &self.paths, self.label);
        }
    }

    /// Adds `path`, keeping the list sorted. False if it was already there.
    fn insert(&mut self, path: PathBuf) -> bool {
        if self.paths.contains(&path) {
            return false;
        }
        self.paths.push(path);
        self.paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddDirectory {
    Added(PathBuf),
    Duplicate,
    Unresolved,
}

/// Folders Tundra may search, cache, and tag inside.
#[derive(Debug, Clone, Default)]
pub struct AllowedDirectories(SavedPaths);

impl AllowedDirectories {
    pub fn load() -> Self {
        let file = config_file("allowed_directories.bin");
        if let Some(path) = &file {
            migrate_settings_from_cache(path);
        }
        Self::load_from(file)
    }

    fn load_from(file: Option<PathBuf>) -> Self {
        Self(SavedPaths::load(file, "allowed directories"))
    }

    pub fn persist(&self) {
        self.0.persist();
    }

    pub fn is_empty(&self) -> bool {
        self.0.paths.is_empty()
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.0.paths
    }

    pub fn add(&mut self, path: &Path) -> AddDirectory {
        match canonical_path(path) {
            Err(_) => AddDirectory::Unresolved,
            Ok(resolved) if self.0.insert(resolved.clone()) => AddDirectory::Added(resolved),
            Ok(_) => AddDirectory::Duplicate,
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.0.paths.retain(|root| root != path);
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        let resolved = canonical_path(path).unwrap_or_else(|_| path.to_path_buf());
        self.contains_cached_path(&resolved)
    }

    /// Like `contains_path` for keys that are already canonical (cache entries),
    /// skipping a filesystem lookup per key.
    pub fn contains_cached_path(&self, path: &Path) -> bool {
        self.roots().iter().any(|root| crate::path_util::is_under(path, root))
    }

    pub fn startup_directory(&self) -> Option<PathBuf> {
        self.roots().first().cloned()
    }
}

/// Starred files. Never pruned automatically; missing files are hidden at display time.
#[derive(Debug, Clone, Default)]
pub struct FavoritesStore(SavedPaths);

impl FavoritesStore {
    pub fn load() -> Self {
        Self(SavedPaths::load(config_file("favorites.bin"), "favorites"))
    }

    pub fn persist(&self) {
        self.0.persist();
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.0.paths
    }

    /// Whether a row from a directory listing is starred. Listing paths are
    /// already resolved, so this skips the filesystem and is cheap per frame.
    pub fn contains_listed(&self, path: &Path) -> bool {
        let key = crate::path_util::cache_key(path);
        self.paths().contains(&key)
    }

    /// Stars or unstars `path`; true when it is now a favorite.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let key = crate::path_util::favorite_lookup_key(path);
        if let Some(index) = self.paths().iter().position(|stored| *stored == key) {
            self.0.paths.remove(index);
            false
        } else {
            self.0.insert(key)
        }
    }
}

/// Older builds kept the allowed directories in the cache directory.
fn migrate_settings_from_cache(config_path: &Path) {
    if config_path.exists() {
        return;
    }
    if let Some(cache_path) = cache_file("allowed_directories.bin") {
        crate::app_data::migrate_file(&cache_path, config_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_write::{reclaim_write_sidecars, sidecar, REPLACE_OLD_SUFFIX};
    use crate::test_fixtures::ScratchDir;

    #[test]
    fn favorites_toggle_adds_then_removes() {
        let dir = ScratchDir::new("favorites-store");
        let sample = dir.path().join("kick.wav");
        std::fs::write(&sample, b"wav").expect("sample");

        let mut favorites = FavoritesStore::default();
        assert!(favorites.toggle(&sample));
        assert_eq!(favorites.paths().len(), 1);
        assert!(!favorites.toggle(&sample));
        assert!(favorites.paths().is_empty());
    }

    #[test]
    fn allowed_directories_persist_recovers_from_crash_aside() {
        let dir = ScratchDir::new("settings-persist");
        let path = dir.path().join("allowed_directories.bin");
        let samples = dir.path().join("samples");
        std::fs::create_dir_all(&samples).expect("samples dir");
        let mut allowed = AllowedDirectories::load_from(Some(path.clone()));
        assert!(matches!(allowed.add(&samples), AddDirectory::Added(_)));
        assert_eq!(allowed.add(&samples), AddDirectory::Duplicate);

        allowed.persist();
        let bytes = std::fs::read(&path).expect("persisted");

        std::fs::write(sidecar(&path, REPLACE_OLD_SUFFIX), &bytes).expect("crash aside");
        std::fs::remove_file(&path).expect("crash delete");

        reclaim_write_sidecars(dir.path());
        assert_eq!(std::fs::read(&path).expect("restored"), bytes);

        allowed.persist();
        assert_eq!(dir.sidecar_count(), 0);
    }

    #[test]
    fn unreadable_settings_are_moved_aside_not_overwritten() {
        let dir = ScratchDir::new("settings-corrupt");
        let path = dir.path().join("allowed_directories.bin");
        std::fs::write(&path, b"\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFFnot bincode").expect("corrupt");

        let loaded = AllowedDirectories::load_from(Some(path.clone()));
        assert!(loaded.is_empty());
        assert!(!path.exists(), "unreadable file must not stay where a save would replace it");

        loaded.persist();
        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .expect("list")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".unreadable-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            std::fs::read(kept[0].path()).expect("backup"),
            b"\xFF\xFF\xFF\xFF\xFF\xFF\xFF\xFFnot bincode"
        );
    }

    #[test]
    fn migrate_file_moves_once_and_never_overwrites() {
        let dir = ScratchDir::new("settings-migrate");
        let src = dir.path().join("cache").join("allowed_directories.bin");
        let dest = dir.path().join("config").join("allowed_directories.bin");
        std::fs::create_dir_all(src.parent().unwrap()).expect("cache dir");
        std::fs::write(&src, b"old").expect("src");

        crate::app_data::migrate_file(&src, &dest);
        assert_eq!(std::fs::read(&dest).expect("dest"), b"old");
        assert!(!src.exists());

        std::fs::write(&src, b"stale").expect("src again");
        crate::app_data::migrate_file(&src, &dest);
        assert_eq!(std::fs::read(&dest).expect("dest"), b"old");
    }
}
