//! Operating system integration: the file manager, child processes, and
//! finding files shipped beside the executable.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the platform calls "show this file in its folder".
pub fn file_manager_label() -> &'static str {
    if cfg!(windows) {
        "Open in Explorer"
    } else if cfg!(target_os = "macos") {
        "Open in Finder"
    } else {
        "Open in File browser"
    }
}

/// Opens the system file manager with `path` selected (or opened, for a folder).
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(windows)]
    let mut command = Command::new("explorer");
    #[cfg(windows)]
    if path.is_dir() {
        command.arg(path);
    } else {
        use std::os::windows::process::CommandExt;
        command.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "macos")]
    command.args(path.is_file().then_some("-R")).arg(path);
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    #[cfg(all(unix, not(target_os = "macos")))]
    command.arg(if path.is_dir() { path } else { path.parent().unwrap_or(path) });
    hide_console(&mut command);
    if let Err(err) = command.spawn() {
        eprintln!("Could not open the file manager for {}: {err}", path.display());
    }
}

/// Keeps a child process from flashing a console window on Windows.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Directories to search for bundled assets (scripts, models): beside the
/// executable, a macOS bundle's `Resources`, then the source tree in dev builds.
fn search_roots() -> Vec<PathBuf> {
    use crate::path_util::normalize_path;
    let exe_dir = std::env::current_exe().ok().and_then(|exe| Some(normalize_path(exe).parent()?.to_path_buf()));
    let bundle_resources =
        exe_dir.as_ref().filter(|dir| dir.ends_with("MacOS")).and_then(|dir| Some(dir.parent()?.join("Resources")));
    let manifest = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))).filter(|dir| dir.is_dir());

    let mut roots = Vec::new();
    for root in [bundle_resources, exe_dir, manifest].into_iter().flatten().map(normalize_path) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// The first `<root>/<relative>` that satisfies `predicate`, over every search root.
pub fn find_beside(relatives: &[&str], predicate: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    search_roots()
        .into_iter()
        .flat_map(|root| relatives.iter().map(move |relative| root.join(relative)))
        .find(|candidate| predicate(candidate))
}
