use cauldron::audio::AudioSegment;
use iced::svg::Handle;
use iced::{button, Button, Column, Container, Element, Length, Svg, Text};
use rodio::Source;
use std::fs::File;
use std::fs::{self};
use std::io::BufReader;
use std::path::PathBuf;
use svg::node::element::path::Data;
use svg::Document;
mod style;
pub use style::*;

pub struct WaveForm {
    pub wave: AudioSegment,
}

impl WaveForm {
    pub fn view(&mut self) -> Svg {
        let samples: Vec<i32> = self.wave.samples().unwrap().map(|r| r.unwrap()).collect();
        let samples_len = samples.len();
        let max = 2_i64.pow(self.wave.info().bits_per_sample);
        let data = audio_to_svg(samples);
        let path = svg::node::element::Path::new()
            .set("fill", "none")
            .set("stroke", "black")
            .set("stroke-width", 5)
            .set("d", data);
        let document = Document::new()
            .set("viewBox", (0, -max, samples_len, max * 2))
            .set("preserveAspectRatio", "none")
            .add(path);
        let svg_data: Vec<u8> = Vec::from(document.to_string());
        Svg::new(Handle::from_memory(svg_data))
    }
}

fn audio_to_svg(samples: Vec<i32>) -> Data {
    // let truncate = 100; // (samples.len() as u64).div(100 as u64);
    samples
        .iter()
        .enumerate()
        // .filter(|&(i, _)| (i as u64).div(truncate) == 0)
        .fold(Data::new(), |data, (i, s)| {
            if i % 2 == 0 {
                data.move_to((i, s.to_owned()))
            } else {
                data.line_to((i, s.to_owned()))
            }
        })
        .close()
}

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FileButton {
    pub file_button: button::State,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
}

impl FileSelector {
    pub fn new() -> Self {
        let dir = std::env::current_dir().unwrap();
        let file_list = fs::read_dir(dir)
            .unwrap()
            .map(|x| FileButton {
                file_button: button::State::new(),
                file_path: x.unwrap().path(),
            })
            .collect();
        FileSelector {
            file_list,
            selected_file: None,
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::SelectedFile(selected_file) => {
                self.selected_file = selected_file;
                // play the file
                match &self.selected_file {
                    Some(file_path) => {
                        let device = rodio::default_output_device().unwrap();
                        let file = File::open(file_path).unwrap();
                        let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
                        rodio::play_raw(&device, source.convert_samples());
                    }
                    None => (),
                }
            }
        }
    }

    pub fn view(&mut self) -> Column<Message> {
        let selected_file = self.selected_file.as_ref();
        self.file_list
            .iter_mut()
            .fold(Column::new(), |col, button| {
                let path = button.file_path.to_owned();
                let element: Button<Message> = button.view();
                let mut container = Container::new(element).padding(5);
                if Some(path.canonicalize().unwrap())
                    == selected_file.map(|x| x.canonicalize().unwrap())
                {
                    container = container.style(SelectedContainer)
                }
                col.push(container)
            })
    }
}

impl FileButton {
    pub fn view(&mut self) -> Button<Message> {
        Button::new(
            &mut self.file_button,
            Text::new(self.file_path.to_str().unwrap()),
        )
        .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
    }
}
