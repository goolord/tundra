mod types;
use iced::{Container, Element, Length, Row, Sandbox, Settings, Space};
use cauldron::audio::AudioSegment;
use svg::Document;
use types::*;

fn main() {
    App::run(Settings::default())
}

struct App {
    file_selector: FileSelector,
    audio_svg: Option<Document>,
}

impl Sandbox for App {
    type Message = Message;

    fn new() -> Self {
        let file_selector = FileSelector::new();
        App { 
            file_selector,
            audio_svg: None,
        }
    }

    fn title(&self) -> String {
        String::from("Tundra Sample Browser")
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SelectedFile(selected_file) => {
                self.audio_svg = match &selected_file {
                    Some(file_path) => {
                        play_file(&file_path);
                        let mut wave = AudioSegment::read(file_path.to_str().unwrap()).unwrap();
                        let audio_buffer = WaveForm {
                            samples: wave.samples().unwrap().map(|r| r.unwrap()).collect(),
                            bits_per_sample: wave.info().bits_per_sample,
                        };
                        Some(audio_buffer.svg())
                    },
                    None => None,
                };
                self.file_selector.selected_file = selected_file
            },
        }
    }

    fn view(&mut self) -> Element<Message> {
        let svg: Element<Message> = self.audio_svg.as_ref().map_or(Space::new(Length::Fill, Length::Fill).into(), |document| view_wave_form(document).into());
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
