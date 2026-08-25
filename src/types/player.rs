use crate::source::arc_samples::ArcSamplesSource;
use crate::source::callback::Callback;

pub use super::common::*;
pub use super::waveform::*;
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use futures::executor::block_on;
use futures::StreamExt;
use iced::widget::Button;
use iced::widget::Canvas;
use iced::widget::Column;
use iced::widget::Container;
use iced::widget::Row;
use iced::widget::Slider;
use iced::widget::Space;
use iced::widget::Svg;
use iced::widget::Text;
use iced::Element;
use iced::Length;
use rodio::Source;
use std::fs::File;
use std::path::Path;
use std::sync;
use std::sync::atomic::Ordering;
use std::thread;

const MAX_AUDIO_BYTES: u64 = 100 * 1024 * 1024;

pub struct Player {
    pub waveform: Option<WaveForm>,
    pub controls: Controls,
    cmd_sender: UnboundedSender<PlayerCommand>,
    pub error: Option<String>,
}

enum PlayerCommand {
    Load(PlaybackData),
    Play,
    Pause,
    Stop,
    Seek(f64),
}

#[derive(Debug, Clone, Copy)]
pub enum PlayerMsg {
    PlayingStored,
    SinkEmpty,
    StreamFailed,
}

pub struct Controls {
    pub is_playing: sync::Arc<sync::atomic::AtomicBool>,
    pub seekbar: Option<Seekbar>,
}

pub struct Seekbar {
    pub total: u64,
    pub remaining: u64,
    pub seeking: f64,
}

struct PlaybackData {
    samples: sync::Arc<Vec<f32>>,
    channels: u16,
    sample_rate: u32,
    total_frames: u64,
}

struct LoadedAudio {
    waveform: WaveForm,
    playback: PlaybackData,
}

impl Seekbar {
    pub fn view(&self) -> Element<'_, Message> {
        let progress = if self.total == 0 {
            0.0
        } else {
            1.0 - (self.remaining as f64 / self.total as f64)
        };
        Slider::new(0.0..=1.0, progress, Message::Seek)
            .step(0.01)
            .on_release(Message::SeekCommit)
            .into()
    }
}

impl Controls {
    pub fn seeking(&mut self, p: f64) {
        if let Some(seekbar) = &mut self.seekbar {
            seekbar.seeking = p;
        }
    }

    pub fn play_button(&self) -> Button<'_, Message> {
        let playing = self.is_playing.load(Ordering::SeqCst);
        let label = if playing {
            Svg::from_path(resource_path("pause.svg"))
        } else {
            Svg::from_path(resource_path("play.svg"))
        }
        .width(Length::Fixed(24.0))
        .height(Length::Fixed(24.0));
        Button::new(label)
            .on_press(Message::TogglePlaying)
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(48.0))
    }

    pub fn stop_button(&self) -> Button<'_, Message> {
        let label = Svg::from_path(resource_path("stop.svg"))
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0));
        Button::new(label)
            .on_press(Message::StopPlayback)
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(48.0))
    }

    pub fn seek_bar(&self) -> Element<'_, Message> {
        match &self.seekbar {
            None => Slider::new(0.0..=0.0, 0.0, Message::Seek).into(),
            Some(seekbar) => seekbar.view(),
        }
    }

    pub fn view(&self) -> Container<'_, Message> {
        let c_row = Row::new()
            .push(self.play_button())
            .push(self.stop_button())
            .spacing(6)
            .padding(2);
        Container::new(
            Column::new()
                .push(self.seek_bar())
                .push(c_row)
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
        )
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .width(Length::Fill)
    }
}

impl Player {
    pub fn new() -> (Self, UnboundedReceiver<PlayerMsg>) {
        let is_playing = sync::Arc::new(sync::atomic::AtomicBool::new(false));
        let worker_is_playing = is_playing.clone();
        let (cmd_sender, cmd_receiver) = unbounded();
        let (msg_sender, msg_receiver) = unbounded();
        thread::spawn(move || {
            run_audio_worker(cmd_receiver, msg_sender, worker_is_playing);
        });

        (
            Player {
                waveform: None,
                controls: Controls {
                    is_playing,
                    seekbar: None,
                },
                cmd_sender,
                error: None,
            },
            msg_receiver,
        )
    }

    pub fn view(&self) -> Container<'_, Message> {
        let svg: Element<Message> = match &self.waveform {
            Some(wf) => Canvas::new(wf)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => Space::new().width(Length::Fill).height(Length::Fill).into(),
        };

        let mut column = Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(
                Container::new(svg)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(2),
            );

        if let Some(error) = &self.error {
            column = column.push(
                Container::new(
                    Row::new()
                        .push(Text::new(error).size(14))
                        .push(
                            Button::new(Text::new("Dismiss"))
                                .on_press(Message::DismissError),
                        )
                        .spacing(8),
                )
                .padding(4)
                .width(Length::Fill),
            );
        }

        column = column.push(Controls::view(&self.controls));
        Container::new(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(1)
            .center_x(Length::Fill)
    }

    pub fn play_file(&mut self, file_path: &Path) -> Result<(), String> {
        self.stop();
        let loaded = load_audio(file_path)?;
        self.error = None;

        let total_frames = loaded.playback.total_frames;
        self.controls.seekbar = Some(Seekbar {
            total: total_frames,
            remaining: total_frames,
            seeking: 0.0,
        });
        self.waveform = Some(loaded.waveform);

        send_command(&self.cmd_sender, PlayerCommand::Load(loaded.playback));
        self.play();
        Ok(())
    }

