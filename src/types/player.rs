use crate::source::arc_samples::{ArcSamplesSource, PlaybackPosition};
use crate::source::callback::Callback;

pub use super::common::*;
pub use super::waveform::*;
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use futures::executor::block_on;
use futures::StreamExt;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::slider::{Handle, HandleShape, Rail, Status as SliderStatus, Style as SliderStyle};
use iced::widget::{
    button, container, mouse_area, row, text, Button, Canvas, Column, Container, Slider,
    Space, Svg,
};
use iced::widget::text::Wrapping;
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme, theme};
use iced_aw::ContextMenu;
use std::path::PathBuf;
use rodio::Source;
use std::fs::File;
use std::path::Path;
use std::sync;
use std::sync::atomic::Ordering;
use std::thread;

const MAX_AUDIO_BYTES: u64 = 100 * 1024 * 1024;
const SEEKBAR_HEIGHT: f32 = 22.0;
const TRANSPORT_BUTTON: f32 = 42.0;
const TRANSPORT_ICON: f32 = 18.0;
const VOLUME_SLIDER_WIDTH: f32 = 72.0;

pub fn clamp_volume(volume: f32) -> f32 {
    if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn accent_color(theme: &Theme) -> Color {
    theme.extended_palette().primary.base.color
}

fn zoom_button_style(theme: &Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    let accent = accent_color(theme);
    let mut style = ButtonStyle {
        text_color: palette.background.base.text.scale_alpha(0.82),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: palette.background.strong.color.scale_alpha(0.35),
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(palette.background.weak.color.scale_alpha(0.42).into());
        }
        ButtonStatus::Hovered => {
            style.text_color = palette.background.base.text;
            style.background = Some(accent.scale_alpha(0.18).into());
            style.border.color = accent.scale_alpha(0.35);
        }
        ButtonStatus::Pressed => {
            style.text_color = palette.background.base.text;
            style.background = Some(accent.scale_alpha(0.28).into());
        }
    }
    style
}

fn waveform_toolbar_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.scale_alpha(0.48).into()),
        border: Border {
            width: 1.0,
            color: palette.background.strong.color.scale_alpha(0.28),
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn zoom_label_badge(zoom: f32) -> Element<'static, Message> {
    container(
        text(format!("Zoom {zoom:.1}×"))
            .size(11)
            .font(iced::Font {
                weight: iced::font::Weight::Semibold,
                ..iced::Font::default()
            })
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(accent_color(theme).scale_alpha(0.92)),
            }),
    )
    .padding([4, 8])
    .style(|theme| {
        let accent = accent_color(theme);
        container::Style {
            background: Some(accent.scale_alpha(0.14).into()),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: accent.scale_alpha(0.24),
            },
            ..Default::default()
        }
    })
    .into()
}

fn zoom_button(label: &'static str, message: Message) -> Button<'static, Message> {
    button(text(label).size(15))
        .padding([2, 10])
        .on_press(message)
        .style(zoom_button_style)
}

fn controls_panel_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.scale_alpha(0.55).into()),
        border: Border {
            width: 1.0,
            color: palette.background.strong.color.scale_alpha(0.35),
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn transport_button_style(
    theme: &Theme,
    status: ButtonStatus,
    primary: bool,
    active: bool,
) -> ButtonStyle {
    let palette = theme.extended_palette();
    let accent = accent_color(theme);
    let mut style = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border {
            radius: (TRANSPORT_BUTTON / 2.0).into(),
            width: if primary && active { 0.0 } else { 1.0 },
            color: if primary {
                accent.scale_alpha(0.55)
            } else {
                palette.background.strong.color.scale_alpha(0.35)
            },
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if primary && active {
                    accent.into()
                } else {
                    palette.background.weak.color.scale_alpha(0.35).into()
                },
            );
        }
        ButtonStatus::Hovered => {
            style.background = Some(
                if primary {
                    if active {
                        accent.scale_alpha(0.92).into()
                    } else {
                        accent.scale_alpha(0.22).into()
                    }
                } else {
                    palette.background.strong.color.scale_alpha(0.28).into()
                },
            );
        }
        ButtonStatus::Pressed => {
            style.background = Some(
                if primary {
                    accent.scale_alpha(0.78).into()
                } else {
                    palette.background.strong.color.scale_alpha(0.42).into()
                },
            );
        }
    }

    style
}

