#![cfg_attr(
    all(windows, not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod drag_out;
mod launch;
mod path_util;
mod auto_tag;
mod bulk_auto_tag;
mod metadata;
mod source;
mod tag_store;
mod types;
mod waveform_peaks;

#[cfg(test)]
mod data_safety_tests;
#[cfg(test)]
mod test_fixtures;

use types::*;

pub fn main() {
    app()
}
