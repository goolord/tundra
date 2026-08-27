//! Shared scratch dirs and minimal WAV bytes for integration tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::path_util::sidecar;
use crate::path_util::REPLACE_OLD_SUFFIX;

/// PID guaranteed dead on all platforms (`u32::MAX - 1`).
pub const DEAD_PID: u32 = 4294967294;

pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tundra-test-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
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

pub fn count_tundra_sidecars(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .contains(".tundra-")
                })
                .count()
        })
        .unwrap_or(0)
}

pub fn minimal_wav_bytes() -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&1u16.to_le_bytes());
    fmt.extend_from_slice(&44100u32.to_le_bytes());
    fmt.extend_from_slice(&88200u32.to_le_bytes());
    fmt.extend_from_slice(&2u16.to_le_bytes());
    fmt.extend_from_slice(&16u16.to_le_bytes());
    let data = vec![0_u8; 512];
    let chunk = |id: [u8; 4], body: &[u8]| {
        let mut out = id.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    };
    let mut riff = b"RIFF".to_vec();
    let mut body = b"WAVE".to_vec();
    body.extend_from_slice(&chunk(*b"fmt ", &fmt));
    body.extend_from_slice(&chunk(*b"data", &data));
    riff.extend_from_slice(&(body.len() as u32).to_le_bytes());
    riff.extend_from_slice(&body);
    riff
}

pub fn write_minimal_wav(path: &Path) {
    fs::write(path, minimal_wav_bytes()).expect("write wav");
}

/// Stale tag tmp left by a crashed process (dead PID in the sidecar name).
pub fn dead_pid_tag_tmp(dest: &Path) -> PathBuf {
    sidecar(dest, &format!(".tundra-tag-{DEAD_PID}-1.tmp"))
}

/// Simulates a crash that left `.tundra-replace-old` and removed the dest file.
pub fn restore_dest_from_crash_aside(dir: &Path, dest: &Path, aside_bytes: &[u8]) {
    fs::write(
        sidecar(dest, REPLACE_OLD_SUFFIX),
        aside_bytes,
    )
    .expect("crash aside");
    let _ = fs::remove_file(dest);
    crate::path_util::reclaim_write_sidecars(dir);
}

/// Holds `dest` open so same-directory atomic replace fails cross-platform.
#[must_use]
pub struct DestReplaceLock(#[allow(dead_code)] std::fs::File);

pub fn lock_dest_against_replace(dest: &Path) -> DestReplaceLock {
    use std::fs::OpenOptions;
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Allow staging reads; block delete/replace while this handle lives.
        opts.share_mode(1);
    }
    DestReplaceLock(opts.open(dest).expect("lock dest"))
}

#[allow(unused_variables)]
pub fn with_replace_blocked<R>(dir: &Path, dest: &Path, f: impl FnOnce() -> R) -> R {
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(dir).expect("meta").permissions();
        perms.set_readonly(true);
        fs::set_permissions(dir, perms).expect("lock parent");
        let result = f();
        let mut perms = fs::metadata(dir).expect("meta").permissions();
        perms.set_readonly(false);
        let _ = fs::set_permissions(dir, perms);
        result
    }
    #[cfg(windows)]
    {
        let _lock = lock_dest_against_replace(dest);
        f()
    }
}