fn transport_icon_color(primary: bool, active: bool, theme: &Theme) -> Color {
    if primary && active {
        Color::WHITE
    } else if primary {
        accent_color(theme)
    } else {
        theme
            .extended_palette()
            .background
            .base
            .text
            .scale_alpha(0.78)
    }
}

fn track_name_label(name: String) -> Element<'static, Message> {
    container(
        row![
            Svg::from_path(resource_path("music-solid.svg"))
                .width(Length::Fixed(14.0))
                .height(Length::Fixed(14.0))
                .style(|theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(accent_color(theme).scale_alpha(0.85)),
                }),
            text(name)
                .size(12)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(
                        theme
                            .extended_palette()
                            .background
                            .base
                            .text
                            .scale_alpha(0.72),
                    ),
                }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .into()
}

fn format_time(secs: f64) -> String {
    let total = secs.max(0.0).floor() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn seekbar_style(theme: &Theme, status: SliderStatus) -> SliderStyle {
    let palette = theme.extended_palette();
    let accent = accent_color(theme);
    let track = palette.background.strong.color.scale_alpha(0.42);
    let fill = match status {
        SliderStatus::Active => accent.scale_alpha(0.72),
        SliderStatus::Hovered => accent.scale_alpha(0.92),
        SliderStatus::Dragged => accent,
    };
    let handle_radius = match status {
        SliderStatus::Dragged => 9.0,
        SliderStatus::Hovered => 8.0,
        SliderStatus::Active => 7.0,
    };
    SliderStyle {
        rail: Rail {
            backgrounds: (fill.into(), track.into()),
            width: 5.0,
            border: Border {
                radius: 2.5.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
        },
        handle: Handle {
            shape: HandleShape::Circle { radius: handle_radius },
            background: palette.background.base.color.into(),
            border_width: 2.0,
            border_color: fill,
        },
    }
}

fn volume_slider_style(theme: &Theme, status: SliderStatus) -> SliderStyle {
    let palette = theme.extended_palette();
    let accent = accent_color(theme);
    let track = palette.background.strong.color.scale_alpha(0.42);
    let fill = match status {
        SliderStatus::Active => accent.scale_alpha(0.72),
        SliderStatus::Hovered => accent.scale_alpha(0.92),
        SliderStatus::Dragged => accent,
    };
    let handle_radius = match status {
        SliderStatus::Dragged => 6.0,
        SliderStatus::Hovered => 5.5,
        SliderStatus::Active => 5.0,
    };
    SliderStyle {
        rail: Rail {
            backgrounds: (fill.into(), track.into()),
            width: 3.0,
            border: Border {
                radius: 1.5.into(),
                width: 0.0,
                color: Color::TRANSPARENT,
            },
        },
        handle: Handle {
            shape: HandleShape::Circle { radius: handle_radius },
            background: palette.background.base.color.into(),
            border_width: 1.5,
            border_color: fill,
        },
    }
}

fn time_label(content: String, emphasized: bool, align: iced::alignment::Horizontal) -> Element<'static, Message> {
    container(
        text(content)
            .size(12)
            .font(iced::Font::MONOSPACE)
            .style(move |theme: &Theme| iced::widget::text::Style {
                color: Some(if emphasized {
                    theme.extended_palette().background.base.text.scale_alpha(0.88)
                } else {
                    theme.extended_palette().background.base.text.scale_alpha(0.52)
                }),
            }),
    )
    .width(Length::Fixed(44.0))
    .align_x(align)
    .into()
}

#[derive(Debug, Clone)]
pub struct PlayerWorker {
    cmd_sender: UnboundedSender<PlayerCommand>,
}

pub struct Player {
    pub waveform: Option<WaveForm>,
    pub current_file: Option<PathBuf>,
    pub controls: Controls,
    cmd_sender: Option<UnboundedSender<PlayerCommand>>,
    pending_commands: Vec<PlayerCommand>,
}

enum PlayerCommand {
    Load(PlaybackData, sync::Arc<PlaybackPosition>),
    Play,
    Pause,
    Stop,
    Seek(f64, bool),
    SetVolume(f32),
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
    pub track_duration: Option<f64>,
    pub scrubbing: bool,
    pub volume: f32,
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
    fn seek_row(
        progress: f64,
        current_label: String,
        total_label: String,
        scrubbing: bool,
        enabled: bool,
    ) -> Element<'static, Message> {
        if enabled {
            row![
                time_label(current_label, scrubbing, iced::alignment::Horizontal::Right),
                Slider::new(0.0..=1.0, progress, Message::Seek)
                    .step(0.001)
                    .height(SEEKBAR_HEIGHT)
                    .on_release(Message::SeekCommit)
                    .style(seekbar_style)
                    .width(Length::Fill),
                time_label(total_label, false, iced::alignment::Horizontal::Left),
            ]
        } else {
            row![
                time_label(current_label, scrubbing, iced::alignment::Horizontal::Right),
                container(
                    Space::new()
                        .width(Length::Fill)
                        .height(Length::Fixed(5.0)),
                )
                .height(SEEKBAR_HEIGHT)
                .center_y(Length::Fill)
                .width(Length::Fill)
                .style(|theme: &Theme| {
                    let track = theme
                        .extended_palette()
                        .background
                        .strong
                        .color
                        .scale_alpha(0.28);
                    container::Style {
                        background: Some(track.into()),
                        border: Border {
                            radius: 2.5.into(),
                            width: 0.0,
                            color: Color::TRANSPARENT,
                        },
                        ..Default::default()
                    }
                }),
                time_label(total_label, false, iced::alignment::Horizontal::Left),
            ]
        }
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    pub fn view(&self, progress: f64, duration: Option<f64>, scrubbing: bool) -> Element<'_, Message> {
        let current_secs = duration.map(|duration| progress.clamp(0.0, 1.0) * duration);
        let current_label = current_secs
            .map(format_time)
            .unwrap_or_else(|| "0:00".into());
        let total_label = duration
            .map(format_time)
            .unwrap_or_else(|| "0:00".into());
        Self::seek_row(progress, current_label, total_label, scrubbing, true)
    }
}

