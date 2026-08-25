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

pub fn file_name_lossy(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
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

/// Directories to search for bundled assets (scripts, models, icons).
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

pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

pub fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) if to.exists() => replace_existing(from, to),
        Err(err) => Err(err),
    }
}

fn replace_existing(from: &Path, to: &Path) -> io::Result<()> {
    let aside = sidecar(to, ".tundra-replace-old");
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(to, &aside)?;
    match std::fs::rename(from, to) {
        Ok(()) => {
            let _ = std::fs::remove_file(&aside);
            Ok(())
        }
        Err(err) => {
            let _ = std::fs::rename(&aside, to);
            Err(err)
        }
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = sidecar(path, ".tmp");
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
