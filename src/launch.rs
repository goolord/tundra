use std::ffi::OsStr;
use std::path::PathBuf;

pub fn paths_from_args() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .filter(|arg| !is_launch_noise(arg))
        .map(PathBuf::from)
        .filter_map(normalize_launch_path)
        .collect()
}

/// Flags the OS or a launcher adds (macOS `-psn_…`). A real file whose name
/// starts with a dash is still opened.
fn is_launch_noise(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|value| value.starts_with('-')) && !std::path::Path::new(arg).exists()
}

fn normalize_launch_path(path: PathBuf) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    Some(if path.exists() {
        crate::path_util::canonical_path(&path).unwrap_or_else(|_| {
            crate::path_util::normalize_path(path)
        })
    } else {
        crate::path_util::normalize_path(path)
    })
}

pub fn primary_open_target(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|path| crate::metadata::is_audio(path))
        .or_else(|| paths.iter().find(|path| path.is_dir()))
        .cloned()
        .or_else(|| paths.first().cloned())
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
        let paths = vec![
            PathBuf::from("notes.txt"),
            PathBuf::from("2 kick.wav"),
            PathBuf::from("3 kick.wav"),
        ];
        assert_eq!(
            primary_open_target(&paths),
            Some(PathBuf::from("2 kick.wav"))
        );
    }
}