impl Controls {
    fn play_button(&self) -> Button<'_, Message> {
        let playing = self.is_playing.load(Ordering::SeqCst);
        let icon_path = if playing {
            resource_path("pause.svg")
        } else {
            resource_path("play.svg")
        };
        let is_playing = sync::Arc::clone(&self.is_playing);
        Button::new(
            Svg::from_path(icon_path)
                .width(Length::Fixed(TRANSPORT_ICON))
                .height(Length::Fixed(TRANSPORT_ICON))
                .style(move |theme: &Theme, _| {
                    let playing = is_playing.load(Ordering::SeqCst);
                    iced::widget::svg::Style {
                        color: Some(transport_icon_color(true, playing, theme)),
                    }
                }),
        )
        .on_press(Message::TogglePlaying)
        .width(Length::Fixed(TRANSPORT_BUTTON))
        .height(Length::Fixed(TRANSPORT_BUTTON))
        .style({
            let is_playing = sync::Arc::clone(&self.is_playing);
            move |theme: &Theme, status| {
                let playing = is_playing.load(Ordering::SeqCst);
                transport_button_style(theme, status, true, playing)
            }
        })
    }

    fn stop_button(&self) -> Button<'_, Message> {
        Button::new(
            Svg::from_path(resource_path("stop.svg"))
                .width(Length::Fixed(TRANSPORT_ICON))
                .height(Length::Fixed(TRANSPORT_ICON))
                .style(|theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(transport_icon_color(false, false, theme)),
                }),
        )
        .on_press(Message::StopPlayback)
        .width(Length::Fixed(TRANSPORT_BUTTON))
        .height(Length::Fixed(TRANSPORT_BUTTON))
        .style(|theme: &Theme, status| transport_button_style(theme, status, false, false))
    }

    fn transport_cluster(&self) -> Element<'_, Message> {
        container(
            row![self.play_button(), self.stop_button()]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding(4)
        .style(|theme: &theme::Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.base.color.scale_alpha(0.35).into()),
                border: Border {
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.3),
                    radius: 999.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }

    fn volume_control(&self) -> Element<'_, Message> {
        row![
            text("V")
                .size(11)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(
                        theme
                            .extended_palette()
                            .background
                            .base
                            .text
                            .scale_alpha(0.62),
                    ),
                }),
            Slider::new(0.0..=1.0_f32, self.volume, Message::VolumeChanged)
                .step(0.01_f32)
                .width(Length::Fixed(VOLUME_SLIDER_WIDTH))
                .height(16.0)
                .on_release(Message::VolumeCommit)
                .style(volume_slider_style),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
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
            None => Seekbar::seek_row(
                0.0,
                "--:--".into(),
                "--:--".into(),
                false,
                false,
            ),
            Some(seekbar) => seekbar.view(progress, self.track_duration, self.scrubbing),
        }
    }

    pub fn view(&self, track_name: Option<&str>) -> Element<'_, Message> {
        let transport = self.transport_cluster();
        let footer: Element<Message> = if let Some(name) = track_name {
            row![
                container(track_name_label(name.to_owned()))
                    .width(Length::FillPortion(2))
                    .height(Length::Shrink),
                self.volume_control(),
                transport,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
        } else {
            row![
                Space::new().width(Length::Fill),
                self.volume_control(),
                transport,
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
        };

        mouse_area(
            Container::new(
                Column::new()
                    .push(self.seek_bar())
                    .push(footer)
                    .spacing(8)
                    .width(Length::Fill),
            )
            .padding([6, 8])
            .width(Length::Fill)
            .height(Length::Shrink)
            .style(controls_panel_style),
        )
        .on_enter(Message::ControlsHoverChanged(true))
        .on_exit(Message::ControlsHoverChanged(false))
        .into()
    }
}

