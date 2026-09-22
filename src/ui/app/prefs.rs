//! Small UI preferences saved between runs, and window helpers.

use crate::app_data::{cache_file, read_bincode, write_bincode};
use crate::playback::clamp_volume;
use crate::ui::message::Message;
use iced::{Task, window};
use serde::Serialize;
use serde::de::DeserializeOwned;

const DEFAULT_SIDEBAR_WIDTH: f32 = 280.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 160.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 720.0;

/// One value stored in its own cache file.
pub struct Pref<T: 'static> {
    file: &'static str,
    label: &'static str,
    default: T,
    /// Applied on load and save; turns a bad stored value into a valid one.
    clean: fn(T) -> T,
}

impl<T: Copy + Serialize + DeserializeOwned> Pref<T> {
    pub fn load(&self) -> T {
        cache_file(self.file)
            .and_then(|path| read_bincode(&path))
            .map_or(self.default, self.clean)
    }

    pub fn save(&self, value: T) {
        if let Some(path) = cache_file(self.file) {
            write_bincode(&path, &(self.clean)(value), self.label);
        }
    }
}

pub const SIDEBAR_WIDTH: Pref<f32> = Pref {
    file: "sidebar_width.bin",
    label: "sidebar width",
    default: DEFAULT_SIDEBAR_WIDTH,
    clean: |width| {
        if width.is_finite() {
            width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
        } else {
            DEFAULT_SIDEBAR_WIDTH
        }
    },
};
pub const VOLUME: Pref<f32> = Pref {
    file: "volume.bin",
    label: "volume",
    default: 1.0,
    clean: |volume| if volume.is_finite() { clamp_volume(volume) } else { 1.0 },
};
pub const LOOPING: Pref<bool> = Pref {
    file: "looping.bin",
    label: "loop",
    default: false,
    clean: |looping| looping,
};
pub const ALWAYS_ON_TOP: Pref<bool> = Pref {
    file: "always_on_top.bin",
    label: "always on top",
    default: false,
    clean: |always_on_top| always_on_top,
};

pub fn window_level(always_on_top: bool) -> window::Level {
    if always_on_top {
        window::Level::AlwaysOnTop
    } else {
        window::Level::Normal
    }
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

    #[test]
    fn bad_stored_values_are_cleaned() {
        assert_eq!((SIDEBAR_WIDTH.clean)(f32::NAN), DEFAULT_SIDEBAR_WIDTH);
        assert_eq!((SIDEBAR_WIDTH.clean)(10_000.0), MAX_SIDEBAR_WIDTH);
        assert_eq!((VOLUME.clean)(f32::INFINITY), 1.0);
    }
}
