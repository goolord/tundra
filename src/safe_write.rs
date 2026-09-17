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

use crate::path_util::file_name_lossy;

pub const TAG_TMP_SUFFIX: &str = ".tundra-tag.tmp";
/// Legacy only: older builds wrote this, reclaim still deletes it.
pub const TAG_BAK_SUFFIX: &str = ".tundra-tag.bak";
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
    for (suffix, kind) in fixed {
        if let Some(dest) = name.strip_suffix(suffix) {
            return (!dest.is_empty()).then_some((dest, kind, None));
        }
    }
    // `unique_sidecar` names: `<dest>.tundra-<kind>-<pid>-<seq>.tmp`.
    let rest = name.strip_suffix(".tmp")?;
    let (index, marker) = [".tundra-tag-", ".tundra-atomic-"]
        .into_iter()
        .find_map(|marker| rest.rfind(marker).map(|index| (index, marker)))?;
    let dest = &rest[..index];
    let pid = rest[index + marker.len()..].split('-').next()?.parse().ok();
    (!dest.is_empty()).then_some((dest, SidecarKind::Tmp, pid))
}

/// True for temp and recovery files Tundra's writers leave beside a file.
pub fn is_write_sidecar(path: &Path) -> bool {
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
    {
        windows_pid_is_alive(pid)
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("kill");
        command.args(["-0", &pid.to_string()]);
        command.status().is_ok_and(|status| status.success())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

#[cfg(windows)]
fn windows_pid_is_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetLastError() -> u32;
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if !handle.is_null() {
        unsafe { CloseHandle(handle) };
        return true;
    }
    unsafe { GetLastError() == ERROR_ACCESS_DENIED }
}

/// Same-directory replace. POSIX `rename` overwrites atomically. Windows uses
/// `ReplaceFileW` with no backup file. Never moves the dest aside.
pub fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(_) if to.exists() => replace_existing_windows(from, to),
        Err(err) => Err(err),
    }
}

#[cfg(windows)]
fn replace_existing_windows(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            lp_replaced_file_name: *const u16,
            lp_replacement_file_name: *const u16,
            lp_backup_file_name: *const u16,
            dw_replace_flags: u32,
            lp_exclude: *mut core::ffi::c_void,
            lp_reserved: *mut core::ffi::c_void,
        ) -> i32;
    }

    let (replaced, replacement) = (wide(to), wide(from));
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Clear read-only attribute/permissions so writes and fsync succeed.
pub fn ensure_writable(path: &Path) -> io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Flush file data/metadata to disk before atomic replace.
pub fn sync_file(path: &Path) -> io::Result<()> {
    std::fs::OpenOptions::new().read(true).write(true).open(path)?.sync_all()
}

