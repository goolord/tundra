//! Writing files so a crash never loses the old copy, and recovering from the
//! temp files an interrupted write leaves behind.
//!
//! Every write goes to a temp file beside its destination (a "write sidecar",
//! named `<file>.tundra-…`), which then replaces the destination atomically.
//! Walks and startup call `reclaim_write_sidecars` to clean up after crashes.
//! Unrelated to the SQLite tag store in `tag_store`.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Legacy only: older builds wrote these fixed names; reclaim still cleans them up.
const TAG_TMP_SUFFIX: &str = ".tundra-tag.tmp";
const TAG_BAK_SUFFIX: &str = ".tundra-tag.bak";
/// The original, moved aside during a replace. The only sidecar ever restored.
pub const REPLACE_OLD_SUFFIX: &str = ".tundra-replace-old";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidecarKind {
    Tmp,
    Bak,
    ReplaceOld,
}

/// `path` with `suffix` appended to its file name.
pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Same-directory temp that two Tundra processes cannot share.
pub fn unique_sidecar(path: &Path, kind: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    sidecar(path, &format!(".tundra-{kind}-{}-{seq}.tmp", std::process::id()))
}

/// The destination file name, sidecar kind, and writer's process id, if `name` is a sidecar.
fn parse_write_sidecar(name: &str) -> Option<(&str, SidecarKind, Option<u32>)> {
    let fixed = [
        (REPLACE_OLD_SUFFIX, SidecarKind::ReplaceOld),
        (TAG_BAK_SUFFIX, SidecarKind::Bak),
        (TAG_TMP_SUFFIX, SidecarKind::Tmp),
    ];
    let (dest, kind, pid) = match fixed
        .into_iter()
        .find_map(|(suffix, kind)| Some((name.strip_suffix(suffix)?, kind)))
    {
        Some((dest, kind)) => (dest, kind, None),
        None => {
            // `unique_sidecar` names: `<dest>.tundra-<kind>-<pid>-<seq>.tmp`.
            let rest = name.strip_suffix(".tmp")?;
            let (index, marker) = [".tundra-tag-", ".tundra-atomic-"]
                .into_iter()
                .find_map(|marker| rest.rfind(marker).map(|index| (index, marker)))?;
            let pid = rest[index + marker.len()..].split('-').next()?.parse().ok();
            (&rest[..index], SidecarKind::Tmp, pid)
        }
    };
    (!dest.is_empty()).then_some((dest, kind, pid))
}

fn is_write_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| parse_write_sidecar(name).is_some())
}

fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if pid == std::process::id() {
        return true;
    }
    #[cfg(windows)]
    let alive = win::pid_is_alive(pid);
    #[cfg(target_os = "macos")]
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success());
    #[cfg(not(any(windows, target_os = "macos")))]
    let alive = Path::new(&format!("/proc/{pid}")).exists();
    alive
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }

    pub fn pid_is_alive(pid: u32) -> bool {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const ERROR_ACCESS_DENIED: u32 = 5;
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
            return true;
        }
        // Another user's process exists but cannot be opened.
        unsafe { GetLastError() == ERROR_ACCESS_DENIED }
    }

    /// `ReplaceFileW` with no backup file.
    pub fn replace_existing(from: &Path, to: &Path) -> std::io::Result<()> {
        let wide = |path: &Path| path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let (replaced, replacement) = (wide(to), wide(from));
        let null = std::ptr::null_mut();
        match unsafe { ReplaceFileW(replaced.as_ptr(), replacement.as_ptr(), std::ptr::null(), 0, null, null) } {
            0 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}

/// Same-directory replace. POSIX `rename` overwrites atomically; Windows falls
/// back to `ReplaceFileW`. Never moves the dest aside.
pub fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        #[cfg(windows)]
        Err(_) if to.exists() => win::replace_existing(from, to),
        result => result,
    }
}

/// Clear the read-only attribute (Windows) or grant only the owner write
/// permission (Unix; `set_readonly(false)` would make it world-writable) so
/// writes and fsync succeed.
pub fn ensure_writable(path: &Path) -> io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    if !perms.readonly() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(perms.mode() | 0o200);
    }
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(path, perms)
}

/// Flush file data/metadata to disk before atomic replace.
pub fn sync_file(path: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

/// Flush the directory entry, so a completed rename survives power loss.
pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    options.open(parent)?.sync_all()
}

