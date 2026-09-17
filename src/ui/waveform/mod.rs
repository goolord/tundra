//! The waveform canvas: seeking, panning, zooming, and Ctrl+drag to drag the
//! file out. Zoom/pan math lives in `view`, drawing in `draw`.

mod draw;
mod view;

pub use view::WaveFormView;

use super::message::{Message, WaveformMsg};
use crate::playback::PlaybackPosition;
use crate::waveform_peaks::WaveformPeaks;
use iced::keyboard::Modifiers;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{Action, Cache, Event, Frame, Geometry, Program};
use iced::{Point, Rectangle, Renderer, Size, Theme};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const TIME_MARKER_HEIGHT: f32 = 16.0;
const AMPLITUDE_GUTTER: f32 = 36.0;
const PLOT_CLIP_BLEED_LEFT: f32 = 2.0;
const AMPLITUDE_PAD_TOP: f32 = 11.0;

/// Per-widget interaction state that iced keeps between frames.
#[derive(Default)]
pub struct WaveFormState {
    /// Where a Shift+drag pan started; `last_pan_view` is the live view while it runs.
    pan_anchor: Option<PanAnchor>,
    last_pan_view: Option<WaveFormView>,
    last_pan_x: Option<f32>,
    scrub_active: bool,
    last_scrub_progress: f64,
    /// Ctrl+press position, until the drag passes the threshold.
    file_drag_origin: Option<Point>,
    /// Scroll-wheel lines not yet turned into zoom.
    wheel_lines: f32,
    tracked_samples: usize,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct PanAnchor {
    view_offset: f64,
    overscroll: f32,
}

pub struct WaveForm {
    sample_count: usize,
    peaks: Arc<Mutex<WaveformPeaks>>,
    view: WaveFormView,
    sample_rate: u32,
    playback_position: Option<Arc<PlaybackPosition>>,
    is_playing: Option<Arc<AtomicBool>>,
    scrub_progress: Option<f64>,
    ui_scrubbing: Cell<bool>,
    modifiers: Modifiers,
    pan_active: bool,
    cache: Cache,
    content_cache_key: Cell<(u32, u32, u32, u32, u32)>,
    two_outline_armed: Cell<bool>,
}

/// The waveform's area inside the canvas, right of the amplitude labels and above the time ruler.
#[derive(Clone, Copy)]
struct PlotArea {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl PlotArea {
    fn from_size(size: Size) -> Self {
        let (x, y) = (AMPLITUDE_GUTTER, AMPLITUDE_PAD_TOP);
        Self {
            x,
            y,
            width: (size.width - x).max(1.0),
            height: (size.height - y - TIME_MARKER_HEIGHT).max(1.0),
        }
    }

    fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    fn amplitude_y(self, amplitude: f32) -> f32 {
        let half = self.height / 2.0;
        self.y + half - amplitude.clamp(-1.0, 1.0) * half
    }
}

impl WaveForm {
    pub fn new_pending(sample_count: usize, peaks: Arc<Mutex<WaveformPeaks>>) -> Self {
        Self {
            sample_count,
            peaks,
            view: WaveFormView::default(),
            sample_rate: 0,
            playback_position: None,
            is_playing: None,
            scrub_progress: None,
            ui_scrubbing: Cell::new(false),
            modifiers: Modifiers::default(),
            pan_active: false,
            cache: Cache::new(),
            content_cache_key: Cell::new((0, 0, 0, 0, 0)),
            two_outline_armed: Cell::new(false),
        }
    }

    pub fn sample_count(&self) -> usize {
        self.sample_count
    }

    /// Adopts the peak builder's exact sample count once it finishes.
    pub fn apply_peaks_ready(&mut self) -> Option<usize> {
        let count = self
            .peaks
            .lock()
            .ok()
            .filter(|peaks| peaks.complete && peaks.sample_count > 0)
            .map(|peaks| peaks.sample_count)?;
        if self.sample_count != count {
            self.sample_count = count;
            self.invalidate_cache();
        }
        Some(count)
    }

