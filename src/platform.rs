//! Operating system integration: the file manager, child processes, and
//! finding files shipped beside the executable.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the platform calls "show this file in its folder".
pub fn file_manager_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Open in Explorer"
    } else if cfg!(target_os = "macos") {
        "Open in Finder"
    } else {
        "Open in File browser"
    }
}

/// Opens the system file manager with `path` selected (or opened, for a folder).
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer");
        if path.is_dir() {
            command.arg(path);
        } else {
            command.raw_arg(format!("/select,\"{}\"", path.display()));
        }
        hide_console(&mut command);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        if path.is_file() {
            command.arg("-R");
        }
        command.arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        });
        command
    };
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
    let exe_dir = std::env::current_exe()
        .ok()
        .map(crate::path_util::normalize_path)
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let bundle_resources = exe_dir
        .as_ref()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(|dir| dir.parent())
        .map(|contents| contents.join("Resources"));
    let manifest = Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))).filter(|dir| dir.is_dir());

    let mut roots: Vec<PathBuf> = Vec::new();
    for root in [bundle_resources, exe_dir, manifest].into_iter().flatten() {
        let root = crate::path_util::normalize_path(root);
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
