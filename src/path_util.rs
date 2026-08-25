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
