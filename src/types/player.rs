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
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, container, mouse_area, row, scrollable, text, Button, Canvas, Column, Container, Row,
    Slider, Space, Svg,
};
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme, theme};
use crate::metadata::TagField;
use iced_aw::ContextMenu;
use std::path::PathBuf;
use rodio::buffer::SamplesBuffer;
use rodio::source::UniformSourceIterator;
use rodio::Source;
use std::fs::File;
use std::path::Path;
use std::sync;
use std::sync::atomic::Ordering;
use std::thread;

const MAX_AUDIO_BYTES: u64 = 100 * 1024 * 1024;
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

fn tag_chip_accent(field: TagField) -> Color {
    match field {
        TagField::Instrument => Color::from_rgb8(0x52, 0xa8, 0x86),
        TagField::Bpm => Color::from_rgb8(0xc8, 0x72, 0x48),
        TagField::Key => Color::from_rgb8(0x9a, 0x68, 0xc0),
        TagField::Genre => Color::from_rgb8(0x48, 0x96, 0xc8),
        _ => Color::from_rgb8(0x50, 0x7a, 0xe0),
    }
}

fn toolbar_tag_chip(field: TagField, value: String) -> Element<'static, Message> {
    let accent = tag_chip_accent(field);
    container(
        row![
            text(field.label())
                .size(9)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(move |_theme: &Theme| iced::widget::text::Style {
                    color: Some(accent.scale_alpha(0.88)),
                }),
            text(value)
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Medium,
                    ..iced::Font::default()
                })
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(theme.extended_palette().background.base.text.scale_alpha(0.92)),
                }),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([3, 8])
    .style(move |theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(accent.scale_alpha(0.12).into()),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: accent.scale_alpha(0.28),
            },
            text_color: Some(palette.background.base.text),
            ..Default::default()
        }
    })
    .into()
}

const TOOLBAR_TAG_STRIP_MAX: f32 = 340.0;

fn toolbar_tags(tags: Vec<(TagField, String)>) -> Element<'static, Message> {
    let chips: Vec<Element<Message>> = tags
        .into_iter()
        .map(|(field, value)| toolbar_tag_chip(field, value))
        .collect();
    let row = Row::with_children(chips)
        .spacing(6)
        .align_y(Alignment::Center)
        .padding([0, 2]);
    container(
        scrollable(row)
            .direction(Direction::Horizontal(Scrollbar::new().width(3).scroller_width(3)))
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .max_width(TOOLBAR_TAG_STRIP_MAX)
    .align_x(iced::alignment::Horizontal::Right)
    .into()
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

fn track_info_row(
    name: String,
    path: PathBuf,
    current_label: String,
    total_label: String,
) -> Element<'static, Message> {
    let muted = |theme: &Theme| {
        theme
            .extended_palette()
            .background
            .base
            .text
            .scale_alpha(0.42)
    };
    mouse_area(
        container(
            row![
                Svg::from_path(resource_path("music-solid.svg"))
                    .width(Length::Fixed(14.0))
                    .height(Length::Fixed(14.0))
                    .style(|theme: &Theme, _| iced::widget::svg::Style {
                        color: Some(accent_color(theme).scale_alpha(0.85)),
                    }),
                container(
                    text(name).size(12).style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(
                            theme
                                .extended_palette()
                                .background
                                .base
                                .text
                                .scale_alpha(0.72),
                        ),
                    }),
                )
                .width(Length::Fill)
                .clip(true),
                text("·")
                    .size(11)
                    .style(move |theme: &Theme| iced::widget::text::Style {
                        color: Some(muted(theme)),
                    }),
                text(current_label)
                    .size(12)
                    .font(iced::Font::MONOSPACE)
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
                text("/")
                    .size(11)
                    .style(move |theme: &Theme| iced::widget::text::Style {
                        color: Some(muted(theme)),
                    }),
                text(total_label)
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .style(|theme: &Theme| iced::widget::text::Style {
                        color: Some(
                            theme
                                .extended_palette()
                                .background
                                .base
                                .text
                                .scale_alpha(0.52),
                        ),
                    }),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ),
    )
    .on_press(Message::FileDragPress {
        path,
        from_file_list: false,
    })
    .interaction(iced::mouse::Interaction::Grab)
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
    SinkEmpty,
    StreamFailed,
}

