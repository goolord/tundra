use iced::svg::Handle;
use iced::{button, canvas, Button, Canvas, Column, Container, Length, Svg, Text};
use rodio::Source;
use std::ffi::OsStr;
use std::fs::File;
use std::fs::{self};
use std::io::BufReader;
use std::path::PathBuf;
use svg::node::element::path::Data;
use svg::Document;
mod style;
pub use style::*;

pub struct WaveForm {
    pub samples: Vec<i32>,
    pub bits_per_sample: u32,
}

impl WaveForm {
    pub fn svg(self) -> Document {
        let max = 2_i64.pow(self.bits_per_sample);
        let data = audio_to_svg(&self.samples);
        let path = svg::node::element::Path::new()
            .set("fill", "none")
            .set("stroke", "black")
            .set("stroke-width", 5)
            .set("d", data);
        Document::new()
            .set("viewBox", (0, -max, self.samples.len(), max * 2))
            .set("preserveAspectRatio", "none")
            .add(path)
    }
}

pub fn view_wave_form(document: &Document) -> Svg {
    let svg_data: Vec<u8> = Vec::from(document.to_string());
    Svg::new(Handle::from_memory(svg_data))
        .width(Length::Fill)
        .height(Length::Fill)
}

fn audio_to_svg(samples: &Vec<i32>) -> Data {
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
    pub current_dir: PathBuf,
    pub dir_up: DirUp,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FileButton {
    pub file_button: button::State,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DirUp {
    pub button: button::State,
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
}

pub fn play_file(file_path: &PathBuf) {
    let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
    let file = File::open(file_path).unwrap();
    let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
    stream_handle.play_raw(source.convert_samples()).unwrap();
}

fn is_audio(x: &OsStr) -> bool {
    let valid_extensions = ["flac", "wav", "mp3"];
    valid_extensions.contains(&x.to_string_lossy().as_ref())
}

impl DirUp {
    pub fn view(&mut self, cwd: PathBuf) -> Button<Message> {
        Button::new(&mut self.button, Text::new("^^^")).on_press(Message::ChangeDirectory(
            match cwd.parent() {
                Some(x) => x.to_path_buf(),
                None => cwd,
            }
        ))
    }
}

impl FileSelector {
    pub fn new(dir: &PathBuf) -> Self {
        let file_list = fs::read_dir(dir)
            .unwrap()
            .filter_map(|x| match x {
                Ok(x) => {
                    let x_is_dir = x.path().is_dir();
                    let x_is_audio = x.path().extension().map_or(false, is_audio);
                    if x_is_dir || x_is_audio {
                        Some(x)
                    } else {
                        None
                    }
                }
                Err(_) => None,
            })
            .map(|x| FileButton::new(x.path()))
            .collect();
        FileSelector {
            current_dir: dir.to_owned(),
            dir_up: DirUp {
                button: button::State::new(),
            },
            file_list,
            selected_file: None,
        }
    }

    pub fn view(&mut self) -> Column<Message> {
        let selected_file = self.selected_file.as_ref();
        let dir_up = Container::new(self.dir_up.view(self.current_dir.to_owned())).padding(5);
        let column = Column::new().push(dir_up);
        self.file_list.iter_mut().fold(column, |col, button| {
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
    pub fn new(x: PathBuf) -> Self {
        FileButton {
            file_button: button::State::new(),
            file_path: x,
        }
    }

    pub fn view(&mut self) -> Button<Message> {
        Button::new(
            &mut self.file_button,
            Text::new(self.file_path.to_str().unwrap()),
        )
        .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
    }
}
