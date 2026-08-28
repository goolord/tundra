use crate::path_util::{cache_file, read_bincode, write_bincode};
use crate::types::player::clamp_volume;
use crate::types::Message;
use iced::{Task, window};

pub(crate) const DEFAULT_SIDEBAR_WIDTH: f32 = 280.0;
pub(crate) const MIN_SIDEBAR_WIDTH: f32 = 160.0;
pub(crate) const MAX_SIDEBAR_WIDTH: f32 = 720.0;
pub(crate) const SIDEBAR_RESIZER_HIT_WIDTH: f32 = 10.0;
pub(crate) const SIDEBAR_RESIZER_LINE_WIDTH: f32 = 2.0;
pub(crate) const WINDOW_RESIZE_BORDER: f32 = 8.0;
pub(crate) const TITLE_DRAG_THRESHOLD: f32 = 4.0;

fn load_cached_f32(name: &str, default: f32, clamp: impl Fn(f32) -> f32) -> f32 {
    cache_file(name)
        .and_then(|path| read_bincode(&path))
        .filter(|value: &f32| value.is_finite())
        .map(clamp)
        .unwrap_or(default)
}

fn load_cached_bool(name: &str, default: bool) -> bool {
    cache_file(name)
        .and_then(|path| read_bincode(&path))
        .unwrap_or(default)
}

fn persist_cached<T: serde::Serialize>(name: &str, value: &T, label: &str) {
    if let Some(path) = cache_file(name) {
        write_bincode(&path, value, label);
    }
}

pub(crate) fn load_sidebar_width() -> f32 {
    load_cached_f32(
        "sidebar_width.bin",
        DEFAULT_SIDEBAR_WIDTH,
        |width| width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
    )
}

pub(crate) fn persist_sidebar_width(width: f32) {
    let width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
    persist_cached("sidebar_width.bin", &width, "sidebar width");
}

pub(crate) fn load_volume() -> f32 {
    load_cached_f32("volume.bin", 1.0, clamp_volume)
}

pub(crate) fn persist_volume(volume: f32) {
    persist_cached("volume.bin", &clamp_volume(volume), "volume");
}

pub(crate) fn load_looping() -> bool {
    load_cached_bool("looping.bin", false)
}

pub(crate) fn persist_looping(looping: bool) {
    persist_cached("looping.bin", &looping, "loop");
}

pub(crate) fn load_always_on_top() -> bool {
    load_cached_bool("always_on_top.bin", false)
}

pub(crate) fn persist_always_on_top(always_on_top: bool) {
    persist_cached("always_on_top.bin", &always_on_top, "always on top");
}

pub fn window_level(always_on_top: bool) -> window::Level {
    if always_on_top {
        window::Level::AlwaysOnTop
    } else {
        window::Level::Normal
    }
}

pub fn set_window_level(always_on_top: bool) -> Task<Message> {
    let level = window_level(always_on_top);
    window::latest().then(move |id| match id {
        Some(id) => window::set_level(id, level),
        None => Task::none(),
    })
}
