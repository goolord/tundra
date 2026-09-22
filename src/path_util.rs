//! Path spellings and labels.
//!
//! One file can be reached by several spellings: with or without Windows'
//! `\\?\` prefix, and in any letter case on case-insensitive volumes. Caches
//! key everything by `cache_key`; `resolve_open_path` turns a key back into a
//! path the filesystem will open.

use std::path::{Path, PathBuf};

/// Strip Windows extended-length `\\?\` / `\\?\UNC\` prefixes so paths work with
/// drag targets, Python, and stable cache keys.
pub fn normalize_path(path: PathBuf) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = rendered.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

pub fn canonical_path(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(normalize_path)
}

/// The spelling caches key a path by: no `\\?\` prefix, and case-folded on
/// Windows and macOS, whose default volumes ignore case.
pub fn cache_key(path: &Path) -> PathBuf {
    let path = normalize_path(path.to_path_buf());
    if cfg!(any(windows, target_os = "macos")) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path
    }
}

/// Keys a cache lookup must try: the path as stored, then the stable `cache_key`.
/// WalkDir and `canonicalize` disagree on the `\\?\` prefix, so one file can
/// appear under both forms.
pub fn cache_lookup_keys(path: &Path) -> Vec<PathBuf> {
    let key = cache_key(path);
    if key == path {
        vec![key]
    } else {
        vec![path.to_path_buf(), key]
    }
}

/// The key favorites are stored under: the canonical path's cache key when the file exists.
pub fn favorite_lookup_key(path: &Path) -> PathBuf {
    canonical_path(path).map_or_else(|_| cache_key(path), |canonical| cache_key(&canonical))
}

