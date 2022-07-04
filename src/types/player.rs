pub use super::common::*;
pub use super::style::*;
use iced::pure::widget::canvas::*;
use iced::pure::widget::{
    Button, Column, Container, Space, Svg,
};
use iced::pure::Element;
use iced::{Color, Length, Point, Rectangle};
use rodio::Source;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::thread;
use std::sync::mpsc::channel;

pub struct WaveForm {
    samples: Vec<i16>,
    bits_per_sample: u32,
}

impl WaveForm {
    pub fn to_path(&self, frame: &Frame) -> Path {
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

impl Program<Message> for WaveForm {
    type State = ();
    fn draw(&self, _state: &(), bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(bounds.size());
        // frame.scale(0.01);
        // frame.translate(Vector {
        //     x: 0.0,
        //     y: (max / 2) as f32
        // });
        let path = self.to_path(&frame);
        let stroke = Stroke {
            color: Color::from_rgb8(0x50, 0x7a, 0xe0),
            width: 1.0,
            line_cap: Default::default(),
            line_join: Default::default(),
            line_dash: Default::default(),
        };
        frame.stroke(&path, stroke);
        vec![frame.into_geometry()]
    }
}

impl From<rodio::Decoder<std::io::BufReader<File>>> for WaveForm {
    fn from(decoder: rodio::Decoder<std::io::BufReader<File>>) -> WaveForm {
        let number_channels = decoder.channels();
        let all_samples: Vec<i16> = decoder.into_iter().collect();
        let mut samples: Vec<i16> = Vec::with_capacity(all_samples.len());
        for arr in all_samples.chunks_exact(number_channels as usize) {
            samples.push(arr.iter().sum::<i16>() / number_channels as i16);
        }
        WaveForm {
            samples,
            bits_per_sample: 16,
        }
    }
}

// todo: abstract this into a player type
// ref: https://github.com/tindleaj/miso/blob/master/src/player.rs
pub struct Player {
    pub waveform: Option<WaveForm>,
    pub controls: Controls,
    pub sender: Sender<PlayerCommand>,
    pub receiver: Receiver<PlayerCommand>,
}

pub enum PlayerCommand {
    Play,
    Pause,
}

pub struct Controls {
    pub is_playing: sync::Arc<sync::atomic::AtomicBool>,
    pub volume: f32,
}

impl Controls {
    pub fn new() -> Self {
        Controls {
            is_playing: sync::Arc::new(false.into()),
            volume: f32::MAX,
        }
    }

    pub fn play_button(&self) -> Button<Message> {
        let playing = self.is_playing.load(sync::atomic::Ordering::SeqCst);
        let label = if playing {
            Svg::from_path("./resources/pause.svg")
        } else {
            Svg::from_path("./resources/play.svg")
        }
        .width(Length::Units(24))
        .height(Length::Units(24));
        Button::new(label).on_press(Message::TogglePlaying)
            .style(ControlButton_)
            .width(Length::Units(48))
            .height(Length::Units(48))
    }

    pub fn view(&self) -> Column<Message> {
        let play_pause = self.play_button();
        Column::new().push(play_pause)
    }
}

impl Player {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Player {
            waveform: None,
            controls: Controls::new(),
            sender,
            receiver,
        }
    }

    pub fn view(&self) -> Container<Message> {
        let svg: Element<Message> = match &self.waveform {
            Some(wf) => {
                let canvas = Canvas::new(wf).width(Length::Fill).height(Length::Fill);
                canvas.into()
            }
            None => Space::new(Length::Fill, Length::Fill).into(),
        };
        let controls = Controls::view(&self.controls);
        let player = Column::new().push(svg).push(controls);
        Container::new(player)
            .width(Length::Fill)
            .height(Length::FillPortion(1))
            .style(PlayerContainer)
            .padding(1)
            .center_x()
            .center_y()
    }

    pub fn play_file(&mut self, file_path: PathBuf) {
        self.controls
            .is_playing
            .store(true, sync::atomic::Ordering::SeqCst);
        let audio_buffer: WaveForm = load_source(&file_path).into();
        self.waveform = Some(audio_buffer);
        let is_playing = sync::Arc::clone(&self.controls.is_playing);
        let (sender, receiver) = channel();
        self.sender = sender;
        self.resume();
        thread::spawn(move || {
            let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
            let sink = rodio::Sink::try_new(&stream_handle).unwrap();
            // now we have to re-figure out how to tell the parent thread
            // when the file is done playing. smh my head.
            // probably can't use a channel, probably need to use something
            // bidirectional like a socket
            for msg in receiver.iter() {
                match msg {
                    PlayerCommand::Play => {
                        is_playing.store(true, sync::atomic::Ordering::SeqCst);
                        if sink.empty() {
                            sink.append(load_source(&file_path));
                        }
                        sink.play();
                    },
                    PlayerCommand::Pause => {
                        is_playing.store(false, sync::atomic::Ordering::SeqCst);
                        sink.pause();
                    }
                }
            }
        });
    }

    pub fn pause(&mut self) {
        self.controls
            .is_playing
            .store(false, sync::atomic::Ordering::SeqCst);
        self.sender.send(PlayerCommand::Pause).unwrap();
    }

    pub fn resume(&mut self) {
        self.controls
            .is_playing
            .store(true, sync::atomic::Ordering::SeqCst);
        self.sender.send(PlayerCommand::Play).unwrap();
    }
}

pub fn load_source<T: std::convert::AsRef<std::path::Path>>(file_path: T) -> rodio::Decoder<BufReader<File>> {
    let file = File::open(file_path).unwrap();
    rodio::Decoder::new(BufReader::new(file)).unwrap()
}
