use crate::source::arc_samples::PlaybackPosition;
use crate::source::callback::Callback;
use crate::source::streaming::{append_stream, probe_decoder};
use crate::waveform_peaks::{spawn_peak_build, WaveformPeaks};

pub use super::common::*;
pub use super::waveform::*;
use futures::channel::mpsc::unbounded;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::slider::{Handle, HandleShape, Rail, Status as SliderStatus, Style as SliderStyle};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{
    button, container, mouse_area, row, scrollable, text, Button, Canvas, Column, Container, Row,
    Slider, Space,
};
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme, theme};
use crate::metadata::TagField;
use iced_aw::ContextMenu;
use std::path::PathBuf;
use rodio::buffer::SamplesBuffer;
use std::path::Path;
use std::sync::{self, Arc, Mutex};
use std::sync::atomic::Ordering;
use std::thread;

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

fn toolbar_tag_chip(field: TagField, value: String) -> Element<'static, Message> {
    let accent = tag_field_color(field);
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
                resource_svg("music-solid.svg")
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
    msg_sender: UnboundedSender<PlayerMsg>,
}

pub struct Player {
    pub waveform: Option<WaveForm>,
    pub current_file: Option<PathBuf>,
    pub controls: Controls,
    cmd_sender: Option<UnboundedSender<PlayerCommand>>,
    msg_sender: Option<UnboundedSender<PlayerMsg>>,
    pending_commands: Vec<PlayerCommand>,
    /// Id of the loaded track; also read by its peak builder to stop early.
    track_id: sync::Arc<sync::atomic::AtomicU64>,
}

/// Track ids tie asynchronous events to the file that caused them, so a late
/// event from the previous file is ignored instead of acting on the new one.
enum PlayerCommand {
    Load(PlaybackData, sync::Arc<PlaybackPosition>, u64),
    Play,
    Pause,
    Stop,
    Seek(f64, bool),
    SetVolume(f32),
    /// Sent by the audio callback when a segment of track `id` runs out.
    Ended(u64),
}

#[derive(Debug, Clone)]
pub enum PlayerMsg {
    Ended(u64),
    Looped(u64),
    WaveformPeaksReady(u64),
    DeviceUnavailable,
    FileFailed(String),
}

pub struct Controls {
    pub is_playing: sync::Arc<sync::atomic::AtomicBool>,
    pub playback_progress: Option<PlaybackProgress>,
    pub playback_position: Option<sync::Arc<PlaybackPosition>>,
    pub track_duration: Option<f64>,
    pub scrubbing: bool,
    pub volume: f32,
    pub looping: sync::Arc<sync::atomic::AtomicBool>,
}

pub struct PlaybackProgress {
    pub progress: f64,
}

struct PlaybackData {
    path: PathBuf,
    sample_rate: u32,
    total_frames: u64,
}

