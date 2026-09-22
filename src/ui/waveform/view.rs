//! Zoom and pan state for the waveform, independent of drawing.
//!
//! `offset` is where the visible window starts, as a fraction of the track.
//! `overscroll` is the rubber-band stretch when panning past either end; it
//! springs back to zero.

use iced::keyboard::{Key, key::Named};
use iced::mouse::ScrollDelta;

const MIN_ZOOM: f32 = 1.0;
const ZOOM_FACTOR: f32 = 1.25;
pub(super) const PAN_STEP: f32 = 0.08;
const MAX_OVERSCROLL: f32 = 0.14;
const OVERSCROLL_SPRING: f32 = 0.78;
const OVERSCROLL_STOP: f32 = 0.002;
const WHEEL_ZOOM_TAIL: f32 = 0.01;
const WHEEL_ZOOM_MAX: f32 = 0.6;
const WHEEL_SCROLL_PIXELS_PER_LINE: f32 = 28.0;
const EDGE_RUBBER_BAND: f32 = 0.35;

/// Zooming stops at one sample per plot width.
fn max_zoom(sample_count: usize) -> f32 {
    (sample_count as f32).max(MIN_ZOOM)
}

fn visible_samples(sample_count: usize, zoom: f32) -> usize {
    if sample_count == 0 {
        return 0;
    }
    let zoom = f64::from(zoom.max(MIN_ZOOM));
    (sample_count as f64 / zoom).ceil().clamp(1.0, sample_count as f64) as usize
}

/// The largest `offset`: the window's start when it shows the track's end.
fn max_left(sample_count: usize, zoom: f32) -> f64 {
    match sample_count.saturating_sub(visible_samples(sample_count, zoom)) {
        0 => 0.0,
        max_start => max_start as f64 / sample_count as f64,
    }
}

pub(super) fn visible_fraction_of(sample_count: usize, zoom: f32) -> f64 {
    if sample_count == 0 {
        return 1.0;
    }
    visible_samples(sample_count, zoom) as f64 / sample_count as f64
}

