use cauldron::audio::AudioSegment;
use iced::svg::Handle;
use iced::{Column, Container, Length, Svg, Text};
use std::fs::{self};


use std::path::PathBuf;
use svg::node::element::path::Data;
use svg::Document;

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

pub struct FileSelector {
    pub selected_file: Option<PathBuf>,
}

impl FileSelector {
    pub fn new() -> Self {
        FileSelector {
            selected_file: None,
        }
    }

    pub fn view(self) -> Column<'static, ()> {
        let dir = std::env::current_dir().unwrap();
        fs::read_dir(dir).unwrap().fold(Column::new(), |col, path| {
            let path = path.unwrap();
            match path.path().to_str() {
                Some(path_str) => col.push(
                    Container::new(
                        Text::new(path_str)
                            .size(16)
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .padding(5),
                ),
                None => col,
            }
        })
    }
}
