use iced::{Container, Element, Length, Row, Sandbox, Settings};
use std::env;
mod types;
use cauldron::audio::AudioSegment;
use types::*;

fn main() {
    App::run(Settings::default())
}

struct App;

impl Sandbox for App {
    type Message = ();

    fn new() -> Self {
        App
    }

    fn title(&self) -> String {
        String::from("Tundra")
    }

    fn update(&mut self, _message: ()) {}

    fn view(&mut self) -> Element<()> {
        let args: Vec<String> = env::args().collect();
        let mut wave = WaveForm {
            wave: AudioSegment::read(&args[1]).unwrap(),
        };
        let svg = wave.view();
        let svg_container = Container::new(svg)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(10)
            .center_x()
            .center_y();
        let file_selector_container = Container::new(FileSelector::new().view())
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x();
        Row::new()
            .push(svg_container)
            .push(file_selector_container)
            .into()
    }
}
