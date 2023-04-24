#![feature(async_closure)]
#![feature(iter_array_chunks)]

mod source;
mod types;

use iced::Application;
use iced::Settings;
use types::*;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .unwrap();
}
