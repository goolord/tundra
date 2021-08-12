use std::ffi::OsStr;
use std::path::PathBuf;
use walkdir::DirEntry;
use futures::future::{Abortable, AbortHandle, Aborted, AbortRegistration};

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchCompleted(Result<Vec<PathBuf>, Aborted>)
}

pub fn is_audio(x: &OsStr) -> bool {
    let valid_extensions = ["flac", "wav", "mp3", "ogg"];
    valid_extensions.contains(&x.to_string_lossy().as_ref())
}

pub fn is_hidden(entry: &DirEntry) -> bool {
    match entry.file_name().to_str() {
        Some(s) => s.starts_with('.'),
        None => false,
    }
}
