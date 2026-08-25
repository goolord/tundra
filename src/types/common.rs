use futures::channel::mpsc::UnboundedReceiver;
use futures::future::Aborted;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

const RESOURCES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources");

static RESOURCE_PATHS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    [
        "pause.svg",
        "play.svg",
        "stop.svg",
        "up_chevron.svg",
        "folder-solid.svg",
        "music-solid.svg",
    ]
    .into_iter()
    .map(|name| (name, format!("{RESOURCES}/{name}")))
    .collect()
});

pub fn resource_path(name: &str) -> &str {
    match RESOURCE_PATHS.get(name) {
        Some(path) => path.as_str(),
        None => {
            eprintln!("Unknown resource {name:?}; falling back to manifest path");
            &RESOURCE_PATHS["play.svg"]
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchCompleted(Result<Vec<PathBuf>, Aborted>),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    InvalidateDircache,
    Seek(f64),
    SeekCommit,
    PlayerMsg((
        Option<super::PlayerMsg>,
        Arc<UnboundedReceiver<super::PlayerMsg>>,
    )),
    TogglePlaying,
    StopPlayback,
    DismissError,
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| ext == "flac" || ext == "wav" || ext == "mp3" || ext == "ogg")
}

pub fn is_hidden(entry: &Path) -> bool {
    match entry.file_name() {
        Some(s) => s.to_string_lossy().starts_with('.'),
        None => false,
    }
}

pub fn startup_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        eprintln!("Could not read current directory: {err}");
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    })
}

pub(crate) async fn debounce(duration: std::time::Duration) {
    async_io::Timer::after(duration).await;
}

pub fn truncate_path(path: &Path, max_chars: usize) -> String {
    let rendered = path.display().to_string();
    if rendered.chars().count() <= max_chars {
        return rendered;
    }
    let tail: String = rendered
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}
