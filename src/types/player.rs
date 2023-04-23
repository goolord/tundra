use crate::source::callback::Callback;

pub use super::common::*;
pub use super::style::*;
pub use super::waveform::*;
use async_std::task;
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::TrySendError;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use futures::StreamExt;
use iced::pure::widget::canvas::*;
use iced::pure::widget::{Button, Column, Container, Row, Slider, Space, Svg};
use iced::pure::Element;
use iced::Length;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync;
use std::thread;

// todo: abstract this into a player type
// ref: https://github.com/tindleaj/miso/blob/master/src/player.rs
pub struct Player {
    pub waveform: Option<WaveForm>,
    pub controls: Controls,
    pub sender: UnboundedSender<PlayerCommand>,
}

pub enum PlayerCommand {
    Play,
    Pause,
    Stop,
}

#[derive(Debug, Clone)]
pub enum PlayerMsg {
    PlayingStored,
    SinkEmpty,
}

pub struct Controls {
    pub is_playing: sync::Arc<sync::atomic::AtomicBool>,
    pub volume: f32,
    pub seekbar: Option<Seekbar>,
}

pub struct Seekbar {
    pub total: u64,
    pub remaining: u64,
}

impl Seekbar {
    pub fn view(&self) -> Slider<f64, Message> {
        Slider::new(
            0.0..=100.0,
            self.remaining as f64 / self.total as f64,
            Message::Seek,
        )
    }
}

impl Controls {
    pub fn new() -> Self {
        Controls {
            is_playing: sync::Arc::new(false.into()),
            volume: f32::MAX,
            seekbar: None,
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
        Button::new(label)
            .on_press(Message::TogglePlaying)
            .style(ControlButton_)
            .width(Length::Units(50))
            .height(Length::Units(48))
    }

    pub fn stop_button(&self) -> Button<Message> {
        let label = Svg::from_path("./resources/stop.svg")
            .width(Length::Units(24))
            .height(Length::Units(24));
        Button::new(label)
            .on_press(Message::StopPlayback)
            .style(ControlButton_)
            .width(Length::Units(50))
            .height(Length::Units(48))
    }

    pub fn seek_bar(&self) -> Slider<f64, Message> {
        match &self.seekbar {
            None => Slider::new(0.0..=0.0, 0.0, Message::Seek),
            Some(seekbar) => seekbar.view(),
        }
        // Slider::new(0.., self.)
    }

    pub fn view(&self) -> Container<Message> {
        let c_row = Row::new()
            .push(self.play_button())
            .push(self.stop_button())
            .spacing(6)
            .padding(2);
        let column = Column::new()
            .push(self.seek_bar())
            .push(c_row)
            .width(Length::Fill)
            .align_items(iced::Alignment::Center);
        Container::new(column)
            .style(Controls_)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .width(Length::Fill)
    }
}

impl Player {
    pub fn new() -> Self {
        let (sender, _) = unbounded();
        Player {
            waveform: None,
            controls: Controls::new(),
            sender,
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

    pub fn play_file(&mut self, file_path: PathBuf) -> UnboundedReceiver<PlayerMsg> {
        self.controls
            .is_playing
            .store(true, sync::atomic::Ordering::SeqCst);
        let audio_buffer: WaveForm = load_source(&file_path).into();
        self.waveform = Some(audio_buffer);
        let is_playing = sync::Arc::clone(&self.controls.is_playing);
        let (sender, mut receiver) = unbounded();
        self.sender = sender;
        let (player_sender, player_receiver) = unbounded();
        thread::spawn(move || {
            let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
            let sink = rodio::Sink::try_new(&stream_handle).unwrap();
            let player_sender_clonable =
                 ClonableUnboundedSender(player_sender);
            let ps_a =
                 player_sender_clonable.clone().0;
            let ps_b =
                 player_sender_clonable.clone().0;
            let playing_stored = Box::new(move || {
                 ps_a
                    .unbounded_send(PlayerMsg::PlayingStored)
                    .unwrap_or(());
            });
            let sink_empty = Box::new(move || {
                 ps_b
                    .unbounded_send(PlayerMsg::SinkEmpty)
                    .unwrap_or(());
            });
            task::block_on(async move {
                loop {
                    if let Some(msg) = receiver.next().await {
                        match msg {
                            PlayerCommand::Play => {
                                is_playing.store(true, sync::atomic::Ordering::SeqCst);
                                playing_stored();
                                if sink.empty() {
                                    sink.append(load_source(&file_path));
                                    sink.append::<Callback<f32>>(Callback::new(sink_empty.clone()));
                                }
                                sink.play();
                            }
                            PlayerCommand::Pause => {
                                is_playing.store(false, sync::atomic::Ordering::SeqCst);
                                playing_stored();
                                sink.pause();
                            }
                            PlayerCommand::Stop => {
                                is_playing.store(false, sync::atomic::Ordering::SeqCst);
                                playing_stored();
                                // TODO: this is broken now, somehow.
                                // seems like rodio's fault
                                sink.stop();
                            }
                        }
                    } else {
                        break;
                    }
                }
            });
        });
        self.resume();
        player_receiver
    }

    pub fn pause(&mut self) {
        handle_player_command_err(self.sender.unbounded_send(PlayerCommand::Pause))
    }

    pub fn resume(&mut self) {
        handle_player_command_err(self.sender.unbounded_send(PlayerCommand::Play))
    }

    pub fn stop(&mut self) {
        handle_player_command_err(self.sender.unbounded_send(PlayerCommand::Stop))
    }
}

fn handle_player_command_err<T>(res: Result<(), TrySendError<T>>) {
    if res.is_err() {
        eprintln!("{:?}", res);
    };
}

pub fn load_source<T: std::convert::AsRef<std::path::Path>>(
    file_path: T,
) -> rodio::Decoder<BufReader<File>> {
    let file = File::open(file_path).unwrap();
    rodio::Decoder::new(BufReader::new(file)).unwrap()
}
