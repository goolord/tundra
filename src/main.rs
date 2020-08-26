mod types;
use iced::{Container, Element, Length, Row, Sandbox, Settings, Space};
use cauldron::audio::AudioSegment;
use types::*;

fn main() {
    App::run(Settings::default())
}

struct App {
    file_selector: FileSelector,
}

impl Sandbox for App {
    type Message = Message;

    fn new() -> Self {
        let file_selector = FileSelector::new();
        App { file_selector }
    }

    fn title(&self) -> String {
        String::from("Tundra")
    }

    fn update(&mut self, message: Message) {
        self.file_selector.update(message)
    }

    fn view(&mut self) -> Element<Message> {
        let svg: Element<Message> = match &self.file_selector.selected_file {
            Some(file) => {
                let mut wave = WaveForm {
                    wave: AudioSegment::read(file.to_str().unwrap()).unwrap(),
                };
                wave.view().into()
            }
            None => Space::new(Length::Fill, Length::Fill).into(),
        };
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
