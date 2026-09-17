//! Binary entry. The UI lives in `ui/`; the other modules are UI-free services
//! it drives. See ARCHITECTURE.md for a map.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod auto_tag;
mod bulk_auto_tag;
mod drag_out;
mod launch;
mod library;
mod metadata;
mod path_util;
mod playback;
mod tag_store;
mod ui;
mod waveform_peaks;

#[cfg(test)]
mod data_safety_tests;
#[cfg(test)]
mod test_fixtures;

fn main() {
    ui::run()
}