    pub fn invalidate_cache(&mut self) {
        self.cache.clear();
        self.content_cache_key.set((0, 0, 0, 0, 0));
    }

    /// Everything the cached waveform drawing depends on.
    fn content_cache_key(&self, theme: &Theme, plot_width: f32) -> (u32, u32, u32, u32, u32) {
        let peaks_complete = self.peaks.lock().is_ok_and(|peaks| peaks.complete);
        let (start, zoom, phase) = self.view.window_cache_key(self.sample_count);
        let theme_key = draw::theme_cache_key(theme) ^ u32::from(peaks_complete);
        (start, zoom, phase, plot_width.round() as u32, theme_key)
    }

    fn sync_content_cache(&self, theme: &Theme, plot_width: f32) {
        let key = self.content_cache_key(theme, plot_width);
        if self.content_cache_key.get() != key {
            self.cache.clear();
            self.content_cache_key.set(key);
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_playback(&mut self, position: Arc<PlaybackPosition>, is_playing: Arc<AtomicBool>) {
        self.playback_position = Some(position);
        self.is_playing = Some(is_playing);
    }

    fn is_playing(&self) -> bool {
        self.is_playing.as_ref().is_some_and(|playing| playing.load(Ordering::Relaxed))
    }

    /// Draws the playhead at `progress` instead of the playback position while scrubbing.
    pub fn set_scrub_progress(&mut self, progress: Option<f64>) {
        self.scrub_progress = progress;
    }

    pub fn set_ui_scrubbing(&self, scrubbing: bool) {
        self.ui_scrubbing.set(scrubbing);
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }

    pub fn set_pan_active(&mut self, active: bool) {
        self.pan_active = active;
    }

    pub fn pan_active(&self) -> bool {
        self.pan_active
    }

    pub fn view_state(&self) -> WaveFormView {
        self.view
    }

    pub fn set_view(&mut self, view: WaveFormView) {
        self.view = view;
    }

    /// Mirrors the transform `with_content_transform` applies while drawing, including its
    /// overscroll-dependent origin. Using a fixed `width / 2.0` origin here would put the playhead
    /// and the click-to-seek mapping on a different transform than the waveform underneath them.
    fn map_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, _) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        let origin_x = view.content_transform_origin_x(width, self.sample_count);
        (x - origin_x) * scale_x + origin_x + translate_x
    }

    fn unmap_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, _) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        let origin_x = view.content_transform_origin_x(width, self.sample_count);
        if scale_x.abs() < 1e-6 {
            return origin_x;
        }
        (x - translate_x - origin_x) / scale_x + origin_x
    }

    fn playback_progress(&self) -> Option<f64> {
        self.scrub_progress
            .or_else(|| self.playback_position.as_ref().map(|position| position.progress()))
    }

    /// Frames in the track (by the playback clock when known) and the visible
    /// `(start, visible, phase)` window; `None` when there is nothing to map.
    fn timeline(&self, view: WaveFormView, plot_width: f32) -> Option<(usize, usize, usize, f32)> {
        if self.sample_count == 0 || plot_width <= 0.0 {
            return None;
        }
        let frame_count = self
            .playback_position
            .as_ref()
            .map_or(self.sample_count, |position| position.total_frames() as usize);
        let (start, end, phase) = view.sample_window(self.sample_count);
        let visible = end.saturating_sub(start);
        (frame_count > 0 && visible > 0).then_some((frame_count, start, visible, phase))
    }

    fn playhead_content_x(&self, view: WaveFormView, plot_width: f32, progress: f64) -> Option<f32> {
        let (frame_count, start, visible, phase) = self.timeline(view, plot_width)?;
        let progress_frame = ((progress * frame_count as f64).round() as usize).min(frame_count - 1);
        let px_per_sample = plot_width / visible as f32;
        // Signed on purpose: a playhead before the window belongs off the left edge. Saturating the
        // subtraction would park it on the first visible sample and read as a stuck playhead.
        let sample_pos = progress_frame as f64 - start as f64 - phase as f64;
        Some((sample_pos * px_per_sample as f64) as f32)
    }

