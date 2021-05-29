pub use super::common::*;
pub use super::style::*;
use cauldron::audio::AudioSegment;
use iced::canvas::*;
use iced::{Color, Container, Element, Length, Point, Rectangle, Space};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::thread;

pub struct WaveForm {
    samples: Vec<i32>,
    bits_per_sample: u32,
}

impl WaveForm {
    pub fn audio_to_path(&self, frame: &Frame) -> Path {
        // let truncate = 100; // (samples.len() as u64).div(100 as u64);
        let max = 2_i32.pow(self.bits_per_sample);
        let translate_y = (max / 2) as f32;
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
                if i & 1 == 0 {
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
        //     x: 0.0,
        //     y: (max / 2) as f32
        // });
        let path = self.audio_to_path(&frame);
        let stroke = Stroke {
            color: Color::from_rgb8(0x50, 0x7a, 0xe0),
            width: 1.0,
            line_cap: Default::default(),
            line_join: Default::default(),
        };
        frame.stroke(&path, stroke);
        vec![frame.into_geometry()]
    }
}

impl From<AudioSegment> for WaveForm {
    fn from(mut audio_segment: AudioSegment) -> WaveForm {
        let number_channels = audio_segment.number_channels();
        let mut samples: Vec<i32> = Vec::new();
        let all_samples: Vec<i32> = audio_segment
            .samples()
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for arr in all_samples.chunks_exact(number_channels) {
            samples.push(arr.iter().sum::<i32>() / number_channels as i32);
        }
        WaveForm {
            samples,
            bits_per_sample: audio_segment.info().bits_per_sample,
        }
    }
}

// todo: abstract this into a player type
// ref: https://github.com/tindleaj/miso/blob/master/src/player.rs
pub struct Player {
    pub waveform: Option<WaveForm>,
}

impl Player {
    pub fn new() -> Self {
        Player { waveform: None }
    }

    pub fn view(&self) -> Container<'_, Message> {
        let svg: Element<Message> =
            self.waveform
                .as_ref()
                .map_or(Space::new(Length::Fill, Length::Fill).into(), |wf| {
                    let canvas = Canvas::new(wf).width(Length::Fill).height(Length::Fill);
                    canvas.into()
                });
        let svg_container = Container::new(svg)
            .width(Length::Fill)
            .height(Length::FillPortion(1))
            .style(PlayerContainer)
            .padding(1)
            .center_x()
            .center_y();
        svg_container
    }

    pub fn play_file(&mut self, file_path: PathBuf) {
        let wave = AudioSegment::read(file_path.to_str().unwrap()).unwrap();
        let file = File::open(file_path).unwrap();
        let audio_buffer: WaveForm = wave.into();
        thread::spawn(move || {
            let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
            let sink = rodio::Sink::try_new(&stream_handle).unwrap();
            let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
            sink.append(source);
            sink.sleep_until_end()
        });
        self.waveform = Some(audio_buffer);
    }
}