    pub fn play(&mut self) {
        send_command(&self.cmd_sender, PlayerCommand::Play);
    }

    pub fn pause(&mut self) {
        send_command(&self.cmd_sender, PlayerCommand::Pause);
    }

    pub fn stop(&mut self) {
        send_command(&self.cmd_sender, PlayerCommand::Stop);
    }

    pub fn seek(&mut self, p: f64) {
        send_command(&self.cmd_sender, PlayerCommand::Seek(p));
    }

    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
        self.waveform = None;
        self.controls.seekbar = None;
        self.controls.is_playing.store(false, Ordering::SeqCst);
        self.stop();
    }
}

fn send_command(sender: &UnboundedSender<PlayerCommand>, command: PlayerCommand) {
    if let Err(err) = sender.unbounded_send(command) {
        eprintln!("Player command failed: {err:?}");
    }
}

fn run_audio_worker(
    mut cmd_receiver: UnboundedReceiver<PlayerCommand>,
    msg_sender: UnboundedSender<PlayerMsg>,
    is_playing: sync::Arc<sync::atomic::AtomicBool>,
) {
    let stream = match rodio::OutputStreamBuilder::open_default_stream() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("Audio output unavailable: {err}");
            is_playing.store(false, Ordering::SeqCst);
            let _ = msg_sender.unbounded_send(PlayerMsg::StreamFailed);
            return;
        }
    };

    let sink = rodio::Sink::connect_new(stream.mixer());
    let mut playback: Option<PlaybackData> = None;
    let mut play_offset = 0.0_f64;

    block_on(async move {
        while let Some(command) = cmd_receiver.next().await {
            match command {
                PlayerCommand::Load(data) => {
                    playback = Some(data);
                    play_offset = 0.0;
                    sink.clear();
                    is_playing.store(false, Ordering::SeqCst);
                }
                PlayerCommand::Play => {
                    let Some(data) = playback.as_ref() else {
                        continue;
                    };
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    if sink.empty() {
                        append_playback(&sink, data, play_offset, &msg_sender);
                    }
                    sink.play();
                    is_playing.store(true, Ordering::SeqCst);
                }
                PlayerCommand::Pause => {
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    sink.pause();
                    is_playing.store(false, Ordering::SeqCst);
                }
                PlayerCommand::Stop => {
                    play_offset = 0.0;
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    sink.clear();
                    is_playing.store(false, Ordering::SeqCst);
                }
                PlayerCommand::Seek(p) => {
                    let Some(data) = playback.as_ref() else {
                        continue;
                    };
                    play_offset = p.clamp(0.0, 1.0);
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    sink.clear();
                    append_playback(&sink, data, play_offset, &msg_sender);
                    sink.play();
                    is_playing.store(true, Ordering::SeqCst);
                }
            }
        }
    });
}

fn append_playback(
    sink: &rodio::Sink,
    data: &PlaybackData,
    offset: f64,
    msg_sender: &UnboundedSender<PlayerMsg>,
) {
    let channels = data.channels as usize;
    if channels == 0 || data.total_frames == 0 {
        return;
    }

    let skip_frames = (offset.clamp(0.0, 1.0) * data.total_frames as f64).round() as usize;
    let skip_samples = skip_frames.saturating_mul(channels).min(data.samples.len());
    if skip_samples >= data.samples.len() {
        return;
    }

    sink.append(ArcSamplesSource::new(
        sync::Arc::clone(&data.samples),
        data.channels,
        data.sample_rate,
        skip_samples,
    ));

    let sender = msg_sender.clone();
    sink.append(Callback::new(
        Box::new(move |msg| {
            sender.unbounded_send(msg).unwrap_or(());
        }),
        PlayerMsg::SinkEmpty,
        data.sample_rate,
    ));
}

fn load_audio(path: &Path) -> Result<LoadedAudio, String> {
    let file = File::open(path)
        .map_err(|err| format!("Cannot open {}: {err}", path.display()))?;
    let file_len = file
        .metadata()
        .map_err(|err| format!("Cannot read metadata for {}: {err}", path.display()))?
        .len();
    if file_len > MAX_AUDIO_BYTES {
        return Err(format!(
            "{} is too large ({} MB; limit is {} MB)",
            path.display(),
            file_len / (1024 * 1024),
            MAX_AUDIO_BYTES / (1024 * 1024)
        ));
    }

    let decoder = rodio::Decoder::try_from(file)
        .map_err(|err| format!("Cannot decode {}: {err}", path.display()))?;

    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    let interleaved: Vec<f32> = decoder.collect();

    if channels == 0 {
        return Err(format!("{} has no audio channels", path.display()));
    }

    let channels_usize = channels as usize;
    if !interleaved.len().is_multiple_of(channels_usize) {
        return Err(format!("{} has corrupt sample data", path.display()));
    }

    let total_frames = (interleaved.len() / channels_usize) as u64;
    let mut mono = Vec::with_capacity(interleaved.len() / channels_usize);
    for chunk in interleaved.chunks_exact(channels_usize) {
        mono.push(chunk.iter().sum::<f32>() / channels as f32);
    }

    Ok(LoadedAudio {
        waveform: WaveForm::new(mono),
        playback: PlaybackData {
            samples: sync::Arc::new(interleaved),
            channels,
            sample_rate,
            total_frames,
        },
    })
}
