mod types;
use cauldron::audio::AudioSegment;
use iced::{Container, Element, Length, Row, Command, Settings, Space, Application, executor};
use svg::Document;
use types::*;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    }).unwrap();
}

struct App {
    file_selector: FileSelector,
    audio_svg: Option<Document>,
}

impl Application for App {
    type Message = Message;
    type Executor = executor::Default;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let current_dir = std::env::current_dir().unwrap();
        let file_selector = FileSelector::new(&current_dir);
        let app = App {
            file_selector,
            audio_svg: None,
        };
        (app, Command::none())
    }

    fn title(&self) -> String {
        String::from("Tundra Sample Browser")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::SelectedFile(selected_file) => {
                match &selected_file {
                    Some(file_path) => {
                        if selected_file.as_ref().map_or(false, |fp| fp.is_dir()) {
                            self.file_selector = FileSelector::new(file_path);
                        } else {
                            play_file(&file_path);
                            let mut wave = AudioSegment::read(file_path.to_str().unwrap()).unwrap();
                            let audio_buffer = WaveForm {
                                samples: wave.samples().unwrap().map(|r| r.unwrap()).collect(),
                                bits_per_sample: wave.info().bits_per_sample,
                            };
                            self.audio_svg = Some(audio_buffer.svg());
                            self.file_selector.selected_file = selected_file;
                        }
                    }
                    None => {
                        self.audio_svg = None;
                    }
                }

                Command::none()
            }
            Message::ChangeDirectory(parent_dir) => {
                self.file_selector = FileSelector::new(&parent_dir);
                Command::none()
            }
        }
    }

    fn view(&mut self) -> Element<Message> {
        let svg: Element<Message> = self
            .audio_svg
            .as_ref()
            .map_or(Space::new(Length::Fill, Length::Fill).into(), |document| {
                view_wave_form(document).into()
            });
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
