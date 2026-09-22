use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::path_util::{canonical_path, normalize_path};

pub fn paths_from_args() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.is_empty() && !is_launch_noise(arg))
        .map(|arg| canonical_path(Path::new(&arg)).unwrap_or_else(|_| normalize_path(arg.into())))
        .collect()
}

/// Flags the OS or a launcher adds (macOS `-psn_…`). A real file whose name
/// starts with a dash is still opened.
fn is_launch_noise(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|value| value.starts_with('-')) && !Path::new(arg).exists()
}

pub fn primary_open_target(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|path| crate::metadata::is_audio(path))
        .or_else(|| paths.iter().find(|path| path.is_dir()))
        .or_else(|| paths.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_dash_prefixed_launch_noise() {
        assert!(is_launch_noise(OsStr::new("-psn_0_12345")));
        assert!(is_launch_noise(OsStr::new("--help")));
        assert!(!is_launch_noise(OsStr::new("kick.wav")));
    }

    #[test]
    fn primary_open_target_prefers_audio_over_other_paths() {
        let paths = ["notes.txt", "2 kick.wav", "3 kick.wav"].map(PathBuf::from);
        assert_eq!(primary_open_target(&paths), Some(PathBuf::from("2 kick.wav")));
    }
}
