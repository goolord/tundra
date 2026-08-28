use std::io::{self, Write};
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

pub fn canonical_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    path.canonicalize().map(normalize_path)
}

/// Case-fold path keys on Windows/macOS default volumes.
pub fn cache_key(path: PathBuf) -> PathBuf {
    let path = normalize_path(path);
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
    let raw = path.to_path_buf();
    let key = cache_key(raw.clone());
    if key == raw {
        vec![raw]
    } else {
        vec![raw, key]
    }
}

/// True when `path` is `root` or a descendant, ignoring `\\?\` and case.
pub fn is_under(path: &Path, root: &Path) -> bool {
    let path = cache_key(path.to_path_buf());
    let root = cache_key(root.to_path_buf());
    path.starts_with(root)
}

fn tundra_app_dir(base: Option<PathBuf>, require_create: bool) -> Option<PathBuf> {
    let mut dir = base?;
    dir.push("tundra");
    if require_create {
        std::fs::create_dir_all(&dir).ok()?;
    } else {
        let _ = std::fs::create_dir_all(&dir);
    }
    Some(dir)
}

pub fn tundra_cache_dir() -> Option<PathBuf> {
    tundra_app_dir(dirs::cache_dir(), false)
}

pub fn tundra_config_dir() -> Option<PathBuf> {
    tundra_app_dir(dirs::config_dir(), false)
}

pub fn tundra_data_dir() -> Option<PathBuf> {
    tundra_app_dir(dirs::data_dir(), true)
}

pub fn cache_file(name: &str) -> Option<PathBuf> {
    tundra_cache_dir().map(|mut path| {
        path.push(name);
        path
    })
}

pub fn config_file(name: &str) -> Option<PathBuf> {
    tundra_config_dir().map(|mut path| {
        path.push(name);
        path
    })
}

pub fn read_bincode<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    bincode::deserialize(&std::fs::read(path).ok()?).ok()
}

pub fn read_bincode_or_default<T: Default + serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
) -> T {
    match std::fs::read(path) {
        Ok(bytes) => match bincode::deserialize(&bytes) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "Failed to deserialize {label} ({}): {err}",
                    path.display()
                );
                T::default()
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => T::default(),
        Err(err) => {
            eprintln!("Failed to read {label} ({}): {err}", path.display());
            T::default()
        }
    }
}

pub fn write_bincode<T: serde::Serialize>(path: &Path, value: &T, label: &str) -> bool {
    let Ok(bytes) = bincode::serialize(value) else {
        eprintln!("Failed to serialize {label}");
        return false;
    };
    if let Err(err) = write_atomic(path, &bytes) {
        eprintln!("Failed to write {label}: {err}");
        return false;
    }
    true
}

