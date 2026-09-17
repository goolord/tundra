//! Zoom and pan state for the waveform, independent of drawing.
//!
//! `offset` is where the visible window starts, as a fraction of the track.
//! `overscroll` is the rubber-band stretch when panning past either end; it
//! springs back to zero.

use iced::keyboard::{key::Named, Key};
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
pub(super) fn max_zoom(sample_count: usize) -> f32 {
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
    let max_start = sample_count.saturating_sub(visible_samples(sample_count, zoom));
    if max_start == 0 { 0.0 } else { max_start as f64 / sample_count as f64 }
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
        Self {
            zoom: MIN_ZOOM,
            offset: 0.0,
            overscroll: 0.0,
        }
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

    /// Pans by `delta` visible widths.
    pub fn pan(&mut self, delta: f32, sample_count: usize) {
        self.apply_pan_delta(f64::from(delta) * visible_fraction_of(sample_count, self.zoom), sample_count);
    }

    /// Moves `offset` by `offset_delta`, stretching into overscroll past either end.
    pub fn apply_pan_delta(&mut self, offset_delta: f64, sample_count: usize) {
        let visible = visible_fraction_of(sample_count, self.zoom).max(1e-12);
        let max = max_left(sample_count, self.zoom);
        let edge_pull = (offset_delta / visible) as f32 * EDGE_RUBBER_BAND;

        if self.overscroll > OVERSCROLL_STOP {
            self.offset = max;
            self.overscroll = rubber_band(self.overscroll, edge_pull);
            if self.overscroll <= OVERSCROLL_STOP {
                self.overscroll = 0.0;
                self.offset = (max + offset_delta).clamp(0.0, max);
            }
            return;
        }
        if self.overscroll < -OVERSCROLL_STOP {
            self.offset = 0.0;
            self.overscroll = rubber_band(self.overscroll, edge_pull);
            if self.overscroll >= -OVERSCROLL_STOP {
                self.overscroll = 0.0;
                self.offset = offset_delta.clamp(0.0, max);
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
        if self.overscroll.abs() <= OVERSCROLL_STOP {
            self.overscroll = 0.0;
            return false;
        }
        true
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
        let old_visible = visible_samples(sample_count, self.zoom);
        let (start, _, phase) = self.sample_window(sample_count);
        let anchor_sample = start as f64 + f64::from(phase) + anchor_x * old_visible as f64;

        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, max_zoom(sample_count));

        let new_visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(new_visible) as f64;
        let new_start = (anchor_sample - anchor_x * new_visible as f64).clamp(0.0, max_start);
        self.offset = (new_start / sample_count as f64).clamp(0.0, max_left(sample_count, self.zoom));
        self.overscroll = 0.0;
    }

    /// Visible sample window: `(start, end, sub-sample phase)`.
    pub(super) fn sample_window(&self, sample_count: usize) -> (usize, usize, f32) {
        let visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(visible);
        if max_start == 0 {
            return (0, visible, 0.0);
        }
        let raw_start = (self.offset * sample_count as f64).clamp(0.0, max_start as f64);
        let start = raw_start.floor() as usize;
        let phase = (raw_start - start as f64) as f32;
        let start = start.min(max_start);
        (start, (start + visible).min(sample_count), phase)
    }

    /// Changes whenever the drawn window moves by more than an eighth of a sample.
    pub(super) fn window_cache_key(&self, sample_count: usize) -> (u32, u32, u32) {
        let (start, _, phase) = self.sample_window(sample_count);
        (start as u32, self.zoom.to_bits(), (phase * 8.0).round() as u32)
    }

    /// Horizontal and vertical stretch while rubber-banding.
    pub(super) fn content_scale(&self) -> (f32, f32) {
        let stretch = self.overscroll.clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL).abs();
        (1.0 + stretch * 1.35, 1.0 - stretch * 0.12)
    }

    pub(super) fn content_translate_x(&self, width: f32) -> f32 {
        let overscroll = self.overscroll.clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL);
        // Pan step is `-dx`, so visual shift is opposite the overscroll sign.
        -overscroll * width * 0.55
    }

    /// Scale anchor for overscroll bounce: pin the visible edge so rubber-band
    /// stretch does not clip the waveform against the plot boundary.
    pub(super) fn content_transform_origin_x(&self, width: f32, sample_count: usize) -> f32 {
        if !self.overscroll_active() {
            return width / 2.0;
        }
        let max = max_left(sample_count, self.zoom);
        if self.offset <= f64::EPSILON && self.overscroll < 0.0 {
            0.0
        } else if max > f64::EPSILON && self.offset + f64::EPSILON >= max && self.overscroll > 0.0 {
            width
        } else {
            width / 2.0
        }
    }

    pub(super) fn waveform_layout(&self, width: f32, visible_count: usize) -> WaveformLayout {
        let samples_per_col = if visible_count == 0 {
            1
        } else {
            let columns = if width <= 0.0 { 1 } else { (width.ceil() as usize).clamp(1, visible_count) };
            visible_count.div_ceil(columns).next_power_of_two()
        };
        let column_count = visible_count.div_ceil(samples_per_col);
        let per = |count: usize| if count > 0 { width / count as f32 } else { width };
        WaveformLayout {
            width,
            samples_per_col,
            column_count,
            column_width: per(column_count),
            px_per_sample: per(visible_count),
            sample_point_mode: samples_per_col == 1,
        }
    }

    pub(super) fn sample_point_mode(&self, width: f32, visible_samples: usize) -> bool {
        visible_samples > 0 && width > 0.0 && self.waveform_layout(width, visible_samples).sample_point_mode
    }

    /// Keyboard zoom (`+`/`-`) and pan (arrows). True if the key did something.
    pub fn apply_key(&mut self, key: &Key, sample_count: usize) -> bool {
        match key.as_ref() {
            Key::Character("+" | "=") => self.zoom_in(sample_count),
            Key::Character("-") => self.zoom_out(sample_count),
            Key::Named(Named::ArrowLeft) => self.pan(-PAN_STEP, sample_count),
            Key::Named(Named::ArrowRight) => self.pan(PAN_STEP, sample_count),
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

    #[test]
    fn direction_change_does_not_zoom_same_way() {
        let mut view = WaveFormView::default();
        let mut pending = 0.0;
        let samples = 10_000;

        view.accumulate_wheel(0.75, 0.5, samples, &mut pending);
        let zoom_before = view.zoom;
        assert!(pending > 0.0, "expected leftover down-scroll pending, got {pending}");

        view.accumulate_wheel(-0.05, 0.5, samples, &mut pending);
        assert!(
            view.zoom <= zoom_before,
            "reverse scroll must not zoom further in (before={zoom_before}, after={})",
            view.zoom
        );
    }

    #[test]
    fn wheel_zoom_keeps_sample_under_cursor() {
        let mut view = WaveFormView {
            zoom: 8.0,
            offset: 0.2,
            overscroll: 0.0,
        };
        let samples = 100_000;
        let anchor_x = 0.8;
        let sample_under = |view: &WaveFormView| {
            let (start, end, phase) = view.sample_window(samples);
            start as f64 + f64::from(phase) + f64::from(anchor_x) * (end - start) as f64
        };
        let before = sample_under(&view);
        view.apply_zoom_at(2.0, anchor_x, samples);
        let after = sample_under(&view);
        assert!((after - before).abs() < 2.0, "cursor sample moved: before={before} after={after}");
    }

    #[test]
    fn pan_to_max_shows_last_sample() {
        let mut view = WaveFormView {
            zoom: 4.0,
            ..Default::default()
        };
        view.offset = max_left(100_000, view.zoom);
        let (_, end, _) = view.sample_window(100_000);
        assert_eq!(end, 100_000);
    }

    #[test]
    fn panning_past_either_end_rubber_bands_then_springs_back() {
        let samples = 100_000;
        let mut view = WaveFormView {
            zoom: 4.0,
            ..Default::default()
        };
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
    fn playhead_and_first_sample_start_at_plot_origin() {
        let view = WaveFormView::default();
        let (start, end, phase) = view.sample_window(48_000);
        assert_eq!((start, phase), (0, 0.0));
        let layout = view.waveform_layout(800.0, end - start);
        let first_col_left = 0.5 * layout.column_width - phase * layout.px_per_sample - layout.column_width * 0.5;
        assert!(first_col_left >= -f32::EPSILON, "first column envelope should reach the left plot edge");
    }

    #[test]
    fn scroll_down_then_up_changes_zoom_direction() {
        let mut view = WaveFormView::default();
        let mut pending = 0.0;
        let samples = 10_000;

        for _ in 0..5 {
            view.accumulate_wheel(0.15, 0.5, samples, &mut pending);
        }
        let zoom_in = view.zoom;
        assert!(zoom_in > 1.0);

        for _ in 0..5 {
            view.accumulate_wheel(-0.15, 0.5, samples, &mut pending);
        }
        assert!(
            view.zoom < zoom_in,
            "scroll up after scroll down must zoom out (in={zoom_in}, out={})",
            view.zoom
        );
    }

    #[test]
    fn sample_point_mode_when_one_sample_per_column() {
        let view = WaveFormView {
            zoom: 4096.0,
            ..Default::default()
        };
        let layout = view.waveform_layout(900.0, 400);
        assert_eq!(layout.samples_per_col, 1);
        assert!(layout.px_per_sample >= 1.0);
        assert!(layout.sample_point_mode);
        // No cap on how many samples can be drawn individually.
        assert!(view.waveform_layout(5000.0, 5000).sample_point_mode);
    }

    #[test]
    fn zoom_can_reach_sample_level_on_long_files() {
        let mut view = WaveFormView::default();
        let sample_count = 8_640_000;
        for _ in 0..80 {
            view.zoom_in(sample_count);
        }
        assert_eq!(view.zoom, max_zoom(sample_count));
        let (start, end, _) = view.sample_window(sample_count);
        assert_eq!(end - start, 1);
        assert!(view.waveform_layout(800.0, 1).sample_point_mode);
    }

    #[test]
    fn column_count_stays_at_or_below_width() {
        let view = WaveFormView {
            zoom: 28.0,
            ..Default::default()
        };
        let layout = view.waveform_layout(900.0, 132_300);
        assert!(layout.column_count <= 900, "got {}", layout.column_count);
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
