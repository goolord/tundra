use iced::{container, Color, Container, Element, Length, Row, Sandbox, Settings};
use std::env;
mod types;
use cauldron::audio::AudioSegment;
use types::*;

fn main() {
    App::run(Settings::default())
}

struct App {
    file_selector: FileSelector,
}

impl Sandbox for App {
    type Message = ();

    fn new() -> Self {
        let args: Vec<String> = env::args().collect();
        let mut file_selector = FileSelector::new();
        file_selector.update(FileSelectorMessage::SelectedFile(Some((&args[1]).into())));
        App { file_selector }
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
            .style(DEBUG_BORDER_BOUNDS)
            .center_x()
            .center_y();

        let file_selector_container = Container::new(self.file_selector.view())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(DEBUG_BORDER_BOUNDS)
            .center_x();

        Row::new()
            .push(file_selector_container)
            .push(svg_container)
            .into()
    }
}
