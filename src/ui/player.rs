//! The right-hand pane: waveform toolbar, waveform, and transport controls,
//! plus the UI-side state of whatever is playing.

use super::message::{Message, WaveformMsg};
use super::style;
use super::waveform::WaveForm;
use super::widgets::{FileMenuExtras, context_menu_style, file_context_menu, icon, spacer};
use crate::metadata::TagField;
use crate::playback::{PlaybackPosition, PlayerCommand, PlayerWorker, clamp_volume, probe_decoder};
use crate::waveform_peaks::{WaveformPeaks, spawn_peak_build};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::slider::{self, Handle, HandleShape, Rail};
use iced::widget::{Button, Canvas, Row, Slider, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Color, Element, Length, Theme};
use iced_aw::ContextMenu;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const TRANSPORT_BUTTON: f32 = 42.0;
const TRANSPORT_ICON: f32 = 18.0;
const VOLUME_SLIDER_WIDTH: f32 = 72.0;
const TOOLBAR_TAG_STRIP_MAX: f32 = 340.0;

pub struct Player {
    pub waveform: Option<WaveForm>,
    pub current_file: Option<PathBuf>,
    pub controls: Controls,
    worker: Option<PlayerWorker>,
    /// Commands issued before the audio thread was ready.
    pending_commands: Vec<PlayerCommand>,
    /// Id of the loaded track; also read by its peak builder to stop early.
    track_id: Arc<AtomicU64>,
}

pub struct Controls {
    pub is_playing: Arc<AtomicBool>,
    pub looping: Arc<AtomicBool>,
    /// Last known playhead, 0 to 1; `None` when nothing is loaded.
    pub playback_progress: Option<f64>,
    pub playback_position: Option<Arc<PlaybackPosition>>,
    pub track_duration: Option<f64>,
    pub scrubbing: bool,
    pub volume: f32,
}

impl Player {
    pub fn new(volume: f32, looping: bool) -> Self {
        Self {
            waveform: None,
            current_file: None,
            controls: Controls {
                is_playing: Arc::new(AtomicBool::new(false)),
                looping: Arc::new(AtomicBool::new(looping)),
                playback_progress: None,
                playback_position: None,
                track_duration: None,
                scrubbing: false,
                volume: clamp_volume(volume),
            },
            worker: None,
            pending_commands: Vec::new(),
            track_id: Default::default(),
        }
    }

    pub fn attach_worker(&mut self, worker: PlayerWorker) {
        for command in self.pending_commands.drain(..) {
            worker.send(command);
        }
        self.worker = Some(worker);
    }

    pub fn audio_ready(&self) -> bool {
        self.worker.is_some()
    }

