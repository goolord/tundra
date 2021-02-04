use walkdir::DirEntry;
use cauldron::audio::AudioSegment;
use iced::canvas::*;

use iced::{
    button, scrollable, text_input, Button, Color, Column, Container, Length,
    Point, Rectangle, Scrollable, Text, TextInput,
};
use std::cmp::*;
use std::ffi::OsStr;

use std::fs::{self};

use std::path::PathBuf;


mod style;
pub use style::*;

pub struct WaveForm {
    samples: Vec<i32>,
    bits_per_sample: u32,
}

impl WaveForm {
    pub fn audio_to_path(&self, frame: &Frame) -> Path {
        // let truncate = 100; // (samples.len() as u64).div(100 as u64);
        let max = 2_i32.pow(self.bits_per_sample);
        let translate_y: f32 = (max / 2) as f32;
        let height = frame.height();
        let width = frame.width();
        let scale_height = height / max as f32;
        let scale_width = width / self.samples.len() as f32;
        let mut builder = path::Builder::new();
        self.samples
            .iter()
            .enumerate()
            // .filter(|&(i, _)| (i as u64).div(truncate) == 0)
            .for_each(|(i, s)| {
                let sample = s.to_owned() as f32;
                let point = Point {
                    x: i as f32 * scale_width,
                    y: (sample + translate_y) * scale_height,
                };
                if i % 2 == 0 {
                    builder.move_to(point)
                } else {
                    builder.line_to(point)
                }
            });

        builder.close();
        builder.build()
    }
}

impl Program<Message> for &WaveForm {
    fn draw(&self, bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(bounds.size());
        // frame.scale(0.01);
        // frame.translate(Vector {
        // x: 0.0,
        // y: (max / 2) as f32
        // });
        let path = self.audio_to_path(&frame);
        let stroke = Stroke {
            color: Color::BLACK,
            width: 1.0,
            line_cap: Default::default(),
            line_join: Default::default(),
        };
        frame.stroke(&path, stroke);
        vec![frame.into_geometry()]
    }
}

impl Into<WaveForm> for AudioSegment {
    fn into(mut self) -> WaveForm {
        let number_channels = self.number_channels();
        let number_channels_i32 = number_channels as i32;
        let mut samples: Vec<i32> = Vec::new();
        let all_samples = self
            .samples()
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<i32>>();
        for arr in all_samples.chunks_exact(number_channels) {
            samples.push(arr.iter().sum::<i32>() / number_channels_i32);
        }
        WaveForm {
            samples,
            bits_per_sample: self.info().bits_per_sample,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub scroll_state: scrollable::State,
    pub current_dir: PathBuf,
    pub dir_up: DirUp,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<PathBuf>,
    pub search: text_input::State,
    pub search_value: String,
}

#[derive(Debug, Clone)]
pub struct FileList {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Search(String),
}

fn is_audio(x: &OsStr) -> bool {
    let valid_extensions = ["flac", "wav", "mp3"];
    valid_extensions.contains(&x.to_string_lossy().as_ref())
}

impl DirUp {
    pub fn view(&mut self, cwd: PathBuf) -> Button<Message> {
        Button::new(&mut self.button, Text::new("^^^ Go up"))
            .on_press(Message::ChangeDirectory(match cwd.parent() {
                Some(x) => x.to_path_buf(),
                None => cwd,
            }))
            .width(Length::Fill)
    }
}

impl FileList {
    pub fn list_dir(
        dir: &PathBuf,
    ) -> std::iter::FilterMap<
        std::fs::ReadDir,
        fn(x: std::io::Result<std::fs::DirEntry>) -> Option<std::fs::DirEntry>,
    > {
        fn the_filter(x: std::io::Result<std::fs::DirEntry>) -> Option<std::fs::DirEntry> {
            match x {
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
            }
        }
        fs::read_dir(dir).unwrap().filter_map(the_filter)
    }

    pub fn new(dir: &PathBuf) -> Vec<FileButton> {
        let mut buttons: Vec<FileButton> = FileList::list_dir(dir)
            .map(|x| FileButton::new(x.path()))
            .collect();
        buttons.sort();
        buttons
    }
}

impl FileSelector {

    pub fn new(dir: &PathBuf) -> Self {
        FileSelector {
            scroll_state: scrollable::State::new(),
            current_dir: dir.to_owned(),
            dir_up: DirUp {
                button: button::State::new(),
            },
            file_list: FileList::new(dir),
            selected_file: None,
            search: text_input::State::new(),
            search_value: String::new(),
        }
    }

    pub fn view(&mut self) -> Column<Message> {
        let selected_file = self.selected_file.as_ref();
        let dir_up = Container::new(self.dir_up.view(self.current_dir.to_owned()))
            .padding(5)
            .width(Length::Fill);
        let fs_column = Column::new().push(dir_up);
        let new_col = self.file_list.iter_mut().fold(fs_column, |col, button| {
            let path = button.file_path.to_owned();
            let element: Button<Message> = button.view();
            let mut container = Container::new(element).padding(5).width(Length::Fill);
            if Some(path.canonicalize().unwrap())
                == selected_file.map(|x| x.canonicalize().unwrap())
            {
                container = container.style(SelectedContainer)
            }
            col.push(container)
        });
        let fs = Scrollable::new(&mut self.scroll_state)
            .push(new_col)
            .width(Length::Fill)
            .height(Length::Fill);
        let search = TextInput::new(
            &mut self.search,
            "This is the placeholder...",
            &self.search_value,
            Message::Search,
        );

        Column::new().push(fs).push(search)
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
        .style(FileButton_)
        .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
        .width(Length::Fill)
    }
}

impl PartialOrd for FileButton {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileButton {
    fn cmp(&self, other: &Self) -> Ordering {
        self.file_path.cmp(&other.file_path)
    }
}

pub fn is_hidden(entry: &DirEntry) -> bool {
    entry.file_name()
         .to_str()
         .map(|s| s.starts_with("."))
         .unwrap_or(false)
}
