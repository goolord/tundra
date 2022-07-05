use futures::channel::mpsc::UnboundedReceiver;
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
    PlayerMsg((Option<super::PlayerMsg>, ClonableUnboundedReceiver<super::PlayerMsg>)),
    TogglePlaying,
}

#[derive(Debug)]
pub struct ClonableUnboundedReceiver<T>(pub UnboundedReceiver<T>);

// this probably leaks memory bc an `Arc` underlies the UnboundedReceiver
// but it's the only way i can figure out how to stream commands from
// an UnboundedReceiver ATM so ヽ(゜～゜o)ノ
impl<T> Clone for ClonableUnboundedReceiver<T> {
    fn clone(&self) -> Self {
        unsafe { std::mem::transmute(self) }
    }
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
