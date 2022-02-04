use futures::future::Aborted;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchCompleted(Result<Vec<PathBuf>, Aborted>),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    TogglePlaying,
}

pub fn is_audio(x: &OsStr) -> bool {
    let valid_extensions = ["flac", "wav", "mp3", "ogg"];
    let x_str = x.to_string_lossy();
    valid_extensions.iter().any(|&s| x_str.ends_with(s))
}

pub fn is_hidden(entry: &Path) -> bool {
    match entry.file_name() {
        Some(s) => s.to_string_lossy().starts_with('.'),
        None => false,
    }
}
