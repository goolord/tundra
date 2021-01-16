mod types;
use cauldron::audio::AudioSegment;
use iced::{executor, Application, Command, Container, Element, Length, Row, Settings, Space};
use svg::Document;
use types::*;
use std::io::BufReader;
use std::path::PathBuf;
use std::fs::File;
use rodio::Source;
use std::thread;

pub fn main() {
    App::run(Settings {
        antialiasing: true,
        ..Settings::default()
    })
    .unwrap();
}

struct App {
    file_selector: FileSelector,
    audio_svg: Option<Document>,
}

// todo: abstract this into a player type
// ref: https://github.com/tindleaj/miso/blob/master/src/player.rs
impl App {
    fn play_file(&self, file_path: &PathBuf) -> () {
        let file = File::open(file_path).unwrap();
        thread::spawn(move || {
            let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
            let sink = rodio::Sink::try_new(&stream_handle).unwrap();
            let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
            sink.append(source);
            sink.sleep_until_end()
        });
    }
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
                            self.play_file(&file_path);
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