struct LoadedAudio {
    waveform: WaveForm,
    playback: PlaybackData,
    peaks: Arc<Mutex<WaveformPeaks>>,
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
            .map(format_duration)
            .unwrap_or_else(|| "--:--".into());
        let total = self
            .track_duration
            .map(format_duration)
            .unwrap_or_else(|| "--:--".into());
        (current, total)
    }

    fn play_button(&self) -> Button<'_, Message> {
        let playing = self.is_playing.load(Ordering::SeqCst);
        let icon = if playing {
            resource_svg("pause.svg")
        } else {
            resource_svg("play.svg")
        };
        let is_playing = sync::Arc::clone(&self.is_playing);
        Button::new(
            icon
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
            resource_svg("stop.svg")
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
        let looping = self.looping.load(Ordering::Relaxed);
        Button::new(
            resource_svg("repeat.svg")
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
        looping: sync::Arc<sync::atomic::AtomicBool>,
        volume: f32,
    ) -> (Self, UnboundedReceiver<PlayerMsg>) {
        let volume = clamp_volume(volume);
        let (cmd_sender, cmd_receiver) = unbounded();
        let (msg_sender, msg_receiver) = unbounded();
        let worker_msg_sender = msg_sender.clone();
        let worker_cmd_sender = cmd_sender.clone();
        thread::spawn(move || {
            run_audio_worker(
                cmd_receiver,
                worker_cmd_sender,
                msg_sender,
                is_playing,
                looping,
                volume,
            );
        });
        (
            Self {
                cmd_sender,
                msg_sender: worker_msg_sender,
            },
            msg_receiver,
        )
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
                looping: sync::Arc::new(sync::atomic::AtomicBool::new(looping)),
            },
            cmd_sender: None,
            msg_sender: None,
            pending_commands: Vec::new(),
            track_id: Default::default(),
        }
    }

    pub fn attach_worker(&mut self, worker: PlayerWorker) {
        self.cmd_sender = Some(worker.cmd_sender);
        self.msg_sender = Some(worker.msg_sender);
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
                    Canvas::new(wf)
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .spacing(4);

            let waveform_area = ContextMenu::new(underlay, || {
                file_context_menu(
                    Message::WaveformCopyName,
                    Message::WaveformCopyPath,
                    Message::WaveformRevealInFileManager,
                    Some(Message::WaveformOpenAutoTag),
                    None,
                    Some(Message::WaveformEditTags),
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
            .set_playback(
                sync::Arc::clone(&playback_position),
                sync::Arc::clone(&self.controls.is_playing),
            );
        loaded.waveform.set_sample_rate(loaded.playback.sample_rate);
        self.controls.playback_position = Some(sync::Arc::clone(&playback_position));
        self.controls.track_duration = Some(total_frames as f64 / f64::from(loaded.playback.sample_rate));
        self.controls.playback_progress = Some(PlaybackProgress { progress: 0.0 });
        self.current_file = Some(file_path.to_path_buf());
        self.waveform = Some(loaded.waveform);

        let id = self.track_id.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(msg_sender) = self.msg_sender.clone() {
            let track_id = sync::Arc::clone(&self.track_id);
            spawn_peak_build(
                file_path.to_path_buf(),
                total_frames as usize,
                loaded.peaks.clone(),
                move || track_id.load(Ordering::SeqCst) != id,
                move || {
                    let _ = msg_sender.unbounded_send(PlayerMsg::WaveformPeaksReady(id));
                },
            );
        }

        send_command(
            cmd_sender,
            PlayerCommand::Load(loaded.playback, playback_position, id),
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

    pub fn on_waveform_peaks_ready(&mut self) {
        let Some(waveform) = &mut self.waveform else {
            return;
        };
        let Some(sample_count) = waveform.apply_peaks_ready() else {
            waveform.invalidate_cache();
            return;
        };
        let total_frames = sample_count as u64;
        if let Some(position) = &self.controls.playback_position {
            position.set_total_frames(total_frames);
        }
        if waveform.sample_rate() > 0 {
            self.controls.track_duration =
                Some(total_frames as f64 / f64::from(waveform.sample_rate()));
        }
    }

    /// Whether an event for track `id` still applies.
    pub fn is_current_track(&self, id: u64) -> bool {
        self.track_id.load(Ordering::SeqCst) == id
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

    pub fn toggle_loop(&mut self) {
        self.controls
            .looping
            .fetch_xor(true, Ordering::Relaxed);
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
        let p = p.clamp(0.0, 1.0);
        if let Some(state) = &mut self.controls.playback_progress {
            state.progress = p;
        }
        // Move the shared position now instead of waiting for the audio thread to drain the
        // command queue. The playhead reads this atomic directly, so leaving it stale would snap
        // the head back to the old spot for a frame or two before the seek lands. Only safe once
        // the audio thread exists; otherwise the command sits in `pending_commands` and the head
        // would advertise a frame playback never reaches.
        if self.audio_ready()
            && let Some(position) = &self.controls.playback_position
        {
            let total = position.total_frames();
            if total > 0 {
                position.set_frame((p * total as f64).round() as u64);
            }
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
        if self
            .controls
            .playback_progress
            .as_ref()
            .is_some_and(|state| state.progress == progress)
        {
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

/// The loaded track on the audio thread.
struct Track {
    data: PlaybackData,
    position: sync::Arc<PlaybackPosition>,
    id: u64,
}

/// Owns the output stream. Every start, seek, and loop builds a fresh `Sink`
/// and drops the previous one, which stops it without blocking: no queue to
/// drain, and an old segment's end-of-track callback never fires.
struct AudioWorker {
    stream: rodio::OutputStream,
    output: OutputFormat,
    volume: f32,
    sink: Option<rodio::Sink>,
    track: Option<Track>,
    /// Where to resume, as a fraction of the track.
    offset: f64,
    cmd_sender: UnboundedSender<PlayerCommand>,
    msg_sender: UnboundedSender<PlayerMsg>,
    is_playing: sync::Arc<sync::atomic::AtomicBool>,
    looping: sync::Arc<sync::atomic::AtomicBool>,
}

impl AudioWorker {
    fn send(&self, msg: PlayerMsg) {
        let _ = self.msg_sender.unbounded_send(msg);
    }

    fn set_playing(&self, playing: bool) {
        self.is_playing.store(playing, Ordering::SeqCst);
    }

    /// Start a new segment of the current track at `offset`.
    fn start(&mut self, offset: f64, play: bool) {
        self.sink = None;
        let Some(track) = &self.track else {
            self.set_playing(false);
            return;
        };
        self.offset = offset.clamp(0.0, 1.0);

        let sink = rodio::Sink::connect_new(self.stream.mixer());
        sink.set_volume(self.volume);
        if !play {
            sink.pause();
        }
        prime_output_queue(&sink, self.output);
        if let Err(err) = append_stream(
            &sink,
            &track.data.path,
            self.offset,
            track.position.total_frames(),
            Some(sync::Arc::clone(&track.position)),
            self.output.channels,
            self.output.sample_rate,
        ) {
            self.set_playing(false);
            self.send(PlayerMsg::FileFailed(err));
            return;
        }

        let (id, cmd_sender) = (track.id, self.cmd_sender.clone());
        sink.append(Callback::new(
            Box::new(move |()| {
                let _ = cmd_sender.unbounded_send(PlayerCommand::Ended(id));
            }),
            (),
            self.output.sample_rate,
        ));
        self.sink = Some(sink);
        self.set_playing(play);
    }

    fn handle(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Load(data, position, id) => {
                position.reset();
                self.track = Some(Track { data, position, id });
                self.start(0.0, false);
            }
            PlayerCommand::Play => {
                let Some(track) = &self.track else {
                    self.set_playing(false);
                    return;
                };
                let finished = self.sink.as_ref().is_none_or(rodio::Sink::empty)
                    || playback_exhausted(self.offset, track.position.total_frames());
                if finished {
                    let restart = playback_exhausted(track.position.progress(), track.position.total_frames());
                    let offset = if restart { 0.0 } else { self.offset };
                    self.start(offset, true);
                } else if let Some(sink) = &self.sink {
                    sink.play();
                    self.set_playing(true);
                }
            }
            PlayerCommand::Pause => {
                if let Some(track) = &self.track {
                    self.offset = track.position.progress();
                }
                if let Some(sink) = &self.sink {
                    sink.pause();
                }
                self.set_playing(false);
            }
            PlayerCommand::Stop => {
                self.sink = None;
                self.offset = 0.0;
                if let Some(track) = &self.track {
                    track.position.reset();
                }
                self.set_playing(false);
            }
            PlayerCommand::Seek(progress, resume) => {
                if let Some(track) = &self.track {
                    let total = track.position.total_frames();
                    track
                        .position
                        .set_frame((progress.clamp(0.0, 1.0) * total as f64).round() as u64);
                }
                self.start(progress, resume);
            }
            PlayerCommand::Ended(id) => {
                if self.track.as_ref().is_none_or(|track| track.id != id) {
                    return;
                }
                if self.looping.load(Ordering::Acquire) {
                    if let Some(track) = &self.track {
                        track.position.reset();
                    }
                    self.start(0.0, true);
                    self.send(PlayerMsg::Looped(id));
                } else {
                    self.sink = None;
                    self.set_playing(false);
                    self.send(PlayerMsg::Ended(id));
                }
            }
            PlayerCommand::SetVolume(volume) => {
                self.volume = clamp_volume(volume);
                if let Some(sink) = &self.sink {
                    sink.set_volume(self.volume);
                }
            }
        }
    }
}

fn run_audio_worker(
    cmd_receiver: UnboundedReceiver<PlayerCommand>,
    cmd_sender: UnboundedSender<PlayerCommand>,
    msg_sender: UnboundedSender<PlayerMsg>,
    is_playing: sync::Arc<sync::atomic::AtomicBool>,
    looping: sync::Arc<sync::atomic::AtomicBool>,
    initial_volume: f32,
) {
    let stream = match rodio::OutputStreamBuilder::open_default_stream() {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("Audio output unavailable: {err}");
            is_playing.store(false, Ordering::SeqCst);
            let _ = msg_sender.unbounded_send(PlayerMsg::DeviceUnavailable);
            return;
        }
    };
    let output = OutputFormat {
        channels: stream.config().channel_count(),
        sample_rate: stream.config().sample_rate(),
    };
    let mut worker = AudioWorker {
        stream,
        output,
        volume: clamp_volume(initial_volume),
        sink: None,
        track: None,
        offset: 0.0,
        cmd_sender,
        msg_sender,
        is_playing,
        looping,
    };
    for command in futures::executor::block_on_stream(cmd_receiver) {
        worker.handle(command);
    }
}

fn prime_output_queue(sink: &rodio::Sink, output: OutputFormat) {
    let channels = output.channels as usize;
    if channels == 0 || output.sample_rate == 0 {
        return;
    }
    // Tag the rodio queue at the device rate (its default filler is 44100 Hz).
    let silence = vec![0.0_f32; channels];
    sink.append(SamplesBuffer::new(output.channels, output.sample_rate, silence));
}

fn playback_exhausted(offset: f64, total_frames: u64) -> bool {
    if !offset.is_finite() || offset >= 1.0 {
        return true;
    }
    if total_frames == 0 {
        return false;
    }
    let skip_frames = (offset.clamp(0.0, 1.0) * total_frames as f64).round() as u64;
    skip_frames >= total_frames
}

fn load_audio(path: &Path) -> Result<LoadedAudio, String> {
    let info = probe_decoder(path)?;
    let sample_count = info.total_frames as usize;
    let peaks = Arc::new(Mutex::new(WaveformPeaks::empty()));
    let waveform = WaveForm::new_pending(sample_count, peaks.clone());
    Ok(LoadedAudio {
        waveform,
        playback: PlaybackData {
            path: path.to_path_buf(),
            sample_rate: info.sample_rate,
            total_frames: info.total_frames,
        },
        peaks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_from_start_when_offset_at_end() {
        assert!(playback_exhausted(1.0, 100));
        assert!(playback_exhausted(f64::NAN, 100));
        assert!(!playback_exhausted(0.0, 100));
        assert!(!playback_exhausted(0.5, 100));
    }

    #[test]
    fn play_from_start_when_skip_consumes_all_frames() {
        assert!(playback_exhausted(0.999, 100));
        assert!(!playback_exhausted(0.99, 100));
    }

    #[test]
    fn ended_position_reports_full_progress() {
        let position = PlaybackPosition::new(44100);
        position.set_frame(position.total_frames());
        assert_eq!(position.progress(), 1.0);
    }

    fn player_with_position(total_frames: u64) -> (Player, sync::Arc<PlaybackPosition>) {
        let mut player = Player::new(1.0, false);
        let position = PlaybackPosition::new(total_frames);
        player.controls.playback_position = Some(sync::Arc::clone(&position));
        player.controls.playback_progress = Some(PlaybackProgress { progress: 0.0 });
        (player, position)
    }

    #[test]
    fn seek_moves_the_shared_position_before_the_audio_thread_runs() {
        let (mut player, position) = player_with_position(1_000);
        // Nothing drains the receiver, so this stands in for the gap between releasing the
        // scrub and the audio thread handling `Seek`.
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        player.cmd_sender = Some(tx);
        position.set_frame(900);

        player.seek(0.25);

        assert!(
            (position.progress() - 0.25).abs() < 1e-9,
            "playhead should already read the seek target, got {}",
            position.progress()
        );
    }

    #[test]
    fn seek_leaves_the_position_alone_until_a_worker_is_attached() {
        let (mut player, position) = player_with_position(1_000);
        position.set_frame(900);

        player.seek(0.25);

        assert_eq!(
            position.progress(),
            0.9,
            "a queued seek must not advertise a frame playback never reached"
        );
    }

    #[test]
    fn toggle_loop_flips_flag() {
        let mut player = Player::new(1.0, false);
        assert!(!player.controls.looping.load(Ordering::Relaxed));
        player.toggle_loop();
        assert!(player.controls.looping.load(Ordering::Relaxed));
        player.toggle_loop();
        assert!(!player.controls.looping.load(Ordering::Relaxed));
    }
}
