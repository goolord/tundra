mod types;

use iced::{Application, Settings};
use types::*;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .unwrap();
}