/// Flush the directory entry, so a completed rename survives power loss.
pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."));

    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(parent)?
            .sync_all()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = parent;
        Ok(())
    }
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
        let Some(name) = file_name_lossy(&path) else {
            continue;
        };
        if let Some((dest_name, kind, pid)) = parse_write_sidecar(&name) {
            groups.entry(path.with_file_name(dest_name)).or_default().push((path, kind, pid));
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
    let written = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    let result = written.and_then(|()| replace_file(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{dead_pid_tag_tmp, ScratchDir, DEAD_PID};
    use std::fs;

    fn read_only(path: &Path) {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn replace_works_when_dest_readonly() {
        let dir = ScratchDir::new("replace-readonly");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"audio").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::write(&tmp, b"tagged").unwrap();
        read_only(&dest);

        ensure_writable(&dest).unwrap();
        replace_file(&tmp, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"tagged");
    }

    #[test]
    fn sync_file_works_on_readonly_copy() {
        let dir = ScratchDir::new("sync-readonly");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"audio").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::copy(&dest, &tmp).unwrap();
        read_only(&tmp);

        ensure_writable(&tmp).unwrap();
        fs::write(&tmp, b"tagged").unwrap();
        sync_file(&tmp).unwrap();
    }

    #[test]
    fn reclaim_keeps_live_pid_tmp_when_dest_exists() {
        let dir = ScratchDir::new("reclaim-live");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"original").unwrap();
        let tmp = unique_sidecar(&dest, "tag");
        fs::write(&tmp, b"tmp").unwrap();
        fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak").unwrap();
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"old").unwrap();

        reclaim_write_sidecars(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"original");
        assert!(tmp.exists(), "in-progress tmp for this process must stay");
        assert!(!sidecar(&dest, TAG_BAK_SUFFIX).exists());
        assert!(!sidecar(&dest, REPLACE_OLD_SUFFIX).exists());
    }

    #[test]
    fn reclaim_deletes_dead_pid_tmp() {
        let dir = ScratchDir::new("reclaim-dead");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"original").unwrap();
        let tmp = dead_pid_tag_tmp(&dest);
        fs::write(&tmp, b"stale").unwrap();

        reclaim_write_sidecars(dir.path());

        assert!(!tmp.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"original");
    }

    #[test]
    fn reclaim_restores_replace_old_only_when_dest_missing() {
        let dir = ScratchDir::new("reclaim-restore");
        let dest = dir.path().join("snare.wav");
        fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"tmp-maybe-corrupt").unwrap();
        fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak-original").unwrap();
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"aside-original").unwrap();

        reclaim_write_sidecars(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"aside-original");
        assert!(!sidecar(&dest, TAG_TMP_SUFFIX).exists(), "legacy tmp is deleted once dest is present");
        assert!(!sidecar(&dest, TAG_BAK_SUFFIX).exists());
        assert!(!sidecar(&dest, REPLACE_OLD_SUFFIX).exists());
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
    fn atomic_temps_are_recognised_and_reclaimed() {
        let dir = ScratchDir::new("atomic-temps");
        let dest = dir.path().join("favorites.bin");
        fs::write(&dest, b"current").unwrap();
        let live = unique_sidecar(&dest, "atomic");
        let dead = sidecar(&dest, &format!(".tundra-atomic-{DEAD_PID}-7.tmp"));
        fs::write(&live, b"in flight").unwrap();
        fs::write(&dead, b"crashed").unwrap();
        assert!(is_write_sidecar(&live) && is_write_sidecar(&dead));
        assert!(!is_write_sidecar(&dest));

        reclaim_write_sidecars(dir.path());

        assert!(live.exists());
        assert!(!dead.exists());
        assert_eq!(fs::read(&dest).unwrap(), b"current");
    }

    #[test]
    fn write_atomic_replaces_and_creates_parents_without_leaving_tmp() {
        let dir = ScratchDir::new("atomic-write");
        let dest = dir.path().join("deep").join("cache.bin");
        write_atomic(&dest, b"one").unwrap();
        write_atomic(&dest, b"two").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"two");
        assert_eq!(crate::test_fixtures::count_tundra_sidecars(dest.parent().unwrap()), 0);
    }

    #[test]
    fn unique_sidecar_names_differ_for_same_dest() {
        let dest = Path::new("kick.wav");
        assert_ne!(unique_sidecar(dest, "tag"), unique_sidecar(dest, "tag"));
        assert_eq!(sidecar(dest, TAG_TMP_SUFFIX), Path::new("kick.wav.tundra-tag.tmp"));
    }

    #[test]
    fn reclaim_handles_multiple_files_in_one_directory() {
        let dir = ScratchDir::new("reclaim-many");
        let kick = dir.path().join("kick.wav");
        let snare = dir.path().join("snare.wav");
        fs::write(sidecar(&kick, REPLACE_OLD_SUFFIX), b"k-aside").unwrap();
        fs::write(sidecar(&snare, REPLACE_OLD_SUFFIX), b"s-aside").unwrap();
        fs::write(sidecar(&kick, TAG_TMP_SUFFIX), b"stale").unwrap();

        reclaim_write_sidecars(dir.path());

        assert_eq!(fs::read(&kick).unwrap(), b"k-aside");
        assert_eq!(fs::read(&snare).unwrap(), b"s-aside");
        assert!(!sidecar(&kick, TAG_TMP_SUFFIX).exists());
    }

    #[cfg(unix)]
    #[test]
    fn sync_parent_dir_succeeds_for_existing_directory() {
        let dir = ScratchDir::new("sync-parent");
        sync_parent_dir(&dir.path().join("child.bin")).unwrap();
    }

    #[test]
    fn reclaim_keeps_tmps_when_dest_missing_whatever_their_pid() {
        let dir = ScratchDir::new("reclaim-missing");
        let dest = dir.path().join("missing.wav");
        let dead = dead_pid_tag_tmp(&dest);
        let live = unique_sidecar(&dest, "tag");
        fs::write(&dead, b"stale").unwrap();
        fs::write(&live, b"tmp").unwrap();

        reclaim_write_sidecars(dir.path());

        assert!(!dest.exists());
        assert!(dead.exists(), "may be the only copy of the audio");
        assert!(live.exists(), "live pid tmp must stay when dest is missing");
    }

    #[test]
    fn write_atomic_preserves_existing_file_when_replace_fails() {
        use crate::test_fixtures::with_replace_blocked;

        let dir = ScratchDir::new("atomic-preserve");
        let dest = dir.path().join("cache.bin");
        fs::write(&dest, b"stable").unwrap();

        let err = with_replace_blocked(dir.path(), &dest, || write_atomic(&dest, b"new"));

        assert!(err.is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"stable");
        assert_eq!(dir.sidecar_count(), 0);
    }
}