/// Wheel movement in lines, `(x, y)`, whatever unit the device reports.
pub(super) fn scroll_lines(delta: ScrollDelta) -> (f32, f32) {
    match delta {
        ScrollDelta::Lines { x, y } => (x, y),
        ScrollDelta::Pixels { x, y } => (x / WHEEL_SCROLL_PIXELS_PER_LINE, y / WHEEL_SCROLL_PIXELS_PER_LINE),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveFormView {
    pub zoom: f32,
    pub offset: f64,
    pub overscroll: f32,
}

impl Default for WaveFormView {
    fn default() -> Self {
        Self { zoom: MIN_ZOOM, offset: 0.0, overscroll: 0.0 }
    }
}

/// How the visible samples map onto plot columns.
#[derive(Clone, Copy)]
pub(super) struct WaveformLayout {
    pub width: f32,
    /// A power of two, so the column grid stays put while panning.
    pub samples_per_col: usize,
    pub column_count: usize,
    pub column_width: f32,
    pub px_per_sample: f32,
    /// Zoomed in far enough to draw individual samples instead of an envelope.
    pub sample_point_mode: bool,
}

impl WaveformLayout {
    pub(super) fn new(width: f32, visible_count: usize) -> Self {
        let columns = if width <= 0.0 { 1 } else { (width.ceil() as usize).clamp(1, visible_count.max(1)) };
        let samples_per_col = visible_count.div_ceil(columns).next_power_of_two();
        let column_count = visible_count.div_ceil(samples_per_col);
        let per = |count: usize| if count > 0 { width / count as f32 } else { width };
        Self {
            width,
            samples_per_col,
            column_count,
            column_width: per(column_count),
            px_per_sample: per(visible_count),
            sample_point_mode: samples_per_col == 1,
        }
    }
}

impl WaveFormView {
    pub fn zoom_in(&mut self, sample_count: usize) {
        self.apply_zoom_at(ZOOM_FACTOR, 0.5, sample_count);
    }

    pub fn zoom_out(&mut self, sample_count: usize) {
        self.apply_zoom_at(1.0 / ZOOM_FACTOR, 0.5, sample_count);
    }

    /// Adds wheel `lines` to `pending` and zooms by what has built up, around
    /// `anchor_x` (0 to 1 across the plot). True if the zoom changed.
    pub fn accumulate_wheel(&mut self, lines: f32, anchor_x: f32, sample_count: usize, pending: &mut f32) -> bool {
        if lines != 0.0 && pending.signum() != 0.0 && pending.signum() != lines.signum() {
            *pending = 0.0;
        }
        *pending += lines;
        if pending.abs() < WHEEL_ZOOM_TAIL {
            return false;
        }
        let step = pending.clamp(-WHEEL_ZOOM_MAX, WHEEL_ZOOM_MAX);
        self.apply_zoom_at(ZOOM_FACTOR.powf(step), anchor_x, sample_count);
        *pending -= step;
        true
    }

    /// Moves `offset` by `offset_delta`, stretching into overscroll past either end.
    pub fn apply_pan_delta(&mut self, offset_delta: f64, sample_count: usize) {
        let visible = visible_fraction_of(sample_count, self.zoom).max(1e-12);
        let max = max_left(sample_count, self.zoom);
        let edge_pull = (offset_delta / visible) as f32 * EDGE_RUBBER_BAND;

        if self.overscroll_active() {
            // Already stretched past an edge: pull the band, and pan normally once it lets go.
            let side = self.overscroll.signum();
            let edge = if side > 0.0 { max } else { 0.0 };
            self.offset = edge;
            self.overscroll = rubber_band(self.overscroll, edge_pull);
            if self.overscroll * side <= OVERSCROLL_STOP {
                self.overscroll = 0.0;
                self.offset = (edge + offset_delta).clamp(0.0, max);
            }
            return;
        }

        self.overscroll = 0.0;
        let target = self.offset + offset_delta;
        self.offset = target.clamp(0.0, max);
        let overflow = (target - self.offset) / visible;
        if overflow != 0.0 {
            self.overscroll = rubber_band(0.0, overflow as f32 * EDGE_RUBBER_BAND);
        }
    }

    /// One spring step. Settles to exactly zero: a residue below the stop
    /// threshold would end the spring ticks yet keep the uncached draw path on.
    pub fn spring_overscroll(&mut self) -> bool {
        self.overscroll *= OVERSCROLL_SPRING;
        if !self.overscroll_active() {
            self.overscroll = 0.0;
        }
        self.overscroll != 0.0
    }

    /// Whether the rubber-band stretch is visible. A residue at or below the stop
    /// threshold (a slow pan past the edge) is visually nothing and must not keep
    /// the uncached draw path on.
    pub fn overscroll_active(&self) -> bool {
        self.overscroll.abs() > OVERSCROLL_STOP
    }

    pub(super) fn apply_zoom_at(&mut self, factor: f32, anchor_x: f32, sample_count: usize) {
        if sample_count == 0 {
            return;
        }
        let anchor_x = f64::from(anchor_x).clamp(0.0, 1.0);
        let (start, old_visible, phase) = self.sample_window(sample_count);
        let anchor_sample = start as f64 + f64::from(phase) + anchor_x * old_visible as f64;

        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, max_zoom(sample_count));

        let new_visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(new_visible) as f64;
        let new_start = (anchor_sample - anchor_x * new_visible as f64).clamp(0.0, max_start);
        self.offset = (new_start / sample_count as f64).clamp(0.0, max_left(sample_count, self.zoom));
        self.overscroll = 0.0;
    }

    /// Visible sample window: `(start, visible count, sub-sample phase)`.
    pub(super) fn sample_window(&self, sample_count: usize) -> (usize, usize, f32) {
        let visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(visible);
        if max_start == 0 {
            return (0, visible, 0.0);
        }
        let raw_start = (self.offset * sample_count as f64).clamp(0.0, max_start as f64);
        let start = raw_start.floor() as usize;
        (start.min(max_start), visible, (raw_start - start as f64) as f32)
    }

    /// Changes whenever the drawn window moves by more than an eighth of a sample.
    pub(super) fn window_cache_key(&self, sample_count: usize) -> (u32, u32, u32) {
        let (start, _, phase) = self.sample_window(sample_count);
        (start as u32, self.zoom.to_bits(), (phase * 8.0).round() as u32)
    }

    /// The rubber-band stretch as drawn: `(x scale, y scale, x translation, x origin)`. The
    /// origin pins the visible edge so the stretch does not clip the waveform against the plot
    /// boundary; the playhead and click-to-seek mapping have to use this same transform.
    pub(super) fn content_transform(&self, width: f32, sample_count: usize) -> (f32, f32, f32, f32) {
        let overscroll = self.overscroll.clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL);
        let max = max_left(sample_count, self.zoom);
        let origin_x = if !self.overscroll_active() {
            width / 2.0
        } else if self.offset <= f64::EPSILON && self.overscroll < 0.0 {
            0.0
        } else if max > f64::EPSILON && self.offset + f64::EPSILON >= max && self.overscroll > 0.0 {
            width
        } else {
            width / 2.0
        };
        // Pan step is `-dx`, so the visual shift is opposite the overscroll sign.
        (1.0 + overscroll.abs() * 1.35, 1.0 - overscroll.abs() * 0.12, -overscroll * width * 0.55, origin_x)
    }

    /// Keyboard zoom (`+`/`-`) and pan (arrows). True if the key did something.
    pub fn apply_key(&mut self, key: &Key, sample_count: usize) -> bool {
        let pan = |view: &mut Self, delta: f32| {
            let delta = f64::from(delta) * visible_fraction_of(sample_count, view.zoom);
            view.apply_pan_delta(delta, sample_count);
        };
        match key.as_ref() {
            Key::Character("+" | "=") => self.zoom_in(sample_count),
            Key::Character("-") => self.zoom_out(sample_count),
            Key::Named(Named::ArrowLeft) => pan(self, -PAN_STEP),
            Key::Named(Named::ArrowRight) => pan(self, PAN_STEP),
            _ => return false,
        }
        true
    }
}

