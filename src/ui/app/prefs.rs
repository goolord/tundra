//! Small UI preferences saved between runs, and window helpers.

use crate::path_util::{cache_file, read_bincode, write_bincode};
use crate::playback::clamp_volume;
use crate::ui::message::Message;
use iced::{window, Task};
use serde::de::DeserializeOwned;
use serde::Serialize;

const DEFAULT_SIDEBAR_WIDTH: f32 = 280.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 160.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 720.0;

fn load<T: DeserializeOwned>(name: &str) -> Option<T> {
    cache_file(name).and_then(|path| read_bincode(&path))
}

fn save<T: Serialize>(name: &str, value: &T, label: &str) {
    if let Some(path) = cache_file(name) {
        write_bincode(&path, value, label);
    }
}

pub fn load_sidebar_width() -> f32 {
    load::<f32>("sidebar_width.bin")
        .filter(|width| width.is_finite())
        .map_or(DEFAULT_SIDEBAR_WIDTH, |width| width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH))
}

pub fn persist_sidebar_width(width: f32) {
    save("sidebar_width.bin", &width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH), "sidebar width");
}

pub fn load_volume() -> f32 {
    load::<f32>("volume.bin").filter(|volume| volume.is_finite()).map_or(1.0, clamp_volume)
}

pub fn persist_volume(volume: f32) {
    save("volume.bin", &clamp_volume(volume), "volume");
}

pub fn load_looping() -> bool {
    load("looping.bin").unwrap_or(false)
}

pub fn persist_looping(looping: bool) {
    save("looping.bin", &looping, "loop");
}

pub fn load_always_on_top() -> bool {
    load("always_on_top.bin").unwrap_or(false)
}

pub fn persist_always_on_top(always_on_top: bool) {
    save("always_on_top.bin", &always_on_top, "always on top");
}

pub fn window_level(always_on_top: bool) -> window::Level {
    if always_on_top { window::Level::AlwaysOnTop } else { window::Level::Normal }
}

/// Run `task` against the app window, or do nothing if there is none yet.
pub fn on_window(task: impl Fn(window::Id) -> Task<Message> + Send + 'static) -> Task<Message> {
    window::latest().then(move |id| id.map_or_else(Task::none, &task))
}

pub fn set_window_level(always_on_top: bool) -> Task<Message> {
    let level = window_level(always_on_top);
    on_window(move |id| window::set_level(id, level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_level_follows_flag() {
        assert_eq!(window_level(true), window::Level::AlwaysOnTop);
        assert_eq!(window_level(false), window::Level::Normal);
    }
}
