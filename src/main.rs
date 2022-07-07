#![feature(async_closure)]
mod source;
mod types;

use iced::pure::Application;
use iced::Settings;
use types::*;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .unwrap();
}