pub struct Controls {
    pub is_playing: sync::Arc<sync::atomic::AtomicBool>,
    pub playback_progress: Option<PlaybackProgress>,
    pub playback_position: Option<sync::Arc<PlaybackPosition>>,
    pub track_duration: Option<f64>,
    pub scrubbing: bool,
    pub volume: f32,
    pub looping: bool,
}

pub struct PlaybackProgress {
    pub progress: f64,
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

impl Controls {
    fn playback_ratio(&self) -> f64 {
        match &self.playback_progress {
            None => 0.0,
            Some(state) => {
                self.playback_position
                    .as_ref()
                    .map(|position| position.progress())
                    .unwrap_or(state.progress)
            }
        }
    }

    fn time_labels(&self) -> (String, String) {
        let progress = self.playback_ratio();
        let current_secs = self
            .track_duration
            .map(|duration| progress.clamp(0.0, 1.0) * duration);
        let current = current_secs
            .map(format_time)
            .unwrap_or_else(|| "--:--".into());
        let total = self
            .track_duration
            .map(format_time)
            .unwrap_or_else(|| "--:--".into());
        (current, total)
    }

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

    fn loop_button(&self) -> Button<'_, Message> {
        let looping = self.looping;
        Button::new(
            Svg::from_path(resource_path("repeat.svg"))
                .width(Length::Fixed(TRANSPORT_ICON))
                .height(Length::Fixed(TRANSPORT_ICON))
                .style(move |theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(transport_icon_color(looping, looping, theme)),
                }),
        )
        .on_press(Message::ToggleLoop)
        .width(Length::Fixed(TRANSPORT_BUTTON))
        .height(Length::Fixed(TRANSPORT_BUTTON))
        .style(move |theme: &Theme, status| transport_button_style(theme, status, looping, looping))
    }

    fn transport_cluster(&self) -> Element<'_, Message> {
        container(
            row![self.play_button(), self.stop_button(), self.loop_button()]
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

    pub fn view(&self, track_name: Option<&str>, track_path: Option<&Path>) -> Element<'_, Message> {
        let transport = self.transport_cluster();
        let (current_label, total_label) = self.time_labels();
        let footer: Element<Message> = if let (Some(name), Some(path)) = (track_name, track_path) {
            row![
                container(track_info_row(
                    name.to_owned(),
                    path.to_path_buf(),
                    current_label,
                    total_label,
                ))
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
            Container::new(footer)
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
    pub fn new(volume: f32, looping: bool) -> Self {
        let volume = clamp_volume(volume);
        Self {
            waveform: None,
            current_file: None,
            controls: Controls {
                is_playing: sync::Arc::new(sync::atomic::AtomicBool::new(false)),
                playback_progress: None,
                playback_position: None,
                track_duration: None,
                scrubbing: false,
                volume,
                looping,
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

    pub fn audio_ready(&self) -> bool {
        self.cmd_sender.is_some()
    }

    fn enqueue_command(&mut self, command: PlayerCommand) {
        if let Some(sender) = self.cmd_sender.as_ref() {
            send_command(sender, command);
        } else {
            self.pending_commands.push(command);
        }
    }

    pub fn view(&self, tags: Vec<(TagField, String)>) -> Container<'_, Message> {
        let mut column = Column::new()
            .width(Length::Fill)
            .height(Length::Fill);

        if let Some(wf) = &self.waveform {
            wf.set_ui_scrubbing(self.controls.scrubbing);
            let zoom = wf.view_state().zoom;
            let mut bar = row![
                zoom_label_badge(zoom),
                zoom_button("−", Message::WaveformZoomOut),
                zoom_button("+", Message::WaveformZoomIn),
                zoom_button("?", Message::WaveformHelp),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .padding([6, 10]);
            bar = bar.push(Space::new().width(Length::Fill));
            if !tags.is_empty() {
                bar = bar.push(toolbar_tags(tags));
            }
            let toolbar = container(bar)
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
                    Some(Message::WaveformOpenAutoTag),
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
        column = column.push(Controls::view(
            &self.controls,
            track_name.as_deref(),
            self.current_file.as_deref(),
        ));
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
        self.controls.playback_progress = Some(PlaybackProgress { progress: 0.0 });
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
        self.controls
            .is_playing
            .store(true, Ordering::Release);
        self.enqueue_command(PlayerCommand::Play);
    }

    pub fn pause(&mut self) {
        self.controls
            .is_playing
            .store(false, Ordering::Release);
        self.enqueue_command(PlayerCommand::Pause);
    }

    pub fn on_ended(&mut self) {
        if let Some(position) = &self.controls.playback_position {
            position.set_frame(position.total_frames());
        }
        if let Some(state) = &mut self.controls.playback_progress {
            state.progress = 1.0;
        }
        self.pause();
    }

    pub fn restart_from_start(&mut self) {
        self.seek(0.0);
    }

    pub fn toggle_loop(&mut self) {
        self.controls.looping = !self.controls.looping;
    }

    pub fn stop(&mut self) {
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        if let Some(state) = &mut self.controls.playback_progress {
            state.progress = 0.0;
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        self.enqueue_command(PlayerCommand::Stop);
    }

    pub fn seek(&mut self, p: f64) {
        let resume = self.controls.is_playing.load(Ordering::SeqCst);
        if let Some(state) = &mut self.controls.playback_progress {
            state.progress = p;
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
            .playback_progress
            .as_ref()
            .is_some_and(|state| (state.progress - progress).abs() > 0.000_1);
        if !changed {
            return false;
        }
        if let Some(state) = &mut self.controls.playback_progress {
            state.progress = progress;
        }
        true
    }

    pub fn reset_on_error(&mut self) {
        self.enqueue_command(PlayerCommand::Stop);
        self.controls.is_playing.store(false, Ordering::SeqCst);
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        self.controls.playback_progress = None;
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

#[derive(Clone, Copy)]
struct OutputFormat {
    channels: u16,
    sample_rate: u32,
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
    let output = OutputFormat {
        channels: stream.config().channel_count(),
        sample_rate: stream.config().sample_rate(),
    };
    let mut playback: Option<PlaybackData> = None;
    let mut playback_position: Option<sync::Arc<PlaybackPosition>> = None;
    let mut play_offset = 0.0_f64;
    let mut playback_revision: u64 = 0;
    let mut sink_revision: u64 = 0;

    block_on(async move {
        while let Some(command) = cmd_receiver.next().await {
            match command {
                PlayerCommand::Load(data, position) => {
                    position.reset();
                    playback_position = Some(position);
                    playback = Some(data);
                    play_offset = 0.0;
                    playback_revision = playback_revision.wrapping_add(1);
                    sink.clear();
                    prime_output_queue(&sink, output);
                    if let Some(data) = playback.as_ref() {
                        append_playback(
                            &sink,
                            data,
                            play_offset,
                            playback_position.as_ref(),
                            &msg_sender,
                            output,
                        );
                        sink_revision = playback_revision;
                    }
                    is_playing.store(false, Ordering::SeqCst);
                }
                PlayerCommand::Play => {
                    let Some(data) = playback.as_ref() else {
                        is_playing.store(false, Ordering::Release);
                        continue;
                    };
                    let exhausted = playback_exhausted(
                        play_offset,
                        data.total_frames,
                        data.channels as usize,
                        data.samples.len(),
                    );
                    let stale = sink_revision != playback_revision;
                    if stale || should_reappend_on_play(sink.empty(), exhausted) {
                        if exhausted {
                            play_offset = 0.0;
                            if let Some(position) = &playback_position {
                                position.reset();
                            }
                        }
                        if !sink.empty() {
                            sink.clear();
                        }
                        prime_output_queue(&sink, output);
                        append_playback(
                            &sink,
                            data,
                            play_offset,
                            playback_position.as_ref(),
                            &msg_sender,
                            output,
                        );
                        sink_revision = playback_revision;
                    }
                    sink.play();
                    is_playing.store(true, Ordering::Release);
                }
                PlayerCommand::Pause => {
                    if let Some(position) = &playback_position {
                        play_offset = position.progress();
                    }
                    sink.pause();
                    is_playing.store(false, Ordering::Release);
                }
                PlayerCommand::Stop => {
                    play_offset = 0.0;
                    if let Some(position) = &playback_position {
                        position.reset();
                    }
                    sink.clear();
                    is_playing.store(false, Ordering::Release);
                }
                PlayerCommand::Seek(p, resume) => {
                    let Some(data) = playback.as_ref() else {
                        is_playing.store(false, Ordering::Release);
                        continue;
                    };
                    play_offset = p.clamp(0.0, 1.0);
                    if let Some(position) = &playback_position {
                        let frame =
                            (play_offset * data.total_frames as f64).round() as u64;
                        position.set_frame(frame);
                    }
                    sink.clear();
                    prime_output_queue(&sink, output);
                    append_playback(
                        &sink,
                        data,
                        play_offset,
                        playback_position.as_ref(),
                        &msg_sender,
                        output,
                    );
                    if resume {
                        sink.play();
                    } else {
                        sink.pause();
                    }
                    sink_revision = playback_revision;
                    is_playing.store(resume, Ordering::Release);
                }
                PlayerCommand::SetVolume(next) => {
                    sink.set_volume(clamp_volume(next));
                }
            }
        }
    });
}

fn should_reappend_on_play(sink_empty: bool, exhausted: bool) -> bool {
    sink_empty || exhausted
}

fn prime_output_queue(sink: &rodio::Sink, output: OutputFormat) {
    let channels = output.channels as usize;
    if channels == 0 || output.sample_rate == 0 {
        return;
    }
    // Tag the rodio queue at the device rate after clear (default filler is 44100 Hz).
    let silence = vec![0.0_f32; channels];
    sink.append(SamplesBuffer::new(output.channels, output.sample_rate, silence));
}

fn prime_silence_frame_len(channels: u16) -> usize {
    channels as usize
}

fn playback_exhausted(offset: f64, total_frames: u64, channels: usize, sample_len: usize) -> bool {
    if !offset.is_finite() || offset >= 1.0 {
        return true;
    }
    if channels == 0 || total_frames == 0 || sample_len == 0 {
        return true;
    }
    let skip_frames = (offset.clamp(0.0, 1.0) * total_frames as f64).round() as usize;
    skip_frames.saturating_mul(channels) >= sample_len
}

fn append_playback(
    sink: &rodio::Sink,
    data: &PlaybackData,
    offset: f64,
    position: Option<&sync::Arc<PlaybackPosition>>,
    msg_sender: &UnboundedSender<PlayerMsg>,
    output: OutputFormat,
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

    let source = ArcSamplesSource::new(
        sync::Arc::clone(&data.samples),
        data.channels,
        data.sample_rate,
        skip_samples,
        position.cloned(),
    );
    // Resample to the device rate before queuing. Rodio's sink queue can report a
    // stale sample rate briefly when switching sources; normalizing here avoids
    // wrong pitch when browsing between files at different native rates.
    sink.append(UniformSourceIterator::new(
        source,
        output.channels,
        output.sample_rate,
    ));

    let sender = msg_sender.clone();
    sink.append(Callback::new(
        Box::new(move |msg| {
            sender.unbounded_send(msg).unwrap_or(());
        }),
        PlayerMsg::SinkEmpty,
        output.sample_rate,
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
    if sample_rate == 0 {
        return Err(format!("{} has an invalid sample rate", path.display()));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prime_silence_matches_output_channels() {
        assert_eq!(prime_silence_frame_len(2), 2);
        assert_eq!(prime_silence_frame_len(1), 1);
    }

    #[test]
    fn play_reappend_when_sink_empty_or_exhausted() {
        assert!(should_reappend_on_play(true, false));
        assert!(should_reappend_on_play(false, true));
        assert!(should_reappend_on_play(true, true));
        assert!(!should_reappend_on_play(false, false));
    }

    #[test]
    fn play_from_start_when_offset_at_end() {
        assert!(playback_exhausted(1.0, 100, 2, 200));
        assert!(playback_exhausted(f64::NAN, 100, 2, 200));
        assert!(!playback_exhausted(0.0, 100, 2, 200));
        assert!(!playback_exhausted(0.5, 100, 2, 200));
    }

    #[test]
    fn play_from_start_when_skip_consumes_all_samples() {
        assert!(playback_exhausted(0.999, 100, 2, 200));
        assert!(!playback_exhausted(0.99, 100, 2, 200));
    }

    #[test]
    fn ended_position_reports_full_progress() {
        let position = PlaybackPosition::new(44100);
        position.set_frame(position.total_frames());
        assert_eq!(position.progress(), 1.0);
    }

    #[test]
    fn toggle_loop_flips_flag() {
        let mut player = Player::new(1.0, false);
        assert!(!player.controls.looping);
        player.toggle_loop();
        assert!(player.controls.looping);
        player.toggle_loop();
        assert!(!player.controls.looping);
    }
}
