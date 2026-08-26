#![cfg_attr(
    all(windows, not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod drag_out;
mod path_util;
mod auto_tag;
mod bulk_auto_tag;
mod metadata;
mod source;
mod tag_store;
mod types;

use types::*;

pub fn main() {
    app()
}