    fn playhead_screen_x(&self, view: WaveFormView, plot: PlotArea, progress: f64) -> Option<f32> {
        let x = self.playhead_content_x(view, plot.width, progress)?;
        Some(self.map_content_x(view, plot.width, x) + plot.x)
    }

    fn progress_at_x(&self, view: WaveFormView, plot: PlotArea, x: f32) -> Option<f64> {
        let (frame_count, start, visible, phase) = self.timeline(view, plot.width)?;
        let px_per_sample = plot.width / visible as f32;
        let content_x = self.unmap_content_x(view, plot.width, x - plot.x);
        let sample_pos = (content_x / px_per_sample + phase).clamp(0.0, visible as f32);
        let progress_frame = (start as f32 + sample_pos).round() as usize;
        Some(progress_frame.min(frame_count - 1) as f64 / frame_count as f64)
    }

    /// Scroll-wheel zoom, or pan with Shift. Returns the new view if it changed.
    fn wheel(&self, state: &mut WaveFormState, delta: mouse::ScrollDelta, bounds: Rectangle, cursor: Cursor) -> Option<WaveFormView> {
        let mut view = self.view;
        if self.modifiers.shift() {
            let (x, y) = view::scroll_lines(delta);
            let pan_delta = if x.abs() > y.abs() { -x } else { -y };
            if pan_delta == 0.0 {
                return None;
            }
            view.apply_pan_delta(f64::from(pan_delta * view::PAN_STEP), self.sample_count);
            return Some(view);
        }
        let plot = PlotArea::from_size(bounds.size());
        let anchor_x = cursor
            .position_in(bounds)
            .map_or(0.5, |point| ((point.x - plot.x) / plot.width).clamp(0.0, 1.0));
        let lines = view::scroll_lines(delta).1;
        view.accumulate_wheel(lines, anchor_x, self.sample_count, &mut state.wheel_lines)
            .then_some(view)
    }
}

fn publish(message: WaveformMsg) -> Option<Action<Message>> {
    Some(Action::publish(message.into()).and_capture())
}

impl Program<Message> for WaveForm {
    type State = WaveFormState;