    fn send(&mut self, command: PlayerCommand) {
        match &self.worker {
            Some(worker) => worker.send(command),
            None => self.pending_commands.push(command),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.controls.is_playing.load(Ordering::Acquire)
    }

    pub fn play_file(&mut self, file_path: &Path) -> Result<(), String> {
        let Some(worker) = self.worker.clone() else {
            return Err("Audio is still starting. Try again in a moment.".into());
        };
        worker.send(PlayerCommand::Stop);
        let info = probe_decoder(file_path)?;
        let peaks = Arc::new(Mutex::new(WaveformPeaks::empty()));
        let mut waveform = WaveForm::new_pending(info.total_frames as usize, Arc::clone(&peaks));

        let position = PlaybackPosition::new(info.total_frames);
        waveform.set_playback(Arc::clone(&position), Arc::clone(&self.controls.is_playing));
        waveform.set_sample_rate(info.sample_rate);
        self.controls.playback_position = Some(Arc::clone(&position));
        self.controls.track_duration = Some(info.total_frames as f64 / f64::from(info.sample_rate));
        self.controls.playback_progress = Some(0.0);
        self.current_file = Some(file_path.to_path_buf());
        self.waveform = Some(waveform);

        let id = self.track_id.fetch_add(1, Ordering::SeqCst) + 1;
        let track_id = Arc::clone(&self.track_id);
        let events = worker.clone();
        spawn_peak_build(
            file_path.to_path_buf(),
            info.total_frames as usize,
            peaks,
            move || track_id.load(Ordering::SeqCst) != id,
            move || events.emit(crate::playback::PlayerEvent::WaveformPeaksReady(id)),
        );

        worker.send(PlayerCommand::Load(file_path.to_path_buf(), position, id));
        self.play();
        Ok(())
    }

    pub fn play(&mut self) {
        self.controls.is_playing.store(true, Ordering::Release);
        self.send(PlayerCommand::Play);
    }

    pub fn pause(&mut self) {
        self.controls.is_playing.store(false, Ordering::Release);
        self.send(PlayerCommand::Pause);
    }

    pub fn toggle_playing(&mut self) {
        if self.is_playing() { self.pause() } else { self.play() }
    }

    /// Adopts the exact length once the peak builder has decoded the whole file.
    pub fn on_waveform_peaks_ready(&mut self) {
        let Some(waveform) = &mut self.waveform else {
            return;
        };
        let Some(sample_count) = waveform.apply_peaks_ready() else {
            waveform.invalidate_cache();
            return;
        };
        if let Some(position) = &self.controls.playback_position {
            position.set_total_frames(sample_count as u64);
        }
        if waveform.sample_rate() > 0 {
            self.controls.track_duration = Some(sample_count as f64 / f64::from(waveform.sample_rate()));
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
        self.set_progress(1.0);
        self.pause();
    }

    /// Updates the shown progress, if a track is loaded.
    pub fn set_progress(&mut self, progress: f64) {
        if let Some(shown) = &mut self.controls.playback_progress {
            *shown = progress;
        }
    }

    pub fn toggle_loop(&mut self) -> bool {
        !self.controls.looping.fetch_xor(true, Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        if let Some(position) = &self.controls.playback_position {
            position.reset();
        }
        self.set_progress(0.0);
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        self.send(PlayerCommand::Stop);
    }

    pub fn seek(&mut self, progress: f64) {
        let resume = self.is_playing();
        let progress = progress.clamp(0.0, 1.0);
        self.set_progress(progress);
        // Move the shared position now instead of waiting for the audio thread to drain the
        // command queue. The playhead reads this atomic directly, so leaving it stale would snap
        // the head back to the old spot for a frame or two before the seek lands. Only safe once
        // the audio thread exists; otherwise the command sits in `pending_commands` and the head
        // would advertise a frame playback never reaches.
        if self.audio_ready()
            && let Some(position) = &self.controls.playback_position
            && position.total_frames() > 0
        {
            position.seek_to(progress);
        }
        if let Some(waveform) = &mut self.waveform {
            waveform.set_scrub_progress(None);
        }
        self.send(PlayerCommand::Seek(progress, resume));
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.controls.volume = clamp_volume(volume);
        self.send(PlayerCommand::SetVolume(self.controls.volume));
    }

    /// Copies the audio thread's playhead into the time label.
    pub fn sync_playback_ui(&mut self) {
        if self.controls.scrubbing {
            return;
        }
        if let Some(progress) = self
            .controls
            .playback_position
            .as_ref()
            .map(|position| position.progress())
        {
            self.set_progress(progress);
        }
    }

    pub fn reset_on_error(&mut self) {
        self.send(PlayerCommand::Stop);
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

    pub fn view(&self, tags: Vec<(TagField, String)>) -> Element<'_, Message> {
        let waveform_area: Element<'_, Message> = match &self.waveform {
            Some(waveform) => {
                waveform.set_ui_scrubbing(self.controls.scrubbing);
                let underlay = column![
                    waveform_toolbar(waveform.view_state().zoom, tags),
                    Canvas::new(waveform).width(Length::Fill).height(Length::Fill),
                ]
                .spacing(4);
                let menu = ContextMenu::new(underlay, || {
                    file_context_menu(
                        WaveformMsg::CopyName.into(),
                        WaveformMsg::CopyPath.into(),
                        WaveformMsg::RevealInFileManager.into(),
                        FileMenuExtras {
                            auto_tag: Some(WaveformMsg::OpenAutoTag.into()),
                            edit_tags: Some(WaveformMsg::EditTags.into()),
                            favorite: None,
                        },
                    )
                })
                .style(context_menu_style);
                container(menu)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(2)
                    .into()
            }
            None => spacer(Length::Fill, Length::Fill).into(),
        };
        let track_name = self.current_file.as_deref().and_then(crate::path_util::file_name_lossy);

        container(column![
            waveform_area,
            self.controls.view(track_name, self.current_file.as_deref())
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .into()
    }
}

fn waveform_toolbar(zoom: f32, tags: Vec<(TagField, String)>) -> Element<'static, Message> {
    let zoom_button = |label, message: WaveformMsg| {
        button(text(label).size(15))
            .padding([2, 10])
            .on_press(message.into())
            .style(|theme: &Theme, status| {
                let palette = theme.extended_palette();
                let accent = palette.primary.base.color;
                let idle_border = palette.background.strong.color.scale_alpha(0.35);
                button::Style {
                    text_color: style::by_status(
                        status,
                        style::text_alpha(theme, 0.82),
                        palette.background.base.text,
                        palette.background.base.text,
                    ),
                    border: style::outline(
                        style::by_status(status, idle_border, accent.scale_alpha(0.35), idle_border),
                        6.0,
                    ),
                    ..button::Style::default()
                }
                .with_background(style::by_status(
                    status,
                    palette.background.weak.color.scale_alpha(0.42),
                    accent.scale_alpha(0.18),
                    accent.scale_alpha(0.28),
                ))
            })
    };
    let zoom_label = container(text(format!("Zoom {zoom:.1}×")).size(11).font(style::SEMIBOLD).style(
        |theme: &Theme| text::Style {
            color: Some(theme.extended_palette().primary.base.color.scale_alpha(0.92)),
        },
    ))
    .padding([4, 8])
    .style(|theme: &Theme| style::tinted(theme.extended_palette().primary.base.color, 0.14, 0.24, 6.0)(theme));

    let mut bar = row![
        zoom_label,
        zoom_button("−", WaveformMsg::ZoomOut),
        zoom_button("+", WaveformMsg::ZoomIn),
        zoom_button("?", WaveformMsg::Help),
        spacer(Length::Fill, Length::Shrink),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([6, 10]);
    if !tags.is_empty() {
        bar = bar.push(toolbar_tags(tags));
    }
    container(bar)
        .width(Length::Fill)
        .style(style::panel(0.48, 0.28, 0.0))
        .into()
}

fn toolbar_tags(tags: Vec<(TagField, String)>) -> Element<'static, Message> {
    let chips = tags.into_iter().map(|(field, value)| {
        let accent = style::tag_field_color(field);
        container(
            row![
                text(field.label())
                    .size(9)
                    .font(style::SEMIBOLD)
                    .color(accent.scale_alpha(0.88)),
                text(value).size(11).font(style::MEDIUM).style(style::faded_text(0.92)),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([3, 8])
        .style(style::tinted(accent, 0.12, 0.28, 6.0))
        .into()
    });
    container(
        scrollable(
            Row::with_children(chips)
                .spacing(6)
                .align_y(Alignment::Center)
                .padding([0, 2]),
        )
        .direction(Direction::Horizontal(Scrollbar::new().width(3).scroller_width(3)))
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .max_width(TOOLBAR_TAG_STRIP_MAX)
    .align_x(iced::alignment::Horizontal::Right)
    .into()
}

impl Controls {
    fn time_labels(&self) -> (String, String) {
        let progress = self
            .playback_progress
            .map(|shown| {
                self.playback_position
                    .as_ref()
                    .map_or(shown, |position| position.progress())
            })
            .unwrap_or(0.0);
        let label = |secs: Option<f64>| secs.map(format_duration).unwrap_or_else(|| "--:--".into());
        (
            label(self.track_duration.map(|duration| progress.clamp(0.0, 1.0) * duration)),
            label(self.track_duration),
        )
    }

    fn view(&self, track_name: Option<String>, track_path: Option<&Path>) -> Element<'_, Message> {
        let track_info: Element<'_, Message> = match (track_name, track_path) {
            (Some(name), Some(path)) => {
                let (current, total) = self.time_labels();
                container(track_info_row(name, path.to_path_buf(), current, total))
                    .width(Length::FillPortion(2))
                    .into()
            }
            _ => spacer(Length::Fill, Length::Shrink).into(),
        };
        container(
            row![track_info, self.volume_control(), self.transport_cluster()]
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .padding([6, 8])
        .width(Length::Fill)
        .style(style::panel(0.55, 0.35, 0.0))
        .into()
    }

    fn transport_cluster(&self) -> Element<'_, Message> {
        let playing = Arc::clone(&self.is_playing);
        let is_playing = move || playing.load(Ordering::SeqCst);
        let looping = self.looping.load(Ordering::Relaxed);
        let play = transport_button(
            if is_playing() { "pause.svg" } else { "play.svg" },
            Message::TogglePlaying,
            true,
            is_playing,
        );
        let stop = transport_button("stop.svg", Message::StopPlayback, false, || false);
        let repeat = transport_button("repeat.svg", Message::ToggleLoop, looping, move || looping);

        container(row![play, stop, repeat].spacing(6).align_y(Alignment::Center))
            .padding(4)
            .style(|theme: &Theme| {
                let palette = theme.extended_palette();
                container::Style::default()
                    .background(palette.background.base.color.scale_alpha(0.35))
                    .border(style::outline(palette.background.strong.color.scale_alpha(0.3), 999.0))
            })
            .into()
    }

    fn volume_control(&self) -> Element<'_, Message> {
        row![
            text("V").size(11).style(style::faded_text(0.62)),
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
}

/// A round transport button. `primary` buttons use the accent color, and
/// fill with it while `active()` (e.g. while playing).
fn transport_button(
    icon_name: &str,
    message: Message,
    primary: bool,
    active: impl Fn() -> bool + Clone + 'static,
) -> Button<'static, Message> {
    let icon_active = active.clone();
    let glyph = icon(icon_name, TRANSPORT_ICON, move |theme| {
        let accent = theme.extended_palette().primary.base.color;
        match (primary, icon_active()) {
            (true, true) => Color::WHITE,
            (true, false) => accent,
            (false, _) => style::text_alpha(theme, 0.78),
        }
    });
    button(glyph)
        .on_press(message)
        .width(Length::Fixed(TRANSPORT_BUTTON))
        .height(Length::Fixed(TRANSPORT_BUTTON))
        .style(move |theme: &Theme, status| {
            let palette = theme.extended_palette();
            let accent = palette.primary.base.color;
            let strong = palette.background.strong.color;
            let lit = primary && active();
            let background = if primary {
                style::by_status(
                    status,
                    if lit {
                        accent
                    } else {
                        palette.background.weak.color.scale_alpha(0.35)
                    },
                    accent.scale_alpha(if lit { 0.92 } else { 0.22 }),
                    accent.scale_alpha(0.78),
                )
            } else {
                style::by_status(
                    status,
                    palette.background.weak.color.scale_alpha(0.35),
                    strong.scale_alpha(0.28),
                    strong.scale_alpha(0.42),
                )
            };
            let border_color = if primary {
                accent.scale_alpha(0.55)
            } else {
                strong.scale_alpha(0.35)
            };
            button::Style {
                text_color: palette.background.base.text,
                border: style::outline(border_color, TRANSPORT_BUTTON / 2.0).width(if lit { 0.0 } else { 1.0 }),
                ..button::Style::default()
            }
            .with_background(background)
        })
}

fn track_info_row(name: String, path: PathBuf, current: String, total: String) -> Element<'static, Message> {
    mouse_area(
        row![
            icon("music-solid.svg", 14.0, |theme| theme
                .extended_palette()
                .primary
                .base
                .color
                .scale_alpha(0.85)),
            container(text(name).size(12).style(style::faded_text(0.72)))
                .width(Length::Fill)
                .clip(true),
            text("·").size(11).style(style::faded_text(0.42)),
            text(current)
                .size(12)
                .font(iced::Font::MONOSPACE)
                .style(style::faded_text(0.62)),
            text("/").size(11).style(style::faded_text(0.42)),
            text(total)
                .size(12)
                .font(iced::Font::MONOSPACE)
                .style(style::faded_text(0.52)),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Message::FileDragPress {
        path,
        from_file_list: false,
    })
    .interaction(iced::mouse::Interaction::Grab)
    .into()
}

fn volume_slider_style(theme: &Theme, status: slider::Status) -> slider::Style {
    let palette = theme.extended_palette();
    let accent = palette.primary.base.color;
    let (fill, handle_radius) = match status {
        slider::Status::Active => (accent.scale_alpha(0.72), 5.0),
        slider::Status::Hovered => (accent.scale_alpha(0.92), 5.5),
        slider::Status::Dragged => (accent, 6.0),
    };
    slider::Style {
        rail: Rail {
            backgrounds: (fill.into(), palette.background.strong.color.scale_alpha(0.42).into()),
            width: 3.0,
            border: iced::border::rounded(1.5),
        },
        handle: Handle {
            shape: HandleShape::Circle { radius: handle_radius },
            background: palette.background.base.color.into(),
            border_width: 1.5,
            border_color: fill,
        },
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up.
pub fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0).floor() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_with_position(total_frames: u64) -> (Player, Arc<PlaybackPosition>) {
        let mut player = Player::new(1.0, false);
        let position = PlaybackPosition::new(total_frames);
        player.controls.playback_position = Some(Arc::clone(&position));
        player.controls.playback_progress = Some(0.0);
        (player, position)
    }

    #[test]
    fn seek_moves_the_shared_position_before_the_audio_thread_runs() {
        let (mut player, position) = player_with_position(1_000);
        // Nothing drains the commands, so this stands in for the gap between releasing the
        // scrub and the audio thread handling `Seek`.
        let (worker, _commands) = PlayerWorker::detached();
        player.worker = Some(worker);
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
        assert!(player.toggle_loop());
        assert!(player.controls.looping.load(Ordering::Relaxed));
        assert!(!player.toggle_loop());
        assert!(!player.controls.looping.load(Ordering::Relaxed));
    }

    #[test]
    fn durations_switch_to_hours_at_an_hour() {
        assert_eq!(format_duration(59.9), "0:59");
        assert_eq!(format_duration(3_599.0), "59:59");
        assert_eq!(format_duration(3_600.0), "1:00:00");
        assert_eq!(format_duration(-4.0), "0:00");
    }
}