/// Turn a cache key or stale spelling into a path the filesystem will open.
///
/// Metadata and search caches store lowercase `cache_key` paths. Those are fine for
/// lookups, but playback needs the spelling the directory walk recorded (or whatever
/// variant actually exists on disk).
pub fn resolve_open_path<'a>(path: &Path, known_paths: impl IntoIterator<Item = &'a Path>) -> PathBuf {
    let path = repair_windows_drive_path(path);

    if let Some(existing) = cache_lookup_keys(&path).into_iter().find(|key| key.exists()) {
        return canonical_path(&existing).unwrap_or_else(|_| normalize_path(existing));
    }

    let target = cache_key(&path);
    if let Some(candidate) = known_paths
        .into_iter()
        .find(|candidate| cache_key(candidate) == target && candidate.exists())
    {
        return canonical_path(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    }

    normalize_path(path)
}

/// Windows paths missing the separator after the drive letter (`F:Samples\...`) fail
/// to open. Some cached spellings also carry a stray `|` there from older data.
fn repair_windows_drive_path(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    match rendered.as_bytes() {
        _ if cfg!(not(windows)) => path.to_path_buf(),
        [_, b':', b'\\' | b'/', ..] => path.to_path_buf(),
        [_, b':', b'|', ..] => PathBuf::from(format!(r"{}\{}", &rendered[..2], &rendered[3..])),
        [_, b':', _, ..] => PathBuf::from(format!(r"{}\{}", &rendered[..2], &rendered[2..])),
        _ => path.to_path_buf(),
    }
}

/// True when `path` is `root` or a descendant, ignoring `\\?\` and case.
pub fn is_under(path: &Path, root: &Path) -> bool {
    cache_key(path).starts_with(cache_key(root))
}

pub fn file_name_lossy(path: &Path) -> Option<String> {
    path.file_name().map(|name| name.to_string_lossy().into_owned())
}

pub fn file_stem_lossy(path: &Path) -> Option<String> {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

/// The file name, or the whole path when it has none.
pub fn file_label(path: &Path) -> String {
    file_name_lossy(path).unwrap_or_else(|| path.display().to_string())
}

/// `path` as text, keeping the end when it is longer than `max_chars`.
pub fn truncate_path(path: &Path, max_chars: usize) -> String {
    let rendered = path.display().to_string();
    let count = rendered.chars().count();
    if count <= max_chars {
        return rendered;
    }
    let tail: String = rendered.chars().skip(count - max_chars.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// Dot-files, plus files the OS marks hidden.
pub fn is_hidden(path: &Path) -> bool {
    if path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_HIDDEN: u32 = 0x8000;
        if let Ok(meta) = std::fs::metadata(path) {
            return meta.st_flags() & UF_HIDDEN != 0;
        }
    }
    false
}

/// Identifies one version of a file: its size and modification time. Size is
/// included because copies and archive extraction often keep the original
/// mtime (and exFAT has 2 s resolution), so mtime alone can match a different file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileStamp {
    pub secs: u64,
    pub nanos: u32,
    pub len: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(Self {
            secs: modified.as_secs(),
            nanos: modified.subsec_nanos(),
            len: meta.len(),
        })
    }
}

/// The file's modification time in whole seconds.
pub fn file_mtime_secs(path: &Path) -> Option<u64> {
    FileStamp::of(path).map(|stamp| stamp.secs)
}

/// A user-facing I/O error message: "Failed to <verb> <path>: <err>".
pub fn path_io_error(verb: &str, path: &Path, err: impl std::fmt::Display) -> String {
    format!("Failed to {verb} {}: {err}", path.display())
}

pub fn open_file(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path).map_err(|err| path_io_error("open", path, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;
    use std::fs;

    #[test]
    fn is_under_matches_whole_components_ignoring_verbatim_prefix_and_case() {
        let root = Path::new("/Samples");
        assert!(is_under(Path::new("/Samples/Snare/01_Snare.flac"), root));
        assert!(is_under(root, root));
        assert!(!is_under(Path::new("/Samples Old/snare.flac"), root));
        assert!(!is_under(Path::new("/Other/snare.flac"), root));
        #[cfg(windows)]
        {
            let root = Path::new(r"\\?\F:\Samples");
            assert!(is_under(
                Path::new(r"f:\samples\ADM Samples - Copy\Snare\01_Snare.flac"),
                root
            ));
            assert!(!is_under(Path::new(r"F:\Other\snare.flac"), root));
        }
    }

    #[test]
    fn cache_lookup_keys_include_normalized_form() {
        let path = PathBuf::from(r"\\?\F:\Samples\Snare\01_Snare.flac");
        let keys = cache_lookup_keys(&path);
        assert!(keys.contains(&path));
        assert!(keys.contains(&cache_key(&path)));
    }

    #[test]
    #[cfg(windows)]
    fn repair_windows_drive_path_fixes_missing_separator_and_pipe() {
        for broken in [r"F:Samples\kick.wav", r"F:|Samples\kick.wav"] {
            assert_eq!(
                repair_windows_drive_path(Path::new(broken)),
                PathBuf::from(r"F:\Samples\kick.wav")
            );
        }
        for fine in [r"F:\Samples\kick.wav", "F:", "kick.wav"] {
            assert_eq!(repair_windows_drive_path(Path::new(fine)), PathBuf::from(fine));
        }
    }

    #[test]
    fn resolve_open_path_prefers_a_walked_spelling() {
        let dir = ScratchDir::new("resolve-open-path");
        let nested = dir.path().join("Drums");
        fs::create_dir_all(&nested).unwrap();
        let audio = nested.join("kick.wav");
        fs::write(&audio, b"RIFF").unwrap();

        let cache_key_path = cache_key(&audio);
        let resolved = resolve_open_path(&cache_key_path, [audio.as_path()]);
        assert!(resolved.exists());
        assert_eq!(cache_key(&resolved), cache_key_path);
    }

    #[test]
    fn favorite_lookup_key_matches_verbatim_and_canonical_paths() {
        let dir = ScratchDir::new("favorite-lookup-key");
        let file = dir.path().join("kick.wav");
        fs::write(&file, b"wav").unwrap();
        let stored = favorite_lookup_key(&file);
        assert_eq!(favorite_lookup_key(&stored), stored);

        // The temp dir as the OS reports it can differ from its canonical form:
        // a symlink (`/var` -> `/private/var` on macOS) or an 8.3 short name on Windows.
        let reported = std::env::temp_dir()
            .join(dir.path().file_name().unwrap())
            .join("kick.wav");
        assert_eq!(favorite_lookup_key(&reported), stored);

        #[cfg(windows)]
        {
            let verbatim = PathBuf::from(format!(r"\\?\{}", file.display()));
            assert_eq!(favorite_lookup_key(&verbatim), stored);
        }
    }

    #[test]
    fn truncate_path_keeps_the_end() {
        let path = Path::new("/samples/drums/kick.wav");
        assert_eq!(truncate_path(path, 100), "/samples/drums/kick.wav");
        assert_eq!(truncate_path(path, 9), "…kick.wav");
    }
}
