mod types;

use iced::Settings;
use iced::pure::Application;
use types::*;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .unwrap();
}
