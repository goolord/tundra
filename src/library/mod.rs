//! The user's sample library: allowed roots and favorites, directory listings,
//! the persisted listing and tag caches, and running a search over them.
//!
//! Nothing here depends on the UI; `ui::app` drives it from background tasks.

pub mod cache;
pub mod search;
mod user_lists;

pub use user_lists::{AddDirectory, AllowedDirectories, FavoritesStore};

use crate::metadata::is_audio;
use crate::path_util::is_hidden;
use crate::safe_write::SidecarSweep;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// What listings and walks show: visible folders, and audio files. `is_dir`
/// comes from the directory entry so files cost no extra `stat`.
fn keeps_entry(path: &Path, is_dir: bool) -> bool {
    if is_dir { !is_hidden(path) } else { is_audio(path) }
}

/// Every audio file and visible folder under `dir`, recursively. Reclaims
/// interrupted tag writes on the way.
pub fn walk_directory(dir: &Path) -> Vec<PathBuf> {
    let mut sweep = SidecarSweep::default();
    let mut paths: Vec<PathBuf> = WalkDir::new(dir)
        .max_depth(100)
        .max_open(100)
        .follow_links(true)
        .into_iter()
        .filter_entry(|entry| {
            sweep.note(entry.path());
            keeps_entry(entry.path(), entry.file_type().is_dir())
        })
        .filter_map(|entry| entry.ok().map(walkdir::DirEntry::into_path))
        .collect();
    paths.extend(sweep.finish().into_iter().filter(|path| is_audio(path)));
    paths
}

pub struct ListedEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// The folders and audio files directly inside `dir`, unsorted.
pub fn list_directory(dir: &Path) -> Result<Vec<ListedEntry>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|err| format!("Cannot read {}: {err}", crate::path_util::display_path(dir)))?;
    let mut sweep = SidecarSweep::default();
    let mut listed: Vec<ListedEntry> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            let path = entry.path();
            sweep.note(&path);
            let is_dir = file_type.is_dir() || (file_type.is_symlink() && path.is_dir());
            keeps_entry(&path, is_dir).then_some(ListedEntry { path, is_dir })
        })
        .collect();
    listed.extend(
        sweep
            .finish()
            .into_iter()
            .filter(|path| is_audio(path))
            .map(|path| ListedEntry { path, is_dir: false }),
    );
    Ok(listed)
}

#[cfg(test)]
mod tests {
    use super::walk_directory;
    use crate::safe_write::{REPLACE_OLD_SUFFIX, sidecar};
    use crate::test_fixtures::{ScratchDir, dead_pid_tag_tmp};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn walk_lists_audio_reclaims_sidecars_and_skips_hidden_dirs() {
        let dir = ScratchDir::new("walk-directory");
        let drums = dir.path().join("Drums");
        let hidden = dir.path().join(".git");
        std::fs::create_dir_all(&drums).unwrap();
        std::fs::create_dir_all(&hidden).unwrap();
        let kick = drums.join("kick.wav");
        std::fs::write(&kick, b"RIFF").unwrap();
        std::fs::write(dead_pid_tag_tmp(&kick), b"stale").unwrap();
        let snare = drums.join("snare.wav");
        std::fs::write(sidecar(&snare, REPLACE_OLD_SUFFIX), b"aside").unwrap();
        std::fs::write(hidden.join("hat.wav"), b"RIFF").unwrap();
        std::fs::write(drums.join("notes.txt"), b"text").unwrap();

        let walked: HashSet<PathBuf> = walk_directory(dir.path()).into_iter().collect();

        assert!(walked.contains(&kick));
        assert!(walked.contains(&snare), "restored file must be listed");
        assert!(!walked.iter().any(|path| path.starts_with(&hidden)));
        assert!(!walked.iter().any(|path| path.ends_with("notes.txt")));
        assert_eq!(dir.sidecar_count(), 0);
        assert_eq!(crate::test_fixtures::count_tundra_sidecars(&drums), 0);
    }
}
