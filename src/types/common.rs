use futures::channel::mpsc::UnboundedReceiver;
use futures::future::Aborted;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchCompleted(Result<Vec<PathBuf>, Aborted>),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    PlayerMsg((Option<super::PlayerMsg>, ClonableUnboundedReceiver<super::PlayerMsg>)),
    TogglePlaying,
    StopPlayback,
}

#[derive(Debug)]
pub struct ClonableUnboundedReceiver<T>(pub UnboundedReceiver<T>);

// this is horribly jank
// we are doing this pattern matching on the transmute
// so that we don't memory leak the Arc
impl<T> Clone for ClonableUnboundedReceiver<T> {
    fn clone(&self) -> Self {
        unsafe {
            match std::mem::transmute(self) {
                Some(x) => {
                    let foo: Arc<()> = std::sync::Arc::clone(x);
                    ClonableUnboundedReceiver(std::mem::transmute(Some(foo)))
                },
                None => {
                    let foo: Option<Arc<()>> = None;
                    ClonableUnboundedReceiver(std::mem::transmute(foo))
                }
            }
        }
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
