use futures::future::Aborted;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchCompleted(Result<Vec<PathBuf>, Aborted>),
}

pub fn is_audio(x: &OsStr) -> bool {
    let valid_extensions = ["flac", "wav", "mp3", "ogg"];
    valid_extensions.contains(&x.to_string_lossy().as_ref())
}

pub fn is_hidden(entry: &Path) -> bool {
    match entry.file_name() {
        Some(s) => s.to_string_lossy().starts_with('.'),
        None => false,
    }
}
