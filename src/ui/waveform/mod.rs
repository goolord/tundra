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
use iced::{Point, Rectangle, Renderer, Size, Theme, Vector};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const TIME_MARKER_HEIGHT: f32 = 16.0;
const AMPLITUDE_GUTTER: f32 = 36.0;
const PLOT_CLIP_BLEED_LEFT: f32 = 2.0;
const AMPLITUDE_PAD_TOP: f32 = 11.0;

/// Everything the cached waveform drawing depends on.
type CacheKey = (u32, u32, u32, u32, u32);

/// Per-widget interaction state that iced keeps between frames.
#[derive(Default)]
pub struct WaveFormState {
    /// A Shift+drag pan's starting view and the last cursor x; `last_pan_view` is the live view.
    pan: Option<(WaveFormView, f32)>,
    last_pan_view: Option<WaveFormView>,
    scrub_active: bool,
    last_scrub_progress: f64,
    /// Ctrl+press position, until the drag passes the threshold.
    file_drag_origin: Option<Point>,
    /// Scroll-wheel lines not yet turned into zoom.
    wheel_lines: f32,
    tracked_samples: usize,
}

pub struct WaveForm {
    sample_count: usize,
    peaks: Arc<Mutex<WaveformPeaks>>,
    view: WaveFormView,
    sample_rate: u32,
    playback_position: Arc<PlaybackPosition>,
    is_playing: Arc<AtomicBool>,
    scrub_progress: Option<f64>,
    ui_scrubbing: Cell<bool>,
    modifiers: Modifiers,
    pan_active: bool,
    cache: Cache,
    content_cache_key: Cell<Option<CacheKey>>,
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
        let (width, height) = (
            (size.width - x).max(1.0),
            (size.height - y - TIME_MARKER_HEIGHT).max(1.0),
        );
        Self { x, y, width, height }
    }

    fn amplitude_y(self, amplitude: f32) -> f32 {
        let half = self.height / 2.0;
        self.y + half - amplitude.clamp(-1.0, 1.0) * half
    }
}

impl WaveForm {
    /// A waveform for a track that `peaks` fills in as it decodes.
    pub fn new(
        sample_count: usize,
        sample_rate: u32,
        peaks: Arc<Mutex<WaveformPeaks>>,
        playback_position: Arc<PlaybackPosition>,
        is_playing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sample_count,
            peaks,
            view: WaveFormView::default(),
            sample_rate,
            playback_position,
            is_playing,
            scrub_progress: None,
            ui_scrubbing: Cell::new(false),
            modifiers: Modifiers::default(),
            pan_active: false,
            cache: Cache::new(),
            content_cache_key: Cell::new(None),
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
        self.content_cache_key.set(None);
    }