fn rubber_band(current: f32, additional: f32) -> f32 {
    let resistance = 1.0 + (current.abs() / MAX_OVERSCROLL) * 2.5;
    (current + additional / resistance).clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(zoom: f32, offset: f64) -> WaveFormView {
        WaveFormView { zoom, offset, overscroll: 0.0 }
    }

    #[test]
    fn wheel_zoom_follows_scroll_direction() {
        let samples = 10_000;
        let (mut view, mut pending) = (WaveFormView::default(), 0.0);
        view.accumulate_wheel(0.75, 0.5, samples, &mut pending);
        let zoom_before = view.zoom;
        assert!(pending > 0.0, "expected leftover down-scroll pending, got {pending}");
        view.accumulate_wheel(-0.05, 0.5, samples, &mut pending);
        assert!(view.zoom <= zoom_before, "reverse scroll must not zoom further in");

        let (mut view, mut pending) = (WaveFormView::default(), 0.0);
        for lines in [0.15; 5] {
            view.accumulate_wheel(lines, 0.5, samples, &mut pending);
        }
        let zoom_in = view.zoom;
        assert!(zoom_in > 1.0);
        for lines in [-0.15; 5] {
            view.accumulate_wheel(lines, 0.5, samples, &mut pending);
        }
        assert!(view.zoom < zoom_in, "scroll up after scroll down must zoom out");
    }

    #[test]
    fn wheel_zoom_keeps_sample_under_cursor() {
        let (samples, anchor_x) = (100_000, 0.8);
        let sample_under = |view: &WaveFormView| {
            let (start, visible, phase) = view.sample_window(samples);
            start as f64 + f64::from(phase) + f64::from(anchor_x) * visible as f64
        };
        let mut view = view(8.0, 0.2);
        let before = sample_under(&view);
        view.apply_zoom_at(2.0, anchor_x, samples);
        let after = sample_under(&view);
        assert!((after - before).abs() < 2.0, "cursor sample moved: before={before} after={after}");
    }

    #[test]
    fn panning_past_either_end_rubber_bands_then_springs_back() {
        let samples = 100_000;
        let mut view = view(4.0, max_left(samples, 4.0));
        let (start, visible, _) = view.sample_window(samples);
        assert_eq!(start + visible, samples, "panned to the end shows the last sample");

        view.offset = 0.0;
        view.apply_pan_delta(-0.1, samples);
        assert_eq!(view.offset, 0.0);
        assert!(view.overscroll < 0.0 && view.overscroll_active());
        while view.spring_overscroll() {}
        assert_eq!(view.overscroll, 0.0);

        view.apply_pan_delta(2.0, samples);
        assert_eq!(view.offset, max_left(samples, view.zoom));
        assert!(view.overscroll > 0.0);
    }

    #[test]
    fn layout_columns_and_sample_point_mode() {
        let (start, visible, phase) = WaveFormView::default().sample_window(48_000);
        assert_eq!((start, phase), (0, 0.0));
        let layout = WaveformLayout::new(800.0, visible);
        let first_col_left = 0.5 * layout.column_width - phase * layout.px_per_sample - layout.column_width * 0.5;
        assert!(first_col_left >= -f32::EPSILON, "first column envelope should reach the left plot edge");

        assert!(WaveformLayout::new(900.0, 132_300).column_count <= 900);
        let layout = WaveformLayout::new(900.0, 400);
        assert_eq!(layout.samples_per_col, 1);
        assert!(layout.px_per_sample >= 1.0 && layout.sample_point_mode);
        // No cap on how many samples can be drawn individually.
        assert!(WaveformLayout::new(5000.0, 5000).sample_point_mode);
    }

    #[test]
    fn zoom_can_reach_sample_level_on_long_files() {
        let mut view = WaveFormView::default();
        let sample_count = 8_640_000;
        for _ in 0..80 {
            view.zoom_in(sample_count);
        }
        assert_eq!(view.zoom, max_zoom(sample_count));
        assert_eq!(view.sample_window(sample_count).1, 1);
        assert!(WaveformLayout::new(800.0, 1).sample_point_mode);
    }

    #[test]
    fn arrow_keys_pan_and_plus_minus_zoom() {
        let samples = 100_000;
        let mut view = WaveFormView::default();
        assert!(view.apply_key(&Key::Character("+".into()), samples));
        assert!(view.zoom > 1.0);
        assert!(view.apply_key(&Key::Named(Named::ArrowRight), samples));
        assert!(view.offset > 0.0);
        assert!(!view.apply_key(&Key::Character("q".into()), samples));
    }
}