/// Restore a missing dest from `.tundra-replace-old` only (crash-aside).
/// Keep a tmp only when its PID is still live. Delete dest-less legacy
/// `.tundra-tag.tmp` only after dest exists. Never resurrect dest from
/// `.bak`/`.tmp` (user may have deleted the audio).
///
/// Returns the files restored from a crash-aside copy.
pub fn reclaim_write_sidecars(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut groups: HashMap<PathBuf, Vec<(PathBuf, SidecarKind, Option<u32>)>> = HashMap::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some((dest_name, kind, pid)) = parse_write_sidecar(&name) {
            let dest = path.with_file_name(dest_name);
            groups.entry(dest).or_default().push((path, kind, pid));
        }
    }

    let mut restored = Vec::new();
    for (dest, sidecars) in groups {
        if !dest.exists()
            && let Some((aside, _, _)) = sidecars.iter().find(|(_, kind, _)| *kind == SidecarKind::ReplaceOld)
        {
            if std::fs::rename(aside, &dest).is_err() && std::fs::copy(aside, &dest).is_ok() {
                let _ = std::fs::remove_file(aside);
            }
            if dest.exists() {
                restored.push(dest.clone());
            }
        }

        // With the destination gone a sidecar may be the only copy left by a
        // replace that failed half-way; never delete it then.
        if !dest.exists() {
            continue;
        }
        for (path, kind, pid) in sidecars {
            let in_flight = kind == SidecarKind::Tmp && pid.is_some_and(pid_is_alive);
            if !in_flight {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    restored
}

/// Notes directories holding write sidecars while a walk is already listing
/// them, so recovery costs no second pass over the tree and never follows a
/// link the walk itself would not.
#[derive(Default)]
pub struct SidecarSweep {
    dirs: HashSet<PathBuf>,
}

impl SidecarSweep {
    pub fn note(&mut self, path: &Path) {
        if is_write_sidecar(path)
            && let Some(parent) = path.parent()
        {
            self.dirs.insert(parent.to_path_buf());
        }
    }

    /// Reclaim every noted directory. Returns files restored from a
    /// crash-aside copy, which the walk could not have listed.
    pub fn finish(self) -> Vec<PathBuf> {
        self.dirs.iter().flat_map(|dir| reclaim_write_sidecars(dir)).collect()
    }
}

/// Replaces `path` with `bytes` all at once, creating parent directories.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = unique_sidecar(path, "atomic");
    let result = std::fs::File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| replace_file(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{DEAD_PID, ScratchDir, with_replace_blocked};
    use std::fs;

    #[test]
    fn readonly_files_can_be_replaced_and_synced() {
        let dir = ScratchDir::new("readonly");
        let dest = dir.path().join("kick.wav");
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        for path in [&dest, &tmp] {
            fs::write(path, b"audio").unwrap();
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_readonly(true);
            fs::set_permissions(path, perms).unwrap();
            ensure_writable(path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(path).unwrap().permissions().mode();
                assert_eq!(mode & 0o222, 0o200, "only the owner may gain write access");
            }
        }
        fs::write(&tmp, b"tagged").unwrap();
        sync_file(&tmp).unwrap();
        replace_file(&tmp, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"tagged");
        #[cfg(unix)]
        sync_parent_dir(&dest).unwrap();
    }

    /// Each case seeds a directory, reclaims it, and checks each file's contents
    /// afterwards (`None`: absent). `LIVE`/`DEAD` in a name become this
    /// process's id and a dead one.
    #[test]
    fn reclaim_restores_only_crash_asides_and_keeps_possible_last_copies() {
        type Case<'a> = (&'a str, &'a [(&'a str, Option<&'a str>, Option<&'a str>)]);
        let cases: [Case; 4] = [
            (
                "dest present: live tmps stay, everything else goes",
                &[
                    ("kick.wav", Some("original"), Some("original")),
                    ("kick.wav.tundra-tag-LIVE-1.tmp", Some("tmp"), Some("tmp")),
                    ("kick.wav.tundra-atomic-LIVE-2.tmp", Some("tmp"), Some("tmp")),
                    ("kick.wav.tundra-tag-DEAD-1.tmp", Some("stale"), None),
                    ("kick.wav.tundra-atomic-DEAD-7.tmp", Some("stale"), None),
                    ("kick.wav.tundra-tag.tmp", Some("legacy"), None),
                    ("kick.wav.tundra-tag.bak", Some("bak"), None),
                    ("kick.wav.tundra-replace-old", Some("old"), None),
                ],
            ),
            (
                "dest missing: restored from the aside, never from tmp or bak",
                &[
                    ("snare.wav.tundra-tag.tmp", Some("tmp"), None),
                    ("snare.wav.tundra-tag.bak", Some("bak"), None),
                    ("snare.wav.tundra-replace-old", Some("aside"), None),
                    ("snare.wav", None, Some("aside")),
                    ("kick.wav.tundra-replace-old", Some("kick-aside"), None),
                    ("kick.wav", None, Some("kick-aside")),
                ],
            ),
            (
                "dest deleted by the user: not resurrected, and no possible last copy deleted",
                &[
                    ("gone.wav", None, None),
                    ("gone.wav.tundra-tag.tmp", Some("tmp"), Some("tmp")),
                    ("gone.wav.tundra-tag.bak", Some("bak"), Some("bak")),
                    ("gone.wav.tundra-tag-DEAD-1.tmp", Some("stale"), Some("stale")),
                    ("gone.wav.tundra-tag-LIVE-1.tmp", Some("tmp"), Some("tmp")),
                ],
            ),
            (
                "not sidecars: untouched",
                &[
                    (".tundra-tag.tmp", Some("no dest name"), Some("no dest name")),
                    ("hat.wav.tundra-other-DEAD-1.tmp", Some("unknown"), Some("unknown")),
                ],
            ),
        ];
        let name = |name: &str| {
            name.replace("LIVE", &std::process::id().to_string())
                .replace("DEAD", &DEAD_PID.to_string())
        };
        for (case, files) in cases {
            let dir = ScratchDir::new("reclaim");
            for (file, before, _) in files {
                if let Some(contents) = before {
                    fs::write(dir.path().join(name(file)), contents).unwrap();
                }
            }
            reclaim_write_sidecars(dir.path());
            for (file, _, after) in files {
                let actual = fs::read_to_string(dir.path().join(name(file))).ok();
                assert_eq!(actual.as_deref(), *after, "{case}: {file}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn reclaim_keeps_sidecars_when_restore_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = ScratchDir::new("reclaim-readonly-dir");
        let dest = dir.path().join("rim.wav");
        let aside = sidecar(&dest, REPLACE_OLD_SUFFIX);
        fs::write(&aside, b"aside-original").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::write(&tmp, b"tmp").unwrap();

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        reclaim_write_sidecars(dir.path());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        assert!(!dest.exists());
        assert!(aside.exists());
        assert!(tmp.exists());
    }

    #[test]
    fn sweep_restores_only_noted_directories() {
        let dir = ScratchDir::new("sweep");
        let nested = dir.path().join("drums");
        fs::create_dir(&nested).unwrap();
        let dest = nested.join("kick.wav");
        let aside = sidecar(&dest, REPLACE_OLD_SUFFIX);
        fs::write(&aside, b"aside").unwrap();

        assert!(SidecarSweep::default().finish().is_empty());
        assert!(!dest.exists());

        let mut sweep = SidecarSweep::default();
        sweep.note(&nested.join("snare.wav"));
        sweep.note(&aside);
        assert_eq!(sweep.finish(), vec![dest.clone()]);
        assert_eq!(fs::read(&dest).unwrap(), b"aside");
    }

    #[test]
    fn write_atomic_replaces_whole_files_and_never_leaves_temps() {
        let dir = ScratchDir::new("atomic-write");
        let dest = dir.path().join("deep").join("cache.bin");
        write_atomic(&dest, b"one").unwrap();
        write_atomic(&dest, b"two").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"two");
        assert_eq!(crate::test_fixtures::count_tundra_sidecars(dest.parent().unwrap()), 0);

        // A failed replace keeps the old file and deletes the temp.
        let dest = dir.path().join("cache.bin");
        fs::write(&dest, b"stable").unwrap();
        assert!(with_replace_blocked(dir.path(), &dest, || write_atomic(&dest, b"new")).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"stable");
        let dir_dest = dir.path().join("settings");
        fs::create_dir(&dir_dest).unwrap();
        assert!(
            write_atomic(&dir_dest, b"partial").is_err(),
            "replacing a directory must fail"
        );
        assert!(dir_dest.is_dir());
        assert_eq!(dir.sidecar_count(), 0);
    }

    #[test]
    fn unique_sidecar_names_differ_for_same_dest() {
        let dest = Path::new("kick.wav");
        assert_ne!(unique_sidecar(dest, "tag"), unique_sidecar(dest, "tag"));
        assert!(is_write_sidecar(&unique_sidecar(dest, "atomic")) && !is_write_sidecar(dest));
        assert_eq!(sidecar(dest, TAG_TMP_SUFFIX), Path::new("kick.wav.tundra-tag.tmp"));
    }
}