impl PlayerWorker {
    pub fn spawn(
        is_playing: sync::Arc<sync::atomic::AtomicBool>,
        volume: f32,
    ) -> (Self, UnboundedReceiver<PlayerMsg>) {
        let volume = clamp_volume(volume);
        let (cmd_sender, cmd_receiver) = unbounded();
        let (msg_sender, msg_receiver) = unbounded();
        thread::spawn(move || {
            run_audio_worker(cmd_receiver, msg_sender, is_playing, volume);
        });
        (Self { cmd_sender }, msg_receiver)
    }
}

impl Player {
    pub fn new(volume: f32) -> Self {
        let volume = clamp_volume(volume);
        Self {
            waveform: None,
            current_file: None,
            controls: Controls {
                is_playing: sync::Arc::new(sync::atomic::AtomicBool::new(false)),
                seekbar: None,
                playback_position: None,
                track_duration: None,
                scrubbing: false,
                volume,
            },
            cmd_sender: None,
            pending_commands: Vec::new(),
        }
    }

    pub fn attach_worker(&mut self, worker: PlayerWorker) {
        self.cmd_sender = Some(worker.cmd_sender);
        for command in std::mem::take(&mut self.pending_commands) {
            if let Some(sender) = self.cmd_sender.as_ref() {
                send_command(sender, command);
            }
        }
    }

    fn enqueue_command(&mut self, command: PlayerCommand) {
        if let Some(sender) = self.cmd_sender.as_ref() {
            send_command(sender, command);
        } else {
            self.pending_commands.push(command);
        }
    }

    pub fn view(&self) -> Container<'_, Message> {
        let mut column = Column::new()
            .width(Length::Fill)
            .height(Length::Fill);

