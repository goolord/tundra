use crate::source::arc_samples::{ArcSamplesSource, PlaybackPosition};
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
use iced::widget::button;
use iced::widget::mouse_area;
use iced::widget::row;
use iced::widget::text;
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
    Load(PlaybackData, sync::Arc<PlaybackPosition>),
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
    pub playback_position: Option<sync::Arc<PlaybackPosition>>,
    pub scrubbing: bool,
}

pub struct Seekbar {
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
    pub fn view(&self, progress: f64) -> Element<'_, Message> {
        Slider::new(0.0..=1.0, progress, Message::Seek)
            .step(0.01)
            .on_release(Message::SeekCommit)
            .into()
    }
}

impl Controls {
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
        let progress = match &self.seekbar {
            None => 0.0,
            Some(seekbar) => {
                if self.scrubbing {
                    seekbar.seeking
                } else {
                    self.playback_position
                        .as_ref()
                        .map(|position| position.progress())
                        .unwrap_or(seekbar.seeking)
                }
            }
        };
        match &self.seekbar {
            None => Slider::new(0.0..=0.0, 0.0, Message::Seek).into(),
            Some(seekbar) => seekbar.view(progress),
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
                    playback_position: None,
                    scrubbing: false,
                },
                cmd_sender,
                error: None,
            },
            msg_receiver,
        )
    }

    pub fn view(&self) -> Container<'_, Message> {
        let waveform_area: Element<Message> = match &self.waveform {
            Some(wf) => {
                let zoom = wf.view_state().zoom;
                let toolbar = row![
                    text(format!("Zoom {zoom:.1}×")).size(12),
                    button(text("−").size(16))
                        .padding([2, 8])
                        .on_press(Message::WaveformZoomOut),
                    button(text("+").size(16))
                        .padding([2, 8])
                        .on_press(Message::WaveformZoomIn),
                    text("Scroll to zoom · Shift+scroll to pan · ← → when hovered")
                        .size(11)
                        .color(iced::Color::from_rgb(0.55, 0.58, 0.62)),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);

                Column::new()
                    .push(toolbar)
                    .push(
                        mouse_area(
                            Canvas::new(wf)
                                .width(Length::Fill)
                                .height(Length::Fill),
                        )
                        .on_enter(Message::WaveformHoverChanged(true))
                        .on_exit(Message::WaveformHoverChanged(false)),
                    )
                    .spacing(4)
                    .into()
            }
            None => Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        };

        let mut column = Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(
                Container::new(waveform_area)
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
        let mut loaded = load_audio(file_path)?;
        self.error = None;

        let total_frames = loaded.playback.total_frames;
        let playback_position = PlaybackPosition::new(total_frames);
        loaded
            .waveform
            .set_playback_position(sync::Arc::clone(&playback_position));
        self.controls.playback_position = Some(sync::Arc::clone(&playback_position));
        self.controls.scrubbing = false;
        self.controls.seekbar = Some(Seekbar { seeking: 0.0 });
        self.waveform = Some(loaded.waveform);

        send_command(
            &self.cmd_sender,
            PlayerCommand::Load(loaded.playback, playback_position),
        );
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
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        self.controls.scrubbing = false;
        if let Some(seekbar) = &mut self.controls.seekbar {
            seekbar.seeking = 0.0;
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        send_command(&self.cmd_sender, PlayerCommand::Stop);
    }

    pub fn seek(&mut self, p: f64) {
        self.controls.scrubbing = false;
        if let Some(seekbar) = &mut self.controls.seekbar {
            seekbar.seeking = p;
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        send_command(&self.cmd_sender, PlayerCommand::Seek(p));
    }

    pub fn begin_scrub(&mut self, p: f64) {
        self.controls.scrubbing = true;
        if let Some(seekbar) = &mut self.controls.seekbar {
            seekbar.seeking = p;
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(Some(p));
        }
    }

    pub fn sync_playback_ui(&mut self) -> bool {
        if self.controls.scrubbing {
            return false;
        }
        let Some(position) = self.controls.playback_position.as_ref() else {
            return false;
        };
        let progress = position.progress();
        let changed = self
            .controls
            .seekbar
            .as_ref()
            .is_some_and(|seekbar| (seekbar.seeking - progress).abs() > 0.000_1);
        if !changed {
            return false;
        }
        if let Some(seekbar) = &mut self.controls.seekbar {
            seekbar.seeking = progress;
        }
        true
    }

    pub fn set_error(&mut self, message: String) {
        send_command(&self.cmd_sender, PlayerCommand::Stop);
        self.controls.is_playing.store(false, Ordering::SeqCst);
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        self.controls.scrubbing = false;
        self.controls.seekbar = None;
        self.controls.playback_position = None;
        self.waveform = None;
        self.error = Some(message);
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
    let mut playback_position: Option<sync::Arc<PlaybackPosition>> = None;
    let mut play_offset = 0.0_f64;

    block_on(async move {
        while let Some(command) = cmd_receiver.next().await {
            match command {
                PlayerCommand::Load(data, position) => {
                    position.reset();
                    playback_position = Some(position);
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
                        append_playback(
                            &sink,
                            data,
                            play_offset,
                            playback_position.as_ref(),
                            &msg_sender,
                        );
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
                    if let Some(position) = &playback_position {
                        position.reset();
                    }
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    sink.clear();
                    is_playing.store(false, Ordering::SeqCst);
                }
                PlayerCommand::Seek(p) => {
                    let Some(data) = playback.as_ref() else {
                        continue;
                    };
                    play_offset = p.clamp(0.0, 1.0);
                    if let Some(position) = &playback_position {
                        let frame =
                            (play_offset * data.total_frames as f64).round() as u64;
                        position.set_frame(frame);
                    }
                    let _ = msg_sender.unbounded_send(PlayerMsg::PlayingStored);
                    sink.clear();
                    append_playback(
                        &sink,
                        data,
                        play_offset,
                        playback_position.as_ref(),
                        &msg_sender,
                    );
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
    position: Option<&sync::Arc<PlaybackPosition>>,
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

    if let Some(position) = position {
        position.set_frame(skip_frames as u64);
    }

    sink.append(ArcSamplesSource::new(
        sync::Arc::clone(&data.samples),
        data.channels,
        data.sample_rate,
        skip_samples,
        position.cloned(),
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
