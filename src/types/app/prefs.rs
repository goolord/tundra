//! Sidebar width, volume, loop, always-on-top prefs.

use super::cache::cache_file;
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
pub(crate) const FILE_DRAG_THRESHOLD: f32 = 8.0;

fn load_cached_f32(name: &str, default: f32, clamp: impl Fn(f32) -> f32) -> f32 {
    let Some(path) = cache_file(name) else {
        return default;
    };
    match std::fs::read(path) {
        Ok(bytes) => bincode::deserialize::<f32>(&bytes)
            .ok()
            .filter(|value| value.is_finite())
            .map(clamp)
            .unwrap_or(default),
        Err(_) => default,
    }
}

fn load_cached_bool(name: &str, default: bool) -> bool {
    let Some(path) = cache_file(name) else {
        return default;
    };
    match std::fs::read(path) {
        Ok(bytes) => bincode::deserialize(&bytes).unwrap_or(default),
        Err(_) => default,
    }
}

fn persist_cached_bool(name: &str, value: bool, label: &str) {
    let Some(path) = cache_file(name) else {
        return;
    };
    let Ok(bytes) = bincode::serialize(&value) else {
        eprintln!("Failed to serialize {label}");
        return;
    };
    if let Err(err) = crate::path_util::write_atomic(&path, &bytes) {
        eprintln!("Failed to write {label}: {err}");
    }
}

fn persist_cached_f32(name: &str, value: f32, label: &str) {
    let Some(path) = cache_file(name) else {
        return;
    };
    let Ok(bytes) = bincode::serialize(&value) else {
        eprintln!("Failed to serialize {label}");
        return;
    };
    if let Err(err) = crate::path_util::write_atomic(&path, &bytes) {
        eprintln!("Failed to write {label}: {err}");
    }
}

pub(crate) struct SidebarSettings;

impl SidebarSettings {
    pub(crate) fn load() -> f32 {
        load_cached_f32(
            "sidebar_width.bin",
            DEFAULT_SIDEBAR_WIDTH,
            |width| width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
        )
    }

    pub(crate) fn persist(width: f32) {
        let width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        persist_cached_f32("sidebar_width.bin", width, "sidebar width");
    }
}

pub(crate) struct VolumeSettings;

impl VolumeSettings {
    pub(crate) fn load() -> f32 {
        load_cached_f32("volume.bin", 1.0, clamp_volume)
    }

    pub(crate) fn persist(volume: f32) {
        persist_cached_f32("volume.bin", clamp_volume(volume), "volume");
    }
}

pub(crate) struct LoopSettings;

impl LoopSettings {
    pub(crate) fn load() -> bool {
        load_cached_bool("looping.bin", false)
    }

    pub(crate) fn persist(looping: bool) {
        persist_cached_bool("looping.bin", looping, "loop");
    }
}

pub(crate) struct AlwaysOnTopSettings;

impl AlwaysOnTopSettings {
    pub(crate) fn load() -> bool {
        load_cached_bool("always_on_top.bin", false)
    }

    pub(crate) fn persist(always_on_top: bool) {
        persist_cached_bool("always_on_top.bin", always_on_top, "always on top");
    }
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