        if let Some(wf) = &self.waveform {
            let zoom = wf.view_state().zoom;
            let toolbar = container(
                row![
                    zoom_label_badge(zoom),
                    zoom_button("−", Message::WaveformZoomOut),
                    zoom_button("+", Message::WaveformZoomIn),
                    Space::new().width(Length::Fill),
                    text("Click to seek. Scroll to zoom. Shift+scroll or drag to pan.")
                        .size(11)
                        .wrapping(Wrapping::None)
                        .style(|theme: &Theme| text::Style {
                            color: Some(
                                theme
                                    .extended_palette()
                                    .background
                                    .base
                                    .text
                                    .scale_alpha(0.62),
                            ),
                        }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .padding([6, 10]),
            )
            .width(Length::Fill)
            .style(waveform_toolbar_style);

            let underlay = Column::new()
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
                .spacing(4);

            let waveform_area = ContextMenu::new(underlay, || {
                file_context_menu(
                    Message::WaveformCopyName,
                    Message::WaveformCopyPath,
                    Message::WaveformRevealInFileManager,
                )
            })
            .style(context_menu_style);

            column = column.push(
                Container::new(waveform_area)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(2),
            );
        } else {
            column = column.push(Space::new().width(Length::Fill).height(Length::Fill));
        }

        let track_name = self
            .current_file
            .as_ref()
            .and_then(|path| crate::path_util::file_name_lossy(path));
        column = column.push(Controls::view(&self.controls, track_name.as_deref()));
        Container::new(column)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
    }

    pub fn play_file(&mut self, file_path: &Path) -> Result<(), String> {
        let Some(cmd_sender) = self.cmd_sender.as_ref() else {
            return Err("Audio is still starting. Try again in a moment.".into());
        };
        send_command(cmd_sender, PlayerCommand::Stop);
        let mut loaded = load_audio(file_path)?;

        let total_frames = loaded.playback.total_frames;
        let playback_position = PlaybackPosition::new(total_frames);
        loaded
            .waveform
            .set_playback_position(sync::Arc::clone(&playback_position));
        loaded.waveform.set_sample_rate(loaded.playback.sample_rate);
        self.controls.playback_position = Some(sync::Arc::clone(&playback_position));
        self.controls.track_duration = Some(total_frames as f64 / f64::from(loaded.playback.sample_rate));
        self.controls.scrubbing = false;
        self.controls.seekbar = Some(Seekbar { seeking: 0.0 });
        self.current_file = Some(file_path.to_path_buf());
        self.waveform = Some(loaded.waveform);

        send_command(
            cmd_sender,
            PlayerCommand::Load(loaded.playback, playback_position),
        );
        self.play();
        Ok(())
    }

    pub fn play(&mut self) {
        self.enqueue_command(PlayerCommand::Play);
    }

    pub fn pause(&mut self) {
        self.enqueue_command(PlayerCommand::Pause);
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
        self.enqueue_command(PlayerCommand::Stop);
    }

    pub fn seek(&mut self, p: f64) {
        self.controls.scrubbing = false;
        let resume = self.controls.is_playing.load(Ordering::SeqCst);
        if let Some(seekbar) = &mut self.controls.seekbar {
            seekbar.seeking = p;
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        self.enqueue_command(PlayerCommand::Seek(p, resume));
    }

    pub fn set_volume(&mut self, volume: f32) {
        let volume = clamp_volume(volume);
        self.controls.volume = volume;
        self.enqueue_command(PlayerCommand::SetVolume(volume));
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

    pub fn reset_on_error(&mut self) {
        self.enqueue_command(PlayerCommand::Stop);
        self.controls.is_playing.store(false, Ordering::SeqCst);
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        self.controls.scrubbing = false;
        self.controls.seekbar = None;
        self.controls.playback_position = None;
        self.controls.track_duration = None;
        self.current_file = None;
        self.waveform = None;
    }

    pub fn clear_waveform(&mut self) {
        self.waveform = None;
        self.current_file = None;
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
    initial_volume: f32,
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
    sink.set_volume(clamp_volume(initial_volume));
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
                    if let Some(position) = &playback_position {
                        play_offset = position.progress();
                    }
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
                PlayerCommand::Seek(p, resume) => {
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
                    if resume {
                        sink.play();
                    } else {
                        sink.pause();
                    }
                    is_playing.store(resume, Ordering::SeqCst);
                }
                PlayerCommand::SetVolume(next) => {
                    sink.set_volume(clamp_volume(next));
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