    fn content_cache_key(&self, theme: &Theme, plot_width: f32) -> CacheKey {
        let peaks_complete = self.peaks.lock().is_ok_and(|peaks| peaks.complete);
        let (start, zoom, phase) = self.view.window_cache_key(self.sample_count);
        let theme_key = draw::theme_cache_key(theme) ^ u32::from(peaks_complete);
        (start, zoom, phase, plot_width.round() as u32, theme_key)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
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

    /// The rubber-band stretch as drawn: `(x scale, x translation, x origin)`. The origin depends
    /// on the overscroll; a fixed `width / 2.0` here would put the playhead and the click-to-seek
    /// mapping on a different transform than the waveform underneath them.
    fn content_transform(&self, view: WaveFormView, width: f32) -> (f32, f32, f32) {
        let origin_x = view.content_transform_origin_x(width, self.sample_count);
        (view.content_scale().0, view.content_translate_x(width), origin_x)
    }

    fn map_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, translate_x, origin_x) = self.content_transform(view, width);
        (x - origin_x) * scale_x + origin_x + translate_x
    }

    fn unmap_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, translate_x, origin_x) = self.content_transform(view, width);
        if scale_x.abs() < 1e-6 {
            return origin_x;
        }
        (x - translate_x - origin_x) / scale_x + origin_x
    }

    /// Frames in the track by the playback clock, and the visible `(start, visible, phase)`
    /// window; `None` when there is nothing to map.
    fn timeline(&self, view: WaveFormView, plot_width: f32) -> Option<(usize, usize, usize, f32)> {
        if self.sample_count == 0 || plot_width <= 0.0 {
            return None;
        }
        let frame_count = self.playback_position.total_frames() as usize;
        let (start, visible, phase) = view.sample_window(self.sample_count);
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
    fn wheel(
        &self,
        state: &mut WaveFormState,
        delta: mouse::ScrollDelta,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<WaveFormView> {
        let mut view = self.view;
        let (x, y) = view::scroll_lines(delta);
        if self.modifiers.shift() {
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
        view.accumulate_wheel(y, anchor_x, self.sample_count, &mut state.wheel_lines)
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
        let progress = self.scrub_progress.unwrap_or_else(|| self.playback_position.progress());
        let view = state.last_pan_view.unwrap_or(self.view);
        let transformed = view.overscroll_active();
        let plot = PlotArea::from_size(size);
        let plot_size = Size::new(plot.width, plot.height);
        let plot_clip = Rectangle::new(
            Point::new(plot.x - PLOT_CLIP_BLEED_LEFT, plot.y),
            Size::new(plot.width + PLOT_CLIP_BLEED_LEFT, plot.height + TIME_MARKER_HEIGHT),
        );
        let draw_plot = |frame: &mut Frame| {
            frame.with_clip(plot_clip, |frame| {
                frame.translate(Vector::new(plot.x, plot.y));
                if !transformed {
                    return self.draw_waveform_content(frame, theme, view, plot_size);
                }
                // While rubber-banding, the playhead has to share the stretch transform.
                let (scale_x, translate_x, origin_x) = self.content_transform(view, plot.width);
                let origin = Vector::new(origin_x, plot.height / 2.0);
                frame.translate(Vector::new(translate_x, 0.0) + origin);
                frame.scale_nonuniform(Vector::new(scale_x, view.content_scale().1));
                frame.translate(-origin);
                self.draw_waveform_content(frame, theme, view, plot_size);
                if let Some(x) = self.playhead_content_x(view, plot.width, progress) {
                    draw::stroke_playhead(frame, x, plot.height, theme);
                }
            });
        };

        let mut background = Frame::new(renderer, size);
        draw::draw_background(&mut background, theme, size);
        let mut layers = vec![background.into_geometry()];

        // Panning and rubber-banding change every frame; everything else draws from the cache.
        if state.last_pan_view.is_some() || transformed {
            let mut frame = Frame::new(renderer, size);
            draw_plot(&mut frame);
            layers.push(frame.into_geometry());
        } else {
            let key = Some(self.content_cache_key(theme, plot.width));
            if self.content_cache_key.replace(key) != key {
                self.cache.clear();
            }
            layers.push(self.cache.draw(renderer, size, draw_plot));
        }

        // This layer is unclipped, so an off-window playhead would streak across the amplitude
        // gutter. The bounds match `plot_clip`, which the transformed path draws through, so the
        // head does not jump when overscroll settles.
        if !transformed
            && let Some(x) = self.playhead_screen_x(view, plot, progress)
            && (plot.x - PLOT_CLIP_BLEED_LEFT..=plot.x + plot.width).contains(&x)
        {
            let mut frame = Frame::new(renderer, size);
            draw::stroke_playhead(&mut frame, x, size.height, theme);
            layers.push(frame.into_geometry());
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
            return self.is_playing.load(Ordering::Relaxed).then(Action::request_redraw);
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
        let plot = PlotArea::from_size(bounds.size());
        let current_view = state.last_pan_view.unwrap_or(self.view);

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
                state.pan.take()?;
                publish(WaveformMsg::PanEnded(state.last_pan_view.take().unwrap_or(self.view)))
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let position = cursor.position_in(bounds)?;
                if self.modifiers.control() {
                    state.file_drag_origin = Some(position);
                    return Some(Action::capture());
                }
                if self.modifiers.shift() {
                    state.pan = Some((self.view, position.x));
                    state.last_pan_view = None;
                    return publish(WaveformMsg::PanStarted);
                }
                // The amplitude gutter is not part of the timeline; a press there would otherwise
                // clamp to the window start and seek.
                if position.x < plot.x {
                    return None;
                }
                let progress = self.progress_at_x(current_view, plot, position.x)?;
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
                    && let Some(progress) = self.progress_at_x(current_view, plot, position.x)
                {
                    state.last_scrub_progress = progress;
                    return publish(WaveformMsg::Scrub(progress));
                }
                let (anchor, last_x) = state.pan.as_mut()?;
                let dx = position.x - std::mem::replace(last_x, position.x);
                let visible = view::visible_fraction_of(self.sample_count, self.view.zoom);
                let mut view = state.last_pan_view.unwrap_or(WaveFormView {
                    zoom: self.view.zoom,
                    ..*anchor
                });
                view.apply_pan_delta(-f64::from(dx) / f64::from(plot.width) * visible, self.sample_count);
                state.last_pan_view = Some(view);
                Some(Action::request_redraw().and_capture())
            }
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => self
                .wheel(state, delta, bounds, cursor)
                .and_then(|view| publish(WaveformMsg::ViewChanged(view))),
            _ => None,
        }
    }

    fn mouse_interaction(&self, state: &Self::State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            mouse::Interaction::default()
        } else if state.scrub_active {
            mouse::Interaction::Pointer
        } else if state.pan.is_some() {
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

    const SAMPLES: usize = 480_000;

    fn waveform(sample_count: usize, zoom: f32, offset: f64, overscroll: f32) -> WaveForm {
        let peaks = Arc::new(Mutex::new(WaveformPeaks::empty()));
        let position = PlaybackPosition::new(sample_count as u64);
        let mut waveform = WaveForm::new(sample_count, 48_000, peaks, position, Default::default());
        waveform.view = WaveFormView {
            zoom,
            offset,
            overscroll,
        };
        waveform
    }

    #[test]
    fn click_lands_where_the_playhead_is_drawn() {
        let plot = PlotArea::from_size(Size::new(832.0, 240.0));
        for (zoom, offset) in [(1.0_f32, 0.0_f64), (8.0, 0.1), (64.0, 0.5), (512.0, 0.75)] {
            let wf = waveform(SAMPLES, zoom, offset, 0.0);
            let tolerance = 1.0 + plot.width / wf.view.sample_window(SAMPLES).1 as f32;
            for step in 0..=20 {
                let x = plot.x + plot.width * (step as f32 / 20.0);
                let progress = wf
                    .progress_at_x(wf.view, plot, x)
                    .expect("click inside the plot resolves");
                let back = wf
                    .playhead_screen_x(wf.view, plot, progress)
                    .expect("progress resolves back");
                assert!(
                    (back - x).abs() <= tolerance,
                    "zoom {zoom} offset {offset}: clicked {x}, drawn at {back}"
                );
            }
        }
    }

    /// `map_content_x` has to reproduce the transform `draw` applies, otherwise the playhead and
    /// the click mapping drift away from the waveform during an overscroll rubber-band.
    #[test]
    fn content_mapping_matches_the_drawing_transform() {
        let width = 832.0_f32;
        for (offset, overscroll) in [(0.0_f64, -0.1_f32), (0.9, 0.1), (0.4, 0.05), (0.4, 0.0)] {
            let wf = waveform(SAMPLES, 8.0, offset, overscroll);
            let view = wf.view;
            let (scale_x, _) = view.content_scale();
            let translate_x = view.content_translate_x(width);
            let origin_x = view.content_transform_origin_x(width, SAMPLES);
            for step in 0..=10 {
                let x = width * (step as f32 / 10.0);
                let drawn = (x - origin_x) * scale_x + origin_x + translate_x;
                let mapped = wf.map_content_x(view, width, x);
                assert!(
                    (mapped - drawn).abs() < 1e-3,
                    "{offset}/{overscroll}: {x} drawn at {drawn}, mapped {mapped}"
                );
                let back = wf.unmap_content_x(view, width, mapped);
                assert!(
                    (back - x).abs() < 1e-2,
                    "{offset}/{overscroll}: {x} round-tripped to {back}"
                );
            }
        }
    }

    #[test]
    fn cache_key_includes_plot_width() {
        let wf = waveform(10_000, 1.0, 0.0, 0.0);
        assert_ne!(
            wf.content_cache_key(&Theme::Dark, 400.0),
            wf.content_cache_key(&Theme::Dark, 800.0)
        );
    }

    #[test]
    fn playhead_left_of_the_window_is_not_pinned_to_the_left_edge() {
        let wf = waveform(SAMPLES, 8.0, 0.5, 0.0);
        let (start, _, _) = wf.view.sample_window(SAMPLES);
        assert!(start > 0);
        // Playback sits well before the visible window, so the playhead belongs off-screen
        // to the left rather than parked on the first visible sample.
        let progress = (start as f64 / 2.0) / SAMPLES as f64;
        let x = wf.playhead_content_x(wf.view, 832.0, progress).expect("playhead x");
        assert!(
            x < 0.0,
            "playhead for a sample before the window should be left of the plot, got {x}"
        );
    }
}
