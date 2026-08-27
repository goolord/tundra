//! iced UI.

mod app;
mod auto_tag;
mod bulk_auto_tag;
mod common;
mod file_selector;
mod menu;
mod player;
mod settings;
mod tag_editor;
mod waveform;

pub use app::*;
pub use common::{is_audio, AUDIO_EXTENSIONS};
pub use file_selector::*;
pub use menu::*;
pub use waveform::WaveFormView;
pub use player::*;
