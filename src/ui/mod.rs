//! The iced UI. `app` owns the state and routes every `message::Message`;
//! the other modules are views and the state behind them.

mod app;
mod auto_tag;
mod bulk_auto_tag;
mod dialog;
mod file_selector;
mod menu;
mod message;
mod player;
mod selection;
mod settings;
mod style;
mod tag_editor;
mod waveform;
mod widgets;

pub use app::run;

/// How far the pointer must move, in logical pixels, before a press becomes a drag.
const FILE_DRAG_THRESHOLD: f32 = 8.0;
