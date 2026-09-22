//! Shared scratch dirs and minimal WAV bytes for integration tests.

use std::fs;
use std::path::{Path, PathBuf};

use crate::safe_write::sidecar;

/// PID guaranteed dead on all platforms (`u32::MAX - 1`).
pub const DEAD_PID: u32 = 4294967294;

pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        // The 8-digit `_<id>` part marks the folder as throwaway to the path
        // hints, so tests that tag files never take it for an artist name.
        let dir = std::env::temp_dir().join(format!("tundra-test-{label}_{}_{id:08}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch dir");
        // The temp dir as the OS reports it may be a symlink (`/var` on macOS) or
        // an 8.3 short name (`RUNNER~1` on Windows CI). The app canonicalizes
        // library roots, so fixtures standing in for one must match.
        Self(crate::path_util::canonical_path(&dir).expect("canonical scratch dir"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn sidecar_count(&self) -> usize {
        count_tundra_sidecars(self.path())
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Extensions of the `tests/assets/tone.*` fixtures, one per supported container.
pub const ASSET_FORMATS: [&str; 5] = ["wav", "flac", "mp3", "ogg", "aiff"];

/// Copies `tests/assets/tone.<ext>` (a short real recording) to `dir/<stem>.<ext>` so a test can modify it.
pub fn copy_asset(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/assets/tone.{ext}"));
    let target = dir.join(format!("{stem}.{ext}"));
    fs::copy(asset, &target).unwrap_or_else(|err| panic!("copy {ext} fixture: {err}"));
    target
}

pub fn count_tundra_sidecars(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |entries| {
        entries.flatten().filter(|entry| entry.file_name().to_string_lossy().contains(".tundra-")).count()
    })
}

/// 256 silent frames of 16-bit mono 44.1 kHz PCM.
pub fn minimal_wav_bytes() -> Vec<u8> {
    let mut wav = b"RIFF".to_vec();
    wav.extend(548u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    // fmt size; PCM, mono; rate; byte rate; block align 2, 16 bits.
    wav.extend([16u32, 0x0001_0001, 44_100, 88_200, 0x0010_0002].iter().flat_map(|word| word.to_le_bytes()));
    wav.extend(b"data");
    wav.extend(512u32.to_le_bytes());
    wav.resize(wav.len() + 512, 0);
    wav
}

pub fn write_minimal_wav(path: &Path) {
    fs::write(path, minimal_wav_bytes()).expect("write wav");
}

/// Replaces the RIFF INFO list of the WAV at `path` with `fields`, given as
/// `(key, value)` pairs such as `("IKEY", "Snare")`, the way another tagger
/// would have left the file.
pub fn write_riff_info(path: &Path, fields: &[(&str, &str)]) {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::iff::wav::{RiffInfoList, WavFile};

    let mut file = fs::File::open(path).expect("open wav");
    let mut wav = WavFile::read_from(&mut file, ParseOptions::new()).expect("parse wav");
    drop(file);
    let mut info = RiffInfoList::new();
    for (key, value) in fields {
        info.insert(key.to_string(), value.to_string());
    }
    wav.set_riff_info(info);
    wav.save_to_path(path, WriteOptions::default()).expect("save RIFF INFO");
}

/// Stale tag tmp left by a crashed process (dead PID in the sidecar name).
pub fn dead_pid_tag_tmp(dest: &Path) -> PathBuf {
    sidecar(dest, &format!(".tundra-tag-{DEAD_PID}-1.tmp"))
}

/// Runs `f` while an atomic replace of `dest` in `dir` cannot succeed: the
/// directory is read-only on Unix, and `dest` is held open without delete
/// sharing on Windows.
pub fn with_replace_blocked<R>(dir: &Path, dest: &Path, f: impl FnOnce() -> R) -> R {
    #[cfg(unix)]
    {
        let _ = dest;
        let set_readonly = |readonly| {
            let mut perms = fs::metadata(dir).expect("meta").permissions();
            perms.set_readonly(readonly);
            fs::set_permissions(dir, perms)
        };
        set_readonly(true).expect("lock parent");
        let result = f();
        let _ = set_readonly(false);
        result
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let _ = dir;
        // FILE_SHARE_READ: staging may read it; deleting or replacing it fails.
        let _lock = fs::OpenOptions::new().read(true).write(true).share_mode(1).open(dest).expect("lock dest");
        f()
    }
}