pub fn file_name_lossy(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

pub fn file_label(path: &Path) -> String {
    file_name_lossy(path).unwrap_or_else(|| path.display().to_string())
}

pub fn path_io_error(verb: &str, path: &Path, err: impl std::fmt::Display) -> String {
    format!("Failed to {verb} {}: {err}", path.display())
}

pub fn open_file(path: &Path) -> Result<std::fs::File, String> {
    std::fs::File::open(path).map_err(|err| path_io_error("open", path, err))
}

pub fn file_stem_lossy(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

pub fn hide_console(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn manifest_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.is_dir().then_some(dir)
}

/// Directories to search for bundled assets (scripts, models).
pub fn exe_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut push = |path: PathBuf| {
        let path = normalize_path(path);
        if !roots.iter().any(|existing| existing == &path) {
            roots.push(path);
        }
    };

    if let Ok(exe) = std::env::current_exe() {
        let exe = normalize_path(exe);
        if let Some(parent) = exe.parent() {
            if parent.file_name().and_then(|name| name.to_str()) == Some("MacOS") {
                if let Some(contents) = parent.parent() {
                    push(contents.join("Resources"));
                }
            }
            push(parent.to_path_buf());
        }
    }

    if let Some(manifest) = manifest_dir() {
        push(manifest);
    }
    roots
}

pub fn find_beside(
    relatives: &[&str],
    predicate: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for root in exe_search_roots() {
        for relative in relatives {
            let candidate = root.join(relative);
            if predicate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

pub const TAG_TMP_SUFFIX: &str = ".tundra-tag.tmp";
/// Legacy only: older builds wrote this, reclaim still deletes it.
pub const TAG_BAK_SUFFIX: &str = ".tundra-tag.bak";
pub const REPLACE_OLD_SUFFIX: &str = ".tundra-replace-old";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteSidecarKind {
    Tmp,
    Bak,
    ReplaceOld,
}

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
    sidecar(
        path,
        &format!(".tundra-{kind}-{}-{seq}.tmp", std::process::id()),
    )
}

fn parse_write_sidecar(name: &str) -> Option<(&str, WriteSidecarKind, Option<u32>)> {
    if let Some(dest) = name.strip_suffix(REPLACE_OLD_SUFFIX) {
        return (!dest.is_empty()).then_some((dest, WriteSidecarKind::ReplaceOld, None));
    }
    if let Some(dest) = name.strip_suffix(TAG_BAK_SUFFIX) {
        return (!dest.is_empty()).then_some((dest, WriteSidecarKind::Bak, None));
    }
    if let Some(dest) = name.strip_suffix(TAG_TMP_SUFFIX) {
        return (!dest.is_empty()).then_some((dest, WriteSidecarKind::Tmp, None));
    }
    let rest = name.strip_suffix(".tmp")?;
    let marker = ".tundra-tag-";
    let index = rest.rfind(marker)?;
    let dest = &rest[..index];
    if dest.is_empty() {
        return None;
    }
    let meta = &rest[index + marker.len()..];
    let pid = meta.split('-').next()?.parse().ok();
    Some((dest, WriteSidecarKind::Tmp, pid))
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
        return windows_pid_is_alive(pid);
    }
    #[cfg(unix)]
    {
        let path = std::path::PathBuf::from(format!("/proc/{pid}"));
        if path.exists() {
            return true;
        }
        #[cfg(target_os = "macos")]
        {
            let mut command = std::process::Command::new("kill");
            command.args(["-0", &pid.to_string()]);
            hide_console(&mut command);
            return command.status().is_ok_and(|status| status.success());
        }
        #[cfg(not(target_os = "macos"))]
        {
            return false;
        }
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
        Err(err) => {
            #[cfg(windows)]
            if to.exists() {
                return replace_existing_windows(from, to);
            }
            Err(err)
        }
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

    let replaced = wide(to);
    let replacement = wide(from);
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
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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
    use std::fs::OpenOptions;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()
}

pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));

    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()
    }

    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        OpenOptions::new()
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
pub fn reclaim_write_sidecars(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut groups: std::collections::HashMap<
        PathBuf,
        Vec<(PathBuf, WriteSidecarKind, Option<u32>)>,
    > = std::collections::HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = file_name_lossy(&path) else {
            continue;
        };
        let Some((dest_name, kind, pid)) = parse_write_sidecar(&name) else {
            continue;
        };
        let dest = path.with_file_name(dest_name);
        groups.entry(dest).or_default().push((path, kind, pid));
    }

    for (dest, sidecars) in groups {
        if !dest.exists() {
            if let Some((source, _, _)) = sidecars
                .iter()
                .find(|(_, kind, _)| *kind == WriteSidecarKind::ReplaceOld)
            {
                if std::fs::rename(source, &dest).is_err() {
                    if std::fs::copy(source, &dest).is_ok() {
                        let _ = std::fs::remove_file(source);
                    }
                }
            }
        }

        let dest_exists = dest.exists();
        for (path, kind, pid) in sidecars {
            match kind {
                WriteSidecarKind::Tmp => {
                    let live = pid.is_some_and(pid_is_alive);
                    if live {
                        continue;
                    }
                    if dest_exists || pid.is_some() {
                        let _ = std::fs::remove_file(path);
                    }
                }
                WriteSidecarKind::Bak | WriteSidecarKind::ReplaceOld => {
                    if dest_exists {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
    }
}

/// Reclaim each directory before a later WalkDir lists its children.
pub fn reclaim_write_sidecars_tree(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        reclaim_write_sidecars(&dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                if path.is_dir() {
                    stack.push(path);
                }
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = unique_sidecar(path, "atomic");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    let result = replace_file(&tmp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::{dead_pid_tag_tmp, ScratchDir};
    use std::fs;

    #[test]
    fn is_under_ignores_verbatim_prefix_and_case() {
        let root = PathBuf::from(r"\\?\F:\Samples");
        let child = PathBuf::from(r"F:\Samples\ADM Samples - Copy\Snare\01_Snare.flac");
        assert!(is_under(&child, &root));
        assert!(is_under(&root, &root));
        assert!(!is_under(
            Path::new(r"F:\Other\snare.flac"),
            &root
        ));
    }

    #[test]
    fn cache_lookup_keys_include_normalized_form() {
        let path = PathBuf::from(r"\\?\F:\Samples\Snare\01_Snare.flac");
        let keys = cache_lookup_keys(&path);
        assert!(keys.contains(&path));
        assert!(keys.contains(&cache_key(path)));
    }

    #[test]
    fn replace_works_when_dest_readonly() {
        let dir = ScratchDir::new("replace-readonly");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"audio").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::write(&tmp, b"tagged").unwrap();
        let mut perms = fs::metadata(&dest).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dest, perms).unwrap();

        ensure_writable(&dest).unwrap();
        replace_file(&tmp, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"tagged");
    }

    #[test]
    fn sync_file_works_on_readonly_copy() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"audio").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::copy(&dest, &tmp).unwrap();
        let mut perms = fs::metadata(&tmp).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&tmp, perms).unwrap();

        ensure_writable(&tmp).unwrap();
        fs::write(&tmp, b"tagged").unwrap();
        sync_file(&tmp).unwrap();
    }

    #[test]
    fn reclaim_keeps_live_pid_tmp_when_dest_exists() {
        let dir = ScratchDir::new("path-util");
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
        let dir = ScratchDir::new("path-util");
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
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("snare.wav");
        fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"tmp-maybe-corrupt").unwrap();
        fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak-original").unwrap();
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"aside-original").unwrap();

        reclaim_write_sidecars(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"aside-original");
        assert!(
            !sidecar(&dest, TAG_TMP_SUFFIX).exists(),
            "legacy tmp is deleted once dest is present"
        );
        assert!(!sidecar(&dest, TAG_BAK_SUFFIX).exists());
        assert!(!sidecar(&dest, REPLACE_OLD_SUFFIX).exists());
    }

    #[test]
    fn reclaim_does_not_resurrect_from_tmp_or_bak() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("hat.wav");
        fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"tagged-copy").unwrap();
        fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak-original").unwrap();

        reclaim_write_sidecars(dir.path());

        assert!(!dest.exists());
        assert!(sidecar(&dest, TAG_TMP_SUFFIX).exists());
        assert!(sidecar(&dest, TAG_BAK_SUFFIX).exists());
    }

    #[cfg(unix)]
    #[test]
    fn reclaim_keeps_sidecars_when_restore_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("rim.wav");
        let aside = sidecar(&dest, REPLACE_OLD_SUFFIX);
        fs::write(&aside, b"aside-original").unwrap();
        let tmp = sidecar(&dest, TAG_TMP_SUFFIX);
        fs::write(&tmp, b"tmp").unwrap();

        let readonly = fs::Permissions::from_mode(0o555);
        fs::set_permissions(dir.path(), readonly).unwrap();
        reclaim_write_sidecars(dir.path());
        let writable = fs::Permissions::from_mode(0o755);
        fs::set_permissions(dir.path(), writable).unwrap();

        assert!(!dest.exists());
        assert!(aside.exists());
        assert!(tmp.exists());
    }

    #[test]
    fn reclaim_tree_restores_nested_replace_old() {
        let dir = ScratchDir::new("path-util");
        let nested = dir.path().join("drums");
        fs::create_dir(&nested).unwrap();
        let dest = nested.join("kick.wav");
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"aside").unwrap();

        reclaim_write_sidecars_tree(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"aside");
    }

    #[cfg(unix)]
    #[test]
    fn reclaim_tree_follows_symlink_directory() {
        let dir = ScratchDir::new("path-util");
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let dest = real.join("kick.wav");
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"aside").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();

        reclaim_write_sidecars_tree(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"aside");
    }

    #[test]
    fn write_atomic_writes_and_replaces_without_leaving_tmp() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("cache.bin");
        write_atomic(&dest, b"one").unwrap();
        write_atomic(&dest, b"two").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"two");
        let sidecars = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".tundra-")
            })
            .count();
        assert_eq!(sidecars, 0);
    }

    #[test]
    fn write_atomic_creates_parent_directories() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("deep").join("nested.bin");
        write_atomic(&dest, b"payload").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"payload");
    }

    #[test]
    fn unique_sidecar_names_differ_for_same_dest() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("kick.wav");
        let a = unique_sidecar(&dest, "tag");
        let b = unique_sidecar(&dest, "tag");
        assert_ne!(a, b);
    }

    #[test]
    fn sidecar_appends_suffix_to_file_name() {
        let dir = ScratchDir::new("sidecar-name");
        let path = dir.path().join("kick.wav");
        assert_eq!(
            sidecar(&path, TAG_TMP_SUFFIX),
            dir.path().join("kick.wav.tundra-tag.tmp")
        );
    }

    #[test]
    fn reclaim_cleans_legacy_tmp_once_dest_exists() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("kick.wav");
        fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"aside").unwrap();
        fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"legacy").unwrap();

        reclaim_write_sidecars(dir.path());

        assert_eq!(fs::read(&dest).unwrap(), b"aside");
        assert!(!sidecar(&dest, TAG_TMP_SUFFIX).exists());
    }

    #[test]
    fn reclaim_leaves_dest_missing_when_only_tmp_and_bak_exist() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("missing.wav");
        fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"tmp").unwrap();
        fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak").unwrap();

        reclaim_write_sidecars(dir.path());

        assert!(!dest.exists());
        assert!(sidecar(&dest, TAG_TMP_SUFFIX).exists());
        assert!(sidecar(&dest, TAG_BAK_SUFFIX).exists());
    }

    #[test]
    fn reclaim_handles_multiple_files_in_one_directory() {
        let dir = ScratchDir::new("path-util");
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
    fn reclaim_deletes_dead_pid_tmp_when_dest_missing() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("missing.wav");
        let tmp = dead_pid_tag_tmp(&dest);
        fs::write(&tmp, b"stale").unwrap();
        assert!(!dest.exists());

        reclaim_write_sidecars(dir.path());

        assert!(!tmp.exists());
        assert!(!dest.exists());
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

    #[test]
    fn reclaim_keeps_live_pid_tmp_when_dest_missing() {
        let dir = ScratchDir::new("live-missing");
        let dest = dir.path().join("missing.wav");
        let tmp = unique_sidecar(&dest, "tag");
        fs::write(&tmp, b"tmp").unwrap();

        reclaim_write_sidecars(dir.path());

        assert!(!dest.exists());
        assert!(tmp.exists(), "live pid tmp must stay when dest is missing");
    }

    #[test]
    fn write_atomic_removes_tmp_when_replace_fails() {
        let dir = ScratchDir::new("atomic-fail");
        let dest = dir.path().join("blocked.bin");
        fs::create_dir(&dest).unwrap();

        let err = write_atomic(&dest, b"payload");
        assert!(err.is_err());
        assert_eq!(dir.sidecar_count(), 0);
    }

    #[test]
    fn ensure_writable_allows_subsequent_write() {
        let dir = ScratchDir::new("path-util");
        let dest = dir.path().join("kick.wav");
        fs::write(&dest, b"audio").unwrap();
        let mut perms = fs::metadata(&dest).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dest, perms).unwrap();

        ensure_writable(&dest).unwrap();
        fs::write(&dest, b"updated").unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"updated");
    }
}