    fn draw(
        &self,
        state: &WaveFormState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let size = bounds.size();
        let progress = self.playback_progress();
        let view = state.last_pan_view.unwrap_or(self.view);
        let transformed = view.overscroll_active();
        let plot = PlotArea::from_size(size);
        let plot_size = plot.size();
        let plot_clip = Rectangle::new(
            Point::new(plot.x - PLOT_CLIP_BLEED_LEFT, plot.y),
            Size::new(plot.width + PLOT_CLIP_BLEED_LEFT, plot.height + TIME_MARKER_HEIGHT),
        );
        let draw_plot = |frame: &mut Frame| {
            frame.with_clip(plot_clip, |frame| {
                Self::with_plot_origin(frame, plot, |frame| {
                    if !transformed {
                        self.draw_waveform_content(frame, theme, view, plot_size);
                        return;
                    }
                    // While rubber-banding, the playhead has to share the stretch transform.
                    Self::with_content_transform(frame, view, plot_size, self.sample_count, |frame| {
                        self.draw_waveform_content(frame, theme, view, plot_size);
                        if let Some(progress) = progress {
                            self.draw_playhead_on_frame(frame, theme, plot_size, progress, view);
                        }
                    });
                });
            });
        };

        let mut background = Frame::new(renderer, size);
        self.draw_background(&mut background, theme, size);
        self.draw_amplitude_axis(&mut background, theme, size);
        let mut layers = vec![background.into_geometry()];

        // Panning and rubber-banding change every frame; everything else draws from the cache.
        if state.last_pan_view.is_some() || transformed {
            let mut frame = Frame::new(renderer, size);
            draw_plot(&mut frame);
            layers.push(frame.into_geometry());
        } else {
            self.sync_content_cache(theme, plot_size.width);
            layers.push(self.cache.draw(renderer, size, draw_plot));
        }

        if let Some(progress) = progress
            && !transformed
            && let Some(playhead) = self.draw_playhead(renderer, size, theme, progress, view)
        {
            layers.push(playhead);
        }
        layers
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Message>> {
        // The playhead reads the shared position directly, so while playing the
        // canvas redraws itself every frame without rebuilding the app view.
        if let Event::Window(iced::window::Event::RedrawRequested(_)) = event {
            return self.is_playing().then(Action::request_redraw);
        }
        if state.tracked_samples != self.sample_count {
            state.tracked_samples = self.sample_count;
            state.wheel_lines = 0.0;
        }
        if !self.ui_scrubbing.get() {
            state.scrub_active = false;
        }
        let Event::Mouse(event) = event else {
            return None;
        };

        match *event {
            mouse::Event::CursorEntered => publish(WaveformMsg::HoverChanged(true)),
            mouse::Event::CursorLeft => publish(WaveformMsg::HoverChanged(false)),
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.file_drag_origin = None;
                if state.scrub_active {
                    state.scrub_active = false;
                    self.ui_scrubbing.set(false);
                    return publish(WaveformMsg::ScrubEnd(state.last_scrub_progress));
                }
                if state.pan_anchor.take().is_some() {
                    state.last_pan_x = None;
                    return publish(WaveformMsg::PanEnded(state.last_pan_view.take().unwrap_or(self.view)));
                }
                None
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let position = cursor.position_in(bounds)?;
                if self.modifiers.control() {
                    state.file_drag_origin = Some(position);
                    return Some(Action::capture());
                }
                if self.modifiers.shift() {
                    state.pan_anchor = Some(PanAnchor {
                        view_offset: self.view.offset,
                        overscroll: self.view.overscroll,
                    });
                    state.last_pan_x = Some(position.x);
                    state.last_pan_view = None;
                    return publish(WaveformMsg::PanStarted);
                }
                let plot = PlotArea::from_size(bounds.size());
                // The amplitude gutter is not part of the timeline; a press there would otherwise
                // clamp to the window start and seek.
                if position.x < plot.x {
                    return None;
                }
                let progress = self.progress_at_x(state.last_pan_view.unwrap_or(self.view), plot, position.x)?;
                state.scrub_active = true;
                state.last_scrub_progress = progress;
                self.ui_scrubbing.set(true);
                publish(WaveformMsg::Scrub(progress))
            }
            mouse::Event::CursorMoved { .. } => {
                let position = cursor.position_in(bounds)?;
                if let Some(origin) = state.file_drag_origin
                    && position.distance(origin) >= super::FILE_DRAG_THRESHOLD
                {
                    state.file_drag_origin = None;
                    return publish(WaveformMsg::FileDragStart);
                }
                if state.scrub_active
                    && let Some(progress) = self.progress_at_x(
                        state.last_pan_view.unwrap_or(self.view),
                        PlotArea::from_size(bounds.size()),
                        position.x,
                    )
                {
                    state.last_scrub_progress = progress;
                    return publish(WaveformMsg::Scrub(progress));
                }
                let anchor = state.pan_anchor?;
                let last_x = state.last_pan_x.replace(position.x).unwrap_or(position.x);
                let plot = PlotArea::from_size(bounds.size());
                let visible = view::visible_fraction_of(self.sample_count, self.view.zoom);
                let step = -f64::from(position.x - last_x) / f64::from(plot.width) * visible;
                let mut view = state.last_pan_view.unwrap_or(WaveFormView {
                    zoom: self.view.zoom,
                    offset: anchor.view_offset,
                    overscroll: anchor.overscroll,
                });
                view.apply_pan_delta(step, self.sample_count);
                state.last_pan_view = Some(view);
                Some(Action::request_redraw().and_capture())
            }
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                self.wheel(state, delta, bounds, cursor).and_then(|view| publish(WaveformMsg::ViewChanged(view)))
            }
            _ => None,
        }
    }

    fn mouse_interaction(&self, state: &Self::State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            mouse::Interaction::default()
        } else if state.scrub_active {
            mouse::Interaction::Pointer
        } else if state.pan_anchor.is_some() {
            mouse::Interaction::Grabbing
        } else if self.modifiers.control() || self.modifiers.shift() {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::Pointer
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waveform(sample_count: usize, zoom: f32, offset: f64, overscroll: f32) -> WaveForm {
        let mut waveform = WaveForm::new_pending(sample_count, Arc::new(Mutex::new(WaveformPeaks::empty())));
        waveform.view = WaveFormView { zoom, offset, overscroll };
        waveform
    }

    #[test]
    fn click_lands_where_the_playhead_is_drawn() {
        let sample_count = 480_000;
        let plot = PlotArea::from_size(Size::new(832.0, 240.0));

        for (zoom, offset) in [(1.0_f32, 0.0_f64), (8.0, 0.1), (64.0, 0.5), (512.0, 0.75)] {
            let wf = waveform(sample_count, zoom, offset, 0.0);
            let (start, end, _) = wf.view.sample_window(sample_count);
            let tolerance = 1.0 + plot.width / (end - start) as f32;

            for step in 0..=20 {
                let x = plot.x + plot.width * (step as f32 / 20.0);
                let progress = wf
                    .progress_at_x(wf.view, plot, x)
                    .expect("click inside the plot must resolve to a progress");
                let back = wf
                    .playhead_screen_x(wf.view, plot, progress)
                    .expect("progress must resolve back to a playhead x");
                assert!(
                    (back - x).abs() <= tolerance,
                    "zoom {zoom} offset {offset}: clicked {x}, playhead drawn at {back}"
                );
            }
        }
    }

    /// `map_content_x` has to reproduce the transform `with_content_transform` applies, otherwise
    /// the playhead and the click mapping drift away from the waveform during an overscroll
    /// rubber-band.
    #[test]
    fn content_mapping_matches_the_drawing_transform() {
        let sample_count = 480_000;
        let width = 832.0_f32;

        for (offset, overscroll) in [(0.0_f64, -0.1_f32), (0.9, 0.1), (0.4, 0.05), (0.4, 0.0)] {
            let wf = waveform(sample_count, 8.0, offset, overscroll);
            let view = wf.view;
            let (scale_x, _) = view.content_scale();
            let translate_x = view.content_translate_x(width);
            let origin_x = view.content_transform_origin_x(width, sample_count);

            for step in 0..=10 {
                let x = width * (step as f32 / 10.0);
                let drawn = (x - origin_x) * scale_x + origin_x + translate_x;
                let mapped = wf.map_content_x(view, width, x);
                assert!(
                    (mapped - drawn).abs() < 1e-3,
                    "offset {offset} overscroll {overscroll}: content {x} drawn at {drawn}, mapped to {mapped}"
                );
                let back = wf.unmap_content_x(view, width, mapped);
                assert!(
                    (back - x).abs() < 1e-2,
                    "offset {offset} overscroll {overscroll}: {x} round-tripped to {back}"
                );
            }
        }
    }

    #[test]
    fn cache_key_includes_plot_width() {
        let wf = waveform(10_000, 1.0, 0.0, 0.0);
        let theme = Theme::Dark;
        assert_ne!(wf.content_cache_key(&theme, 400.0), wf.content_cache_key(&theme, 800.0));
    }

    #[test]
    fn playhead_left_of_the_window_is_not_pinned_to_the_left_edge() {
        let sample_count = 480_000;
        let wf = waveform(sample_count, 8.0, 0.5, 0.0);
        let plot = PlotArea::from_size(Size::new(832.0, 240.0));
        let (start, _, _) = wf.view.sample_window(sample_count);
        assert!(start > 0);

        // Playback sits well before the visible window, so the playhead belongs off-screen
        // to the left rather than parked on the first visible sample.
        let progress = (start as f64 / 2.0) / sample_count as f64;
        let x = wf
            .playhead_content_x(wf.view, plot.width, progress)
            .expect("playhead x should be computable");
        assert!(x < 0.0, "playhead for a sample before the window should be left of the plot, got {x}");
    }
}
