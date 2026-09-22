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
    /// During a Shift+drag pan, the live view and the last cursor x.
    pan: Option<(WaveFormView, f32)>,
    /// During a scrub, the last progress it reported.
    scrub: Option<f64>,
    /// Ctrl+press position, until the drag passes the threshold.
    file_drag_origin: Option<Point>,
    /// Scroll-wheel lines not yet turned into zoom.
    wheel_lines: f32,
    tracked_samples: usize,
}

pub struct WaveForm {
    peaks: Arc<Mutex<WaveformPeaks>>,
    pub view: WaveFormView,
    pub sample_rate: u32,
    /// The playhead; its total frame count is also the waveform's length.
    pub position: Arc<PlaybackPosition>,
    is_playing: Arc<AtomicBool>,
    /// Draws the playhead here instead of at the playback position while scrubbing.
    pub scrub_progress: Option<f64>,
    pub ui_scrubbing: Cell<bool>,
    pub modifiers: Modifiers,
    pub pan_active: bool,
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
        let (width, height) = ((size.width - x).max(1.0), (size.height - y - TIME_MARKER_HEIGHT).max(1.0));
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
        sample_rate: u32,
        peaks: Arc<Mutex<WaveformPeaks>>,
        position: Arc<PlaybackPosition>,
        is_playing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            peaks,
            view: WaveFormView::default(),
            sample_rate,
            position,
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
        self.position.total_frames() as usize
    }

    /// Everything the cached drawing depends on. New peaks flip `complete`, which changes it.
    fn content_cache_key(&self, theme: &Theme, plot_width: f32) -> CacheKey {
        let peaks_complete = self.peaks.lock().is_ok_and(|peaks| peaks.complete);
        let (start, zoom, phase) = self.view.window_cache_key(self.sample_count());
        let theme_key = draw::theme_cache_key(theme) ^ u32::from(peaks_complete);
        (start, zoom, phase, plot_width.round() as u32, theme_key)
    }

    /// Maps plot x through the rubber-band stretch `draw` applies, or back with `unmap`.
    fn map_content_x(&self, view: WaveFormView, width: f32, x: f32, unmap: bool) -> f32 {
        let (scale_x, _, translate_x, origin_x) = view.content_transform(width, self.sample_count());
        if !unmap {
            (x - origin_x) * scale_x + origin_x + translate_x
        } else if scale_x.abs() < 1e-6 {
            origin_x
        } else {
            (x - translate_x - origin_x) / scale_x + origin_x
        }
    }

    /// Frames in the track, and the visible `(start, visible, phase)` window; `None` when there
    /// is nothing to map.
    fn timeline(&self, view: WaveFormView, plot_width: f32) -> Option<(usize, usize, usize, f32)> {
        let frame_count = self.sample_count();
        let (start, visible, phase) = view.sample_window(frame_count);
        (visible > 0 && plot_width > 0.0).then_some((frame_count, start, visible, phase))
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
        Some(self.map_content_x(view, plot.width, x, false) + plot.x)
    }

    fn progress_at_x(&self, view: WaveFormView, plot: PlotArea, x: f32) -> Option<f64> {
        let (frame_count, start, visible, phase) = self.timeline(view, plot.width)?;
        let px_per_sample = plot.width / visible as f32;
        let content_x = self.map_content_x(view, plot.width, x - plot.x, true);
        let sample_pos = (content_x / px_per_sample + phase).clamp(0.0, visible as f32);
        let progress_frame = (start as f32 + sample_pos).round() as usize;
        Some(progress_frame.min(frame_count - 1) as f64 / frame_count as f64)
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
        let progress = self.scrub_progress.unwrap_or_else(|| self.position.progress());
        let panning = state.pan.is_some();
        let view = state.pan.map_or(self.view, |(view, _)| view);
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
                let (scale_x, scale_y, translate_x, origin_x) = view.content_transform(plot.width, self.sample_count());
                let origin = Vector::new(origin_x, plot.height / 2.0);
                frame.translate(Vector::new(translate_x, 0.0) + origin);
                frame.scale_nonuniform(Vector::new(scale_x, scale_y));
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
        if panning || transformed {
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
        if state.tracked_samples != self.sample_count() {
            state.tracked_samples = self.sample_count();
            state.wheel_lines = 0.0;
        }
        if !self.ui_scrubbing.get() {
            state.scrub = None;
        }
        let Event::Mouse(event) = event else {
            return None;
        };
        let plot = PlotArea::from_size(bounds.size());
        let current_view = state.pan.map_or(self.view, |(view, _)| view);

        match *event {
            mouse::Event::CursorEntered => publish(WaveformMsg::HoverChanged(true)),
            mouse::Event::CursorLeft => publish(WaveformMsg::HoverChanged(false)),
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                state.file_drag_origin = None;
                if let Some(progress) = state.scrub.take() {
                    self.ui_scrubbing.set(false);
                    return publish(WaveformMsg::ScrubEnd(progress));
                }
                publish(WaveformMsg::PanEnded(state.pan.take()?.0))
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                let position = cursor.position_in(bounds)?;
                if self.modifiers.control() {
                    state.file_drag_origin = Some(position);
                    return Some(Action::capture());
                }
                if self.modifiers.shift() {
                    state.pan = Some((self.view, position.x));
                    return publish(WaveformMsg::PanStarted);
                }
                // The amplitude gutter is not part of the timeline; a press there would otherwise
                // clamp to the window start and seek.
                if position.x < plot.x {
                    return None;
                }
                let progress = self.progress_at_x(current_view, plot, position.x)?;
                state.scrub = Some(progress);
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
                if state.scrub.is_some()
                    && let Some(progress) = self.progress_at_x(current_view, plot, position.x)
                {
                    state.scrub = Some(progress);
                    return publish(WaveformMsg::Scrub(progress));
                }
                let (view, last_x) = state.pan.as_mut()?;
                let dx = position.x - std::mem::replace(last_x, position.x);
                let visible = view::visible_fraction_of(self.sample_count(), view.zoom);
                view.apply_pan_delta(-f64::from(dx) / f64::from(plot.width) * visible, self.sample_count());
                Some(Action::request_redraw().and_capture())
            }
            // Zoom around the cursor, or pan with Shift.
            mouse::Event::WheelScrolled { delta } if cursor.is_over(bounds) => {
                let (mut view, sample_count) = (self.view, self.sample_count());
                let (x, y) = view::scroll_lines(delta);
                let changed = if self.modifiers.shift() {
                    let pan_delta = if x.abs() > y.abs() { -x } else { -y };
                    view.apply_pan_delta(f64::from(pan_delta * view::PAN_STEP), sample_count);
                    pan_delta != 0.0
                } else {
                    let anchor_x = cursor.position_in(bounds).map_or(0.5, |point| (point.x - plot.x) / plot.width);
                    view.accumulate_wheel(y, anchor_x.clamp(0.0, 1.0), sample_count, &mut state.wheel_lines)
                };
                changed.then(|| publish(WaveformMsg::ViewChanged(view))).flatten()
            }
            _ => None,
        }
    }

    fn mouse_interaction(&self, state: &Self::State, bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            mouse::Interaction::default()
        } else if state.scrub.is_some() {
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
        let mut waveform = WaveForm::new(48_000, peaks, position, Default::default());
        waveform.view = WaveFormView { zoom, offset, overscroll };
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
                let progress = wf.progress_at_x(wf.view, plot, x).expect("click inside the plot resolves");
                let back = wf.playhead_screen_x(wf.view, plot, progress).expect("progress resolves back");
                assert!((back - x).abs() <= tolerance, "zoom {zoom} offset {offset}: clicked {x}, drawn at {back}");
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
            let (scale_x, _, translate_x, origin_x) = view.content_transform(width, SAMPLES);
            for step in 0..=10 {
                let x = width * (step as f32 / 10.0);
                let drawn = (x - origin_x) * scale_x + origin_x + translate_x;
                let mapped = wf.map_content_x(view, width, x, false);
                assert!((mapped - drawn).abs() < 1e-3, "{offset}/{overscroll}: {x} drawn at {drawn}, mapped {mapped}");
                let back = wf.map_content_x(view, width, mapped, true);
                assert!((back - x).abs() < 1e-2, "{offset}/{overscroll}: {x} round-tripped to {back}");
            }
        }
    }

    #[test]
    fn cache_key_includes_plot_width() {
        let wf = waveform(10_000, 1.0, 0.0, 0.0);
        assert_ne!(wf.content_cache_key(&Theme::Dark, 400.0), wf.content_cache_key(&Theme::Dark, 800.0));
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
        assert!(x < 0.0, "playhead for a sample before the window should be left of the plot, got {x}");
    }
}
