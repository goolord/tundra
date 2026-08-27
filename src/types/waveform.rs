pub use super::common::*;

use iced::keyboard::Key;
use iced::mouse::{self, Cursor, ScrollDelta};
use iced::widget::canvas::Action;
use iced::widget::canvas::*;
use iced::border::Radius;
use iced::keyboard::Modifiers;
use iced::alignment;
use iced::widget::canvas::Text;
use iced::{Color, Pixels, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use crate::source::arc_samples::PlaybackPosition;
use crate::waveform_peaks::WaveformPeaks;

const MIN_ZOOM: f32 = 1.0;
const ZOOM_FACTOR: f32 = 1.25;
const TWO_OUTLINE_ENTER: f32 = 0.65;
const TWO_OUTLINE_EXIT: f32 = 0.40;
const PAN_STEP: f32 = 0.08;
const TIME_MARKER_HEIGHT: f32 = 16.0;
const MIN_TICK_GAP_PX: f32 = 8.0;
const AMPLITUDE_GUTTER: f32 = 36.0;
const PLOT_CLIP_BLEED_LEFT: f32 = 2.0;
const AMPLITUDE_PAD_TOP: f32 = 11.0;
const AMPLITUDE_TICKS: [f32; 5] = [1.0, 0.5, 0.0, -0.5, -1.0];
const MAX_OVERSCROLL: f32 = 0.14;
const OVERSCROLL_SPRING: f32 = 0.78;
const OVERSCROLL_STOP: f32 = 0.002;
const FILE_DRAG_THRESHOLD: f32 = 8.0;
const WHEEL_ZOOM_TAIL: f32 = 0.01;
const WHEEL_ZOOM_MAX: f32 = 0.6;
const WHEEL_SCROLL_PIXELS_PER_LINE: f32 = 28.0;
const EDGE_RUBBER_BAND: f32 = 0.35;
const WAVEFORM_CORNER_RADIUS: f32 = 8.0;
const LANCZOS_A: i32 = 3; // kernel half-width

fn max_zoom(sample_count: usize) -> f32 {
    (sample_count as f64).max(MIN_ZOOM as f64) as f32
}

fn visible_samples(sample_count: usize, zoom: f32) -> usize {
    if sample_count == 0 {
        return 0;
    }
    let zoom = (zoom as f64).max(MIN_ZOOM as f64);
    ((sample_count as f64) / zoom)
        .ceil()
        .clamp(1.0, sample_count as f64) as usize
}

fn max_left(sample_count: usize, zoom: f32) -> f64 {
    if sample_count == 0 {
        return 0.0;
    }
    let visible = visible_samples(sample_count, zoom);
    let max_start = sample_count.saturating_sub(visible);
    if max_start == 0 {
        return 0.0;
    }
    max_start as f64 / sample_count as f64
}

fn visible_fraction_of(sample_count: usize, zoom: f32) -> f64 {
    if sample_count == 0 {
        return 1.0;
    }
    visible_samples(sample_count, zoom) as f64 / sample_count as f64
}

fn theme_cache_key(theme: &Theme) -> u32 {
    let palette = theme.extended_palette();
    let primary = palette.primary.base.color;
    let background = palette.background.base.color;
    primary.r.to_bits()
        ^ primary.g.to_bits()
        ^ primary.b.to_bits()
        ^ background.r.to_bits()
        ^ background.g.to_bits()
        ^ background.b.to_bits()
}

fn scroll_lines(delta: ScrollDelta) -> (f32, f32) {
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
            zoom: 1.0,
            offset: 0.0,
            overscroll: 0.0,
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

    pub fn wheel_lines(delta: ScrollDelta) -> f32 {
        scroll_lines(delta).1
    }

    pub fn accumulate_wheel(
        &mut self,
        lines: f32,
        anchor_x: f32,
        sample_count: usize,
        pending: &mut f32,
    ) -> bool {
        if lines != 0.0 {
            let pending_sign = pending.signum();
            if pending_sign != 0.0 && pending_sign != lines.signum() {
                *pending = 0.0;
            }
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

    pub fn pan(&mut self, delta: f32, sample_count: usize) {
        self.apply_pan_delta(
            delta as f64 * visible_fraction_of(sample_count, self.zoom),
            sample_count,
        );
    }

    pub fn apply_pan_delta(&mut self, offset_delta: f64, sample_count: usize) {
        let visible = visible_fraction_of(sample_count, self.zoom).max(1e-12);
        let max = max_left(sample_count, self.zoom);
        let edge_pull = (offset_delta / visible) as f32 * EDGE_RUBBER_BAND;

        if self.overscroll > OVERSCROLL_STOP {
            self.offset = max;
            self.overscroll = Self::rubber_band(self.overscroll, edge_pull);
            if self.overscroll <= OVERSCROLL_STOP {
                self.overscroll = 0.0;
                self.offset = (max + offset_delta).clamp(0.0, max);
            }
            return;
        }

        if self.overscroll < -OVERSCROLL_STOP {
            self.offset = 0.0;
            self.overscroll = Self::rubber_band(self.overscroll, edge_pull);
            if self.overscroll >= -OVERSCROLL_STOP {
                self.overscroll = 0.0;
                self.offset = offset_delta.clamp(0.0, max);
            }
            return;
        }

        self.overscroll = 0.0;
        let target = self.offset + offset_delta;
        if target < 0.0 {
            self.offset = 0.0;
            let overflow = -target / visible;
            self.overscroll = Self::rubber_band(0.0, -overflow as f32 * EDGE_RUBBER_BAND);
        } else if target > max {
            self.offset = max;
            let overflow = (target - max) / visible;
            self.overscroll = Self::rubber_band(0.0, overflow as f32 * EDGE_RUBBER_BAND);
        } else {
            self.offset = target;
        }
    }

    pub fn spring_overscroll(&mut self) -> bool {
        if self.overscroll.abs() < OVERSCROLL_STOP {
            self.overscroll = 0.0;
            return false;
        }
        self.overscroll *= OVERSCROLL_SPRING;
        true
    }

    pub fn overscroll_active(&self) -> bool {
        self.overscroll.abs() > OVERSCROLL_STOP
    }

    fn rubber_band(current: f32, additional: f32) -> f32 {
        let resistance = 1.0 + (current.abs() / MAX_OVERSCROLL) * 2.5;
        (current + additional / resistance).clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL)
    }

    fn set_timeline_left(&mut self, left: f64, sample_count: usize) {
        if sample_count == 0 {
            self.offset = 0.0;
            return;
        }
        if max_left(sample_count, self.zoom) == 0.0 {
            self.offset = 0.0;
            return;
        }
        self.offset = left.clamp(0.0, max_left(sample_count, self.zoom));
    }

    fn apply_zoom_at(&mut self, factor: f32, anchor_x: f32, sample_count: usize) {
        if sample_count == 0 {
            return;
        }
        let anchor_x = (anchor_x as f64).clamp(0.0, 1.0);
        let old_visible = visible_samples(sample_count, self.zoom);
        let (start, _, phase) = self.sample_window(sample_count);
        let anchor_sample = start as f64 + phase as f64 + anchor_x * old_visible as f64;

        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, max_zoom(sample_count));

        let new_visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(new_visible) as f64;
        let new_start = (anchor_sample - anchor_x * new_visible as f64).clamp(0.0, max_start);
        self.set_timeline_left(new_start / sample_count as f64, sample_count);
        self.overscroll = 0.0;
    }

    #[cfg(test)]
    pub(crate) fn max_offset(&self, sample_count: usize) -> f64 {
        max_left(sample_count, self.zoom)
    }

    /// Visible sample window: `(start, end, sub-sample phase)`.
    fn sample_window(&self, sample_count: usize) -> (usize, usize, f32) {
        if sample_count == 0 {
            return (0, 0, 0.0);
        }

        let visible = visible_samples(sample_count, self.zoom);
        let max_start = sample_count.saturating_sub(visible);
        if max_start == 0 {
            return (0, visible.min(sample_count), 0.0);
        }

        let raw_start = (self.offset as f64 * sample_count as f64).clamp(0.0, max_start as f64);
        let start = raw_start.floor() as usize;
        let phase = (raw_start - start as f64) as f32;
        let start = start.min(max_start);
        (start, (start + visible).min(sample_count), phase)
    }

    fn view_cache_key(&self, sample_count: usize) -> (u32, u32, u32) {
        let (start, _, phase) = self.sample_window(sample_count);
        let zoom_q = self.zoom.to_bits();
        let phase_q = (phase * 8.0).round() as u32;
        (start as u32, zoom_q, phase_q)
    }

    fn draw_cache_key(
        &self,
        sample_count: usize,
        plot_width: f32,
        theme: &Theme,
    ) -> (u32, u32, u32, u32, u32) {
        let (start, zoom_q, phase_q) = self.view_cache_key(sample_count);
        let width_q = plot_width.round() as u32;
        (start, zoom_q, phase_q, width_q, theme_cache_key(theme))
    }

    fn content_scale(&self) -> (f32, f32) {
        let overscroll = self.overscroll.clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL);
        (
            1.0 + overscroll.abs() * 1.35,
            1.0 - overscroll.abs() * 0.12,
        )
    }

    fn content_translate_x(&self, width: f32) -> f32 {
        let overscroll = self.overscroll.clamp(-MAX_OVERSCROLL, MAX_OVERSCROLL);
        // Pan step is `-dx`, so visual shift is opposite the overscroll sign.
        -overscroll * width * 0.55
    }

    fn content_transform_active(&self) -> bool {
        self.overscroll != 0.0
    }

    /// Scale anchor for overscroll bounce: pin the visible edge so rubber-band
    /// stretch does not clip the waveform against the plot boundary.
    fn content_transform_origin_x(&self, width: f32, sample_count: usize) -> f32 {
        if self.overscroll.abs() <= OVERSCROLL_STOP {
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

    fn column_count(&self, width: f32, visible_samples: usize) -> usize {
        if visible_samples == 0 || width <= 0.0 {
            return 1;
        }

        let max_columns = width.ceil() as usize;
        max_columns.min(visible_samples).max(1)
    }

    fn samples_per_column(&self, width: f32, visible_samples: usize) -> usize {
        if visible_samples == 0 {
            return 1;
        }
        let columns = self.column_count(width, visible_samples);
        visible_samples.div_ceil(columns).next_power_of_two().max(1)
    }

    fn waveform_layout(&self, width: f32, visible_count: usize) -> WaveformLayout {
        let samples_per_col = self.samples_per_column(width, visible_count);
        let column_count = visible_count.div_ceil(samples_per_col);
        let column_width = if column_count > 0 {
            width / column_count as f32
        } else {
            width
        };
        let px_per_sample = if visible_count > 0 {
            width / visible_count as f32
        } else {
            width
        };
        WaveformLayout {
            width,
            samples_per_col,
            column_count,
            column_width,
            px_per_sample,
            sample_point_mode: samples_per_col == 1,
        }
    }

    fn sample_point_mode(&self, width: f32, visible_samples: usize) -> bool {
        if visible_samples == 0 || width <= 0.0 {
            return false;
        }
        self.waveform_layout(width, visible_samples).sample_point_mode
    }

    pub fn apply_key(view: &mut Self, key: &Key, sample_count: usize) -> bool {
        match key.as_ref() {
            Key::Character("+") | Key::Character("=") => {
                view.zoom_in(sample_count);
                true
            }
            Key::Character("-") => {
                view.zoom_out(sample_count);
                true
            }
            Key::Named(iced::keyboard::key::Named::ArrowLeft) => {
                view.pan(-PAN_STEP, sample_count);
                true
            }
            Key::Named(iced::keyboard::key::Named::ArrowRight) => {
                view.pan(PAN_STEP, sample_count);
                true
            }
            _ => false,
        }
    }
}

#[derive(Default)]
pub struct WaveFormState {
    pan_anchor: Option<PanAnchor>,
    last_pan_view: Option<WaveFormView>,
    last_pan_x: Option<f32>,
    scrub_active: bool,
    last_scrub_progress: f64,
    file_drag_armed: bool,
    file_drag_origin: Option<Point>,
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
    scrub_progress: Option<f64>,
    ui_scrubbing: Cell<bool>,
    modifiers: Modifiers,
    pan_active: bool,
    cache: Cache,
    content_cache_key: Cell<(u32, u32, u32, u32, u32)>,
    two_outline_armed: Cell<bool>,
}

#[derive(Clone, Copy)]
struct PlotArea {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl PlotArea {
    fn from_size(size: Size) -> Self {
        let x = AMPLITUDE_GUTTER;
        let y = AMPLITUDE_PAD_TOP;
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

struct ColumnSample {
    x: f32,
    stroke_y_min: f32,
    stroke_y_max: f32,
}

#[derive(Clone, Copy)]
struct WaveformLayout {
    width: f32,
    samples_per_col: usize,
    column_count: usize,
    column_width: f32,
    px_per_sample: f32,
    sample_point_mode: bool,
}

struct WaveformPalette {
    background: Color,
    axis: Color,
    fill: Color,
    stroke: Color,
    marker: Color,
    marker_label: Color,
}

#[derive(Clone, Copy)]
enum TimeMarkerTier {
    Major,
    Second,
    Subsecond,
    Sample,
}

impl WaveformPalette {
    fn from_theme(theme: &Theme) -> Self {
        let palette = theme.extended_palette();
        let primary = palette.primary.base.color;
        let base = palette.background.base.color;
        Self {
            background: Color::from_rgb(
                base.r * 0.76,
                base.g * 0.76,
                base.b * 0.78,
            ),
            axis: palette.background.strong.color.scale_alpha(0.35),
            fill: primary.scale_alpha(0.22),
            stroke: primary.scale_alpha(0.92),
            marker: palette.background.strong.color.scale_alpha(0.55),
            marker_label: palette.background.base.text.scale_alpha(0.72),
        }
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

    pub fn set_sample_count(&mut self, sample_count: usize) {
        if self.sample_count != sample_count {
            self.sample_count = sample_count;
            self.invalidate_cache();
        }
    }

    pub fn apply_peaks_ready(&mut self) -> Option<usize> {
        let count = self
            .peaks
            .lock()
            .ok()
            .filter(|peaks| peaks.complete && peaks.sample_count > 0)
            .map(|peaks| peaks.sample_count)?;
        self.set_sample_count(count);
        Some(count)
    }

    pub fn invalidate_cache(&mut self) {
        self.cache.clear();
        self.content_cache_key.set((0, 0, 0, 0, 0));
    }

    fn peaks_complete(&self) -> bool {
        self.peaks
            .lock()
            .map(|peaks| peaks.complete)
            .unwrap_or(false)
    }

    fn sync_content_cache(&self, theme: &Theme, plot_width: f32) {
        let peaks_flag = self.peaks_complete() as u32;
        let key = self
            .view
            .draw_cache_key(self.sample_count, plot_width, theme);
        let key = (key.0, key.1, key.2, key.3, key.4 ^ peaks_flag);
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

    pub fn set_playback_position(&mut self, position: Arc<PlaybackPosition>) {
        self.playback_position = Some(position);
    }

    pub fn set_scrub_progress(&mut self, progress: Option<f64>) {
        if self.scrub_progress != progress {
            self.scrub_progress = progress;
        }
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

    fn with_content_transform(
        frame: &mut Frame,
        view: WaveFormView,
        width: f32,
        height: f32,
        sample_count: usize,
        draw: impl FnOnce(&mut Frame),
    ) {
        let (scale_x, scale_y) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        let origin_x = view.content_transform_origin_x(width, sample_count);
        frame.push_transform();
        frame.translate(Vector::new(translate_x, 0.0));
        frame.translate(Vector::new(origin_x, height / 2.0));
        frame.scale_nonuniform(Vector::new(scale_x, scale_y));
        frame.translate(Vector::new(-origin_x, -height / 2.0));
        draw(frame);
        frame.pop_transform();
    }

    fn map_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, _) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        (x - width / 2.0) * scale_x + width / 2.0 + translate_x
    }

    fn unmap_content_x(&self, view: WaveFormView, width: f32, x: f32) -> f32 {
        let (scale_x, _) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        if scale_x.abs() < 1e-6 {
            return width / 2.0;
        }
        (x - translate_x - width / 2.0) / scale_x + width / 2.0
    }

    fn playback_progress(&self) -> Option<f64> {
        if let Some(progress) = self.scrub_progress {
            return Some(progress);
        }
        self.playback_position.as_ref().map(|position| position.progress())
    }

    fn playhead_content_x(&self, view: WaveFormView, plot_width: f32, progress: f64) -> Option<f32> {
        if self.sample_count == 0 || plot_width <= 0.0 {
            return None;
        }

        let frame_count = self
            .playback_position
            .as_ref()
            .map(|position| position.total_frames() as usize)
            .unwrap_or(self.sample_count);
        if frame_count == 0 {
            return None;
        }

        let progress_frame = ((progress * frame_count as f64).round() as usize)
            .min(frame_count.saturating_sub(1));
        let (start, end, phase) = view.sample_window(self.sample_count);
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let px_per_sample = plot_width / visible as f32;
        let sample_pos = progress_frame.saturating_sub(start) as f32 - phase;
        Some(sample_pos * px_per_sample)
    }

    fn playhead_screen_x(&self, view: WaveFormView, plot: PlotArea, progress: f64) -> Option<f32> {
        let x = self.playhead_content_x(view, plot.width, progress)?;
        Some(self.map_content_x(view, plot.width, x) + plot.x)
    }

    fn stroke_playhead(frame: &mut Frame, x: f32, height: f32, accent: Color) {
        let line = Path::line(Point::new(x, 0.0), Point::new(x, height));
        frame.stroke(
            &line,
            Stroke::default()
                .with_color(accent)
                .with_width(2.0)
                .with_line_cap(LineCap::Round),
        );
        frame.stroke(
            &line,
            Stroke::default()
                .with_color(accent.scale_alpha(0.35))
                .with_width(6.0)
                .with_line_cap(LineCap::Round),
        );
    }

    fn progress_at_x(&self, view: WaveFormView, plot: PlotArea, x: f32) -> Option<f64> {
        if self.sample_count == 0 || plot.width <= 0.0 {
            return None;
        }

        let frame_count = self
            .playback_position
            .as_ref()
            .map(|position| position.total_frames() as usize)
            .unwrap_or(self.sample_count);
        if frame_count == 0 {
            return None;
        }

        let (start, end, phase) = view.sample_window(self.sample_count);
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let px_per_sample = plot.width / visible as f32;
        let content_x = self.unmap_content_x(view, plot.width, x - plot.x);
        let sample_pos = (content_x / px_per_sample + phase).clamp(0.0, visible as f32);
        let progress_frame = (start as f32 + sample_pos).round() as usize;
        Some(progress_frame.min(frame_count.saturating_sub(1)) as f64 / frame_count as f64)
    }

    fn draw_playhead_on_frame(
        &self,
        frame: &mut Frame,
        theme: &Theme,
        size: Size,
        progress: f64,
        view: WaveFormView,
    ) {
        let Some(x) = self.playhead_content_x(view, size.width, progress) else {
            return;
        };
        let accent = theme.extended_palette().primary.base.color;
        Self::stroke_playhead(frame, x, size.height, accent);
    }

    fn draw_playhead(
        &self,
        renderer: &Renderer,
        size: Size,
        theme: &Theme,
        progress: f64,
        view: WaveFormView,
    ) -> Option<Geometry> {
        let plot = PlotArea::from_size(size);
        let x = self.playhead_screen_x(view, plot, progress)?;
        let accent = theme.extended_palette().primary.base.color;
        let mut frame = Frame::new(renderer, size);
        Self::stroke_playhead(&mut frame, x, size.height, accent);
        Some(frame.into_geometry())
    }

    pub fn view_state(&self) -> WaveFormView {
        self.view
    }

    pub fn set_view(&mut self, view: WaveFormView) {
        self.view = view;
    }

    #[cfg(test)]
    fn column_extents(chunk: &[f32]) -> (f32, f32) {
        let mut iter = chunk.iter().map(|sample| sample.clamp(-1.0, 1.0));
        let Some(first) = iter.next() else {
            return (0.0, 0.0);
        };
        let mut min = first;
        let mut max = first;
        for sample in iter {
            min = min.min(sample);
            max = max.max(sample);
        }
        (min, max)
    }

    fn flip_y(y: f32, center: f32) -> f32 {
        2.0 * center - y
    }

    fn sinc_pi(x: f64) -> f64 {
        if x.abs() < 1e-12 {
            1.0
        } else {
            let pix = std::f64::consts::PI * x;
            pix.sin() / pix
        }
    }

    /// `L(x) = sinc(x) sinc(x/3)` for `|x| < 3`.
    fn lanczos3(x: f64) -> f64 {
        let a = f64::from(LANCZOS_A);
        if !x.is_finite() || x.abs() >= a {
            0.0
        } else {
            Self::sinc_pi(x) * Self::sinc_pi(x / a)
        }
    }

    /// Lanczos-3 at sample index `t`; zero-extend outside `[0, samples.len())`.
    #[cfg(test)]
    fn interpolate_at(samples: &[f32], t: f64) -> f32 {
        if samples.is_empty() || !t.is_finite() {
            return 0.0;
        }
        let n = samples.len();
        let a = i64::from(LANCZOS_A);
        let t = t.clamp(-a as f64, n as f64 + a as f64);
        let center = t.floor() as i64;
        let first = (center - a + 1).max(0);
        let last = (center + a).min(n as i64 - 1);
        if first > last {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in first..=last {
            let sample = samples[i as usize] as f64;
            sum += sample * Self::lanczos3(t - i as f64);
        }
        sum as f32
    }

    /// Lanczos-3 at mono frame index `t` using peak midpoints.
    fn interpolate_peak_at(peaks: &WaveformPeaks, t: f64) -> f32 {
        if peaks.sample_count == 0 || !t.is_finite() {
            return 0.0;
        }
        let n = peaks.sample_count;
        let a = i64::from(LANCZOS_A);
        let t = t.clamp(-a as f64, n as f64 + a as f64);
        let center = t.floor() as i64;
        let first = (center - a + 1).max(0);
        let last = (center + a).min(n as i64 - 1);
        if first > last {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in first..=last {
            let sample = peaks.midpoint_at(i as usize) as f64;
            sum += sample * Self::lanczos3(t - i as f64);
        }
        sum as f32
    }

    fn column_extents_from_peaks(
        peaks: &WaveformPeaks,
        chunk_start: usize,
        chunk_end: usize,
    ) -> (f32, f32) {
        peaks.extents_for_range(chunk_start, chunk_end)
    }

    fn sample_index_at_x(x: f32, start: usize, phase: f32, px_per_sample: f32) -> f64 {
        if px_per_sample <= 0.0 || !px_per_sample.is_finite() {
            return start as f64;
        }
        start as f64 - 0.5 + f64::from(phase) + f64::from(x) / f64::from(px_per_sample)
    }

    fn mirror_fill_gradient(fill: Color, outline_y: f32, mirror_y: f32, center: f32) -> Gradient {
        let clear = Color {
            r: fill.r,
            g: fill.g,
            b: fill.b,
            a: 0.0,
        };
        let delta = mirror_y - outline_y;
        let outline_y = if delta.abs() < 1.0 {
            let dir = if delta != 0.0 {
                delta.signum()
            } else {
                let fallback = center - outline_y;
                if fallback != 0.0 {
                    fallback.signum()
                } else {
                    1.0
                }
            };
            outline_y - dir
        } else {
            outline_y
        };
        Gradient::Linear(
            gradient::Linear::new(Point::new(0.0, outline_y), Point::new(0.0, mirror_y))
                .add_stop(0.0, fill)
                .add_stop(0.22, fill)
                .add_stop(0.5, fill.scale_alpha(0.45))
                .add_stop(0.72, fill.scale_alpha(0.12))
                .add_stop(0.85, clear)
                .add_stop(1.0, clear),
        )
    }

    #[cfg(test)]
    fn detected_peak_y(columns: &[ColumnSample], center: f32, upper: bool) -> Option<f32> {
        let mut peak: Option<f32> = None;
        for column in columns {
            let y = if upper {
                if !Self::has_upper_lobe(column, center) {
                    continue;
                }
                column.stroke_y_max
            } else {
                if !Self::has_lower_lobe(column, center) {
                    continue;
                }
                column.stroke_y_min
            };
            peak = Some(match peak {
                Some(current) if upper => current.min(y),
                Some(current) => current.max(y),
                None => y,
            });
        }
        peak
    }

    fn has_upper_lobe(column: &ColumnSample, center: f32) -> bool {
        column.stroke_y_max + 0.5 < center
    }

    fn has_lower_lobe(column: &ColumnSample, center: f32) -> bool {
        column.stroke_y_min > center + 0.5
    }

    fn upper_fill_span(
        column: &ColumnSample,
        center: f32,
        allow_mirror: bool,
    ) -> Option<(f32, f32)> {
        if !Self::has_upper_lobe(column, center) {
            return None;
        }
        let outline = column.stroke_y_max;
        let mut end = if allow_mirror && !Self::has_lower_lobe(column, center) {
            Self::flip_y(outline, center)
        } else {
            center
        };
        if Self::has_lower_lobe(column, center) {
            end = end.min(column.stroke_y_min).min(center);
        }
        if (end - outline).abs() <= 0.5 {
            None
        } else {
            Some((outline, end))
        }
    }

    fn lower_fill_span(
        column: &ColumnSample,
        center: f32,
        allow_mirror: bool,
    ) -> Option<(f32, f32)> {
        if !Self::has_lower_lobe(column, center) {
            return None;
        }
        let outline = column.stroke_y_min;
        let mut end = if allow_mirror && !Self::has_upper_lobe(column, center) {
            Self::flip_y(outline, center)
        } else {
            center
        };
        if Self::has_upper_lobe(column, center) {
            end = end.max(column.stroke_y_max).max(center);
        }
        if (end - outline).abs() <= 0.5 {
            None
        } else {
            Some((outline, end))
        }
    }

    fn append_column_quad(builder: &mut path::Builder, x: f32, half: f32, y0: f32, y1: f32) {
        builder.move_to(Point::new(x - half, y0));
        builder.line_to(Point::new(x + half, y0));
        builder.line_to(Point::new(x + half, y1));
        builder.line_to(Point::new(x - half, y1));
        builder.close();
    }

    fn two_outline_ratio(columns: &[ColumnSample], center: f32) -> f32 {
        if columns.is_empty() {
            return 0.0;
        }
        let bipolar = columns
            .iter()
            .filter(|column| Self::has_upper_lobe(column, center) && Self::has_lower_lobe(column, center))
            .count();
        bipolar as f32 / columns.len() as f32
    }

    fn two_outline_should_arm(ratio: f32, armed: bool) -> bool {
        if armed {
            ratio >= TWO_OUTLINE_EXIT
        } else {
            ratio >= TWO_OUTLINE_ENTER
        }
    }

    fn draw_lobe_fills(
        &self,
        frame: &mut Frame,
        columns: &[ColumnSample],
        column_width: f32,
        center: f32,
        fill: Color,
    ) {
        let ratio = Self::two_outline_ratio(columns, center);
        let two_outline =
            Self::two_outline_should_arm(ratio, self.two_outline_armed.get());
        self.two_outline_armed.set(two_outline);
        let half = column_width * 0.5;

        if two_outline {
            let mut builder = path::Builder::new();
            for column in columns {
                Self::append_column_quad(
                    &mut builder,
                    column.x,
                    half,
                    column.stroke_y_max,
                    column.stroke_y_min,
                );
            }
            frame.fill(&builder.build(), fill);
            return;
        }

        let any_up = columns
            .iter()
            .any(|column| Self::has_upper_lobe(column, center));
        let any_down = columns
            .iter()
            .any(|column| Self::has_lower_lobe(column, center));
        let allow_mirror = any_up != any_down;

        let mut solid = path::Builder::new();
        let mut mirror = path::Builder::new();
        let mut solid_any = false;
        let mut mirror_any = false;
        let mut peak: Option<f32> = None;

        for column in columns {
            if let Some((outline, end)) =
                Self::upper_fill_span(column, center, allow_mirror)
            {
                let solid_end = if allow_mirror { center } else { end };
                if (solid_end - outline).abs() > 0.5 {
                    Self::append_column_quad(&mut solid, column.x, half, outline, solid_end);
                    solid_any = true;
                }
                if allow_mirror {
                    Self::append_column_quad(&mut mirror, column.x, half, center, end);
                    mirror_any = true;
                }
                peak = Some(match peak {
                    Some(current) => current.min(outline),
                    None => outline,
                });
            }
            if let Some((outline, end)) =
                Self::lower_fill_span(column, center, allow_mirror)
            {
                let solid_end = if allow_mirror { center } else { end };
                if (solid_end - outline).abs() > 0.5 {
                    Self::append_column_quad(&mut solid, column.x, half, outline, solid_end);
                    solid_any = true;
                }
                if allow_mirror {
                    Self::append_column_quad(&mut mirror, column.x, half, center, end);
                    mirror_any = true;
                }
                peak = Some(match peak {
                    Some(current) => current.max(outline),
                    None => outline,
                });
            }
        }

        if solid_any {
            frame.fill(&solid.build(), fill);
        }
        if mirror_any {
            if let Some(peak) = peak {
                frame.fill(
                    &mirror.build(),
                    Self::mirror_fill_gradient(
                        fill,
                        center,
                        Self::flip_y(peak, center),
                        center,
                    ),
                );
            }
        }
    }

    fn columns_from_window(
        &self,
        height: f32,
        start: usize,
        end: usize,
        phase: f32,
        layout: WaveformLayout,
    ) -> Vec<ColumnSample> {
        let center = height / 2.0;
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return Vec::new();
        }

        let WaveformLayout {
            samples_per_col,
            column_count,
            column_width,
            px_per_sample,
            width,
            ..
        } = layout;
        debug_assert!(column_count <= width.ceil() as usize);

        let x_shift = -phase * px_per_sample;
        let peaks = match self.peaks.lock() {
            Ok(peaks) => peaks,
            Err(_) => return Vec::new(),
        };

        let mut out = Vec::with_capacity(column_count);
        for col in 0..column_count {
            let chunk_start = col * samples_per_col;
            if chunk_start >= visible_count {
                break;
            }
            let chunk_end = (chunk_start + samples_per_col).min(visible_count);
            let (min_sample, max_sample) = Self::column_extents_from_peaks(
                &peaks,
                start + chunk_start,
                start + chunk_end,
            );
            let x = (col as f32 + 0.5) * column_width + x_shift;
            out.push(ColumnSample {
                x,
                stroke_y_min: center - min_sample * center,
                stroke_y_max: center - max_sample * center,
            });
        }

        out
    }

    fn edge_path(columns: &[ColumnSample], y: impl Fn(&ColumnSample) -> f32) -> Path {
        if columns.is_empty() {
            return path::Builder::new().build();
        }

        let mut builder = path::Builder::new();
        builder.move_to(Point::new(columns[0].x, y(&columns[0])));
        for column in columns.iter().skip(1) {
            builder.line_to(Point::new(column.x, y(column)));
        }
        builder.build()
    }

    fn draw_background(&self, frame: &mut Frame, theme: &Theme, size: Size) {
        let palette = WaveformPalette::from_theme(theme);
        let background = Path::rounded_rectangle(
            Point::ORIGIN,
            size,
            Radius::new(WAVEFORM_CORNER_RADIUS),
        );
        frame.fill(&background, palette.background);
    }

    fn format_amplitude(value: f32) -> String {
        if value.abs() < 1e-6 {
            "0".to_string()
        } else {
            format!("{value:.1}")
        }
    }

    fn draw_amplitude_axis(&self, frame: &mut Frame, theme: &Theme, size: Size) {
        let palette = WaveformPalette::from_theme(theme);
        let plot = PlotArea::from_size(size);
        if plot.width <= 0.0 || plot.height <= 0.0 {
            return;
        }

        for &amplitude in &AMPLITUDE_TICKS {
            let y = plot.amplitude_y(amplitude);
            let is_zero = amplitude.abs() < 1e-6;
            let grid = Path::line(
                Point::new(plot.x, y),
                Point::new(plot.x + plot.width, y),
            );
            frame.stroke(
                &grid,
                Stroke::default()
                    .with_color(if is_zero {
                        palette.axis
                    } else {
                        palette.marker.scale_alpha(0.22)
                    })
                    .with_width(1.0),
            );

            let tick = Path::line(
                Point::new(plot.x - 4.0, y),
                Point::new(plot.x, y),
            );
            frame.stroke(
                &tick,
                Stroke::default()
                    .with_color(palette.marker)
                    .with_width(1.0),
            );

            let (align_y, label_y) = if amplitude >= 0.999 {
                (alignment::Vertical::Top, plot.y)
            } else if amplitude <= -0.999 {
                (alignment::Vertical::Bottom, plot.y + plot.height)
            } else {
                (alignment::Vertical::Center, y)
            };
            frame.fill_text(Text {
                content: Self::format_amplitude(amplitude),
                position: Point::new(plot.x - 6.0, label_y),
                color: palette.marker_label,
                size: Pixels(10.0),
                align_x: iced::alignment::Horizontal::Right.into(),
                align_y,
                ..Default::default()
            });
        }
    }

    fn with_plot_origin(frame: &mut Frame, plot: PlotArea, draw: impl FnOnce(&mut Frame)) {
        frame.push_transform();
        frame.translate(Vector::new(plot.x, plot.y));
        draw(frame);
        frame.pop_transform();
    }

    fn draw_waveform_content(
        &self,
        frame: &mut Frame,
        theme: &Theme,
        view: WaveFormView,
        size: Size,
    ) {
        let palette = WaveformPalette::from_theme(theme);
        let center = size.height / 2.0;

        if self.sample_count == 0 || size.width <= 0.0 || size.height <= 0.0 {
            return;
        }

        let (start, end, phase) = view.sample_window(self.sample_count);
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return;
        }

        let layout = view.waveform_layout(size.width, visible_count);

        if layout.sample_point_mode {
            self.draw_sample_points(
                frame,
                &palette,
                start,
                end,
                phase,
                center,
                layout,
            );
        } else {
            let columns = self.columns_from_window(
                size.height,
                start,
                end,
                phase,
                layout,
            );
            if columns.is_empty() {
                return;
            }

            self.draw_lobe_fills(frame, &columns, layout.column_width, center, palette.fill);

            let stroke = Stroke::default()
                .with_color(palette.stroke)
                .with_width(1.0)
                .with_line_cap(LineCap::Round)
                .with_line_join(LineJoin::Round);

            frame.stroke(
                &Self::edge_path(&columns, |column| column.stroke_y_max),
                stroke,
            );
            frame.stroke(
                &Self::edge_path(&columns, |column| column.stroke_y_min),
                stroke,
            );
        }

        self.draw_time_markers(frame, &palette, size, view);
    }

    fn draw_sample_points(
        &self,
        frame: &mut Frame,
        palette: &WaveformPalette,
        start: usize,
        end: usize,
        phase: f32,
        center: f32,
        layout: WaveformLayout,
    ) {
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return;
        }

        let px_per_sample = layout.px_per_sample;
        if px_per_sample <= 0.0 || !px_per_sample.is_finite() {
            return;
        }
        let x_shift = -phase * px_per_sample;
        let peaks = match self.peaks.lock() {
            Ok(peaks) => peaks,
            Err(_) => return,
        };

        let stem_stroke = Stroke::default()
            .with_color(palette.stroke.scale_alpha(0.55))
            .with_width(1.0)
            .with_line_cap(LineCap::Round);
        let trace_stroke = Stroke::default()
            .with_color(palette.stroke.scale_alpha(0.92))
            .with_width(1.25)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round);

        let mut stem_builder = path::Builder::new();
        for index in 0..visible_count {
            let sample = peaks.midpoint_at(start + index).clamp(-1.0, 1.0);
            let x = (index as f32 + 0.5) * px_per_sample + x_shift;
            let y = center - sample * center;
            if (y - center).abs() > 0.35 {
                stem_builder.move_to(Point::new(x, center));
                stem_builder.line_to(Point::new(x, y));
            }
        }

        let mut trace_builder = path::Builder::new();
        let mut trace_started = false;

        if px_per_sample >= 1.5 {
            let pixels = layout.width.ceil().max(1.0) as usize;
            for x in (0..pixels).map(|px| px as f32 + 0.5) {
                let t = Self::sample_index_at_x(x, start, phase, px_per_sample);
                let sample = Self::interpolate_peak_at(&peaks, t).clamp(-1.0, 1.0);
                let y = center - sample * center;
                if trace_started {
                    trace_builder.line_to(Point::new(x, y));
                } else {
                    trace_builder.move_to(Point::new(x, y));
                    trace_started = true;
                }
            }
        } else {
            for index in 0..visible_count {
                let sample = peaks.midpoint_at(start + index).clamp(-1.0, 1.0);
                let x = (index as f32 + 0.5) * px_per_sample + x_shift;
                let y = center - sample * center;
                if trace_started {
                    trace_builder.line_to(Point::new(x, y));
                } else {
                    trace_builder.move_to(Point::new(x, y));
                    trace_started = true;
                }
            }
        }

        frame.stroke(&stem_builder.build(), stem_stroke);
        if trace_started {
            frame.stroke(&trace_builder.build(), trace_stroke);
        }
    }

    fn nice_time_step(visible_secs: f64) -> f64 {
        if visible_secs <= 0.0 {
            return 1.0;
        }
        let raw = visible_secs / 8.0;
        let magnitude = 10_f64.powf(raw.log10().floor());
        let normalized = raw / magnitude;
        let nice = if normalized <= 1.0 {
            1.0
        } else if normalized <= 2.0 {
            2.0
        } else if normalized <= 5.0 {
            5.0
        } else {
            10.0
        };
        (nice * magnitude).max(0.001)
    }

    fn tick_spacing_px(step_secs: f64, visible_secs: f64, width: f32) -> f32 {
        if step_secs <= 0.0 || visible_secs <= 0.0 || width <= 0.0 {
            return 0.0;
        }
        (step_secs as f32 / visible_secs as f32) * width
    }

    fn tick_step_visible(step_secs: f64, visible_secs: f64, width: f32) -> bool {
        Self::tick_spacing_px(step_secs, visible_secs, width) >= MIN_TICK_GAP_PX
    }

    fn minor_time_step(major_step: f64, visible_secs: f64, width: f32) -> Option<f64> {
        const CANDIDATES: [f64; 11] =
            [60.0, 30.0, 10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.01];
        CANDIDATES.into_iter().rev().find(|&step| {
            step < major_step && Self::tick_step_visible(step, visible_secs, width)
        })
    }

    fn format_time(secs: f64, step: f64) -> String {
        let secs = secs.max(0.0);
        let decimals = if step >= 1.0 {
            0
        } else if step >= 0.1 {
            1
        } else if step >= 0.01 {
            2
        } else {
            3
        };

        let hours = (secs / 3600.0).floor() as u32;
        let minutes = ((secs % 3600.0) / 60.0).floor() as u32;
        let seconds = secs % 60.0;

        if hours > 0 {
            return match decimals {
                0 => format!("{hours}:{minutes:02}:{seconds:02.0}"),
                1 => format!("{hours}:{minutes:02}:{seconds:04.1}"),
                2 => format!("{hours}:{minutes:02}:{seconds:05.2}"),
                _ => format!("{hours}:{minutes:02}:{seconds:06.3}"),
            };
        }
        if minutes > 0 {
            return match decimals {
                0 => format!("{minutes}:{seconds:02.0}"),
                1 => format!("{minutes}:{seconds:04.1}"),
                2 => format!("{minutes}:{seconds:05.2}"),
                _ => format!("{minutes}:{seconds:06.3}"),
            };
        }
        match decimals {
            0 => format!("{seconds:.0}s"),
            1 => format!("{seconds:.1}s"),
            2 => format!("{seconds:.2}s"),
            _ => format!("{seconds:.3}s"),
        }
    }

    fn is_on_time_grid(tick_secs: f64, step: f64) -> bool {
        if step <= 0.0 {
            return false;
        }
        let remainder = tick_secs.rem_euclid(step);
        remainder < step * 0.05 || (step - remainder) < step * 0.05
    }

    fn draw_time_tick(
        frame: &mut Frame,
        palette: &WaveformPalette,
        x: f32,
        line_bottom: f32,
        label_y: f32,
        label: Option<&str>,
        tier: TimeMarkerTier,
    ) {
        let tick_height = line_bottom;
        let color = match tier {
            TimeMarkerTier::Major => palette.marker,
            TimeMarkerTier::Second => palette.marker.scale_alpha(0.82),
            TimeMarkerTier::Subsecond => palette.marker.scale_alpha(0.28),
            TimeMarkerTier::Sample => palette.marker.scale_alpha(0.16),
        };
        let line = Path::line(Point::new(x, 0.0), Point::new(x, tick_height));
        frame.stroke(
            &line,
            Stroke::default().with_color(color).with_width(1.0),
        );

        if let Some(text) = label {
            frame.fill_text(Text {
                content: text.to_owned(),
                position: Point::new(x, label_y),
                color: palette.marker_label,
                size: Pixels(10.0),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: alignment::Vertical::Bottom,
                ..Default::default()
            });
        }
    }

    fn draw_time_markers(
        &self,
        frame: &mut Frame,
        palette: &WaveformPalette,
        size: Size,
        view: WaveFormView,
    ) {
        if self.sample_rate == 0 || self.sample_count == 0 || size.width <= 0.0 {
            return;
        }

        let sample_rate = self.sample_rate as f64;
        let (start, end, phase) = view.sample_window(self.sample_count());
        let visible_samples = end.saturating_sub(start);
        if visible_samples == 0 {
            return;
        }

        let px_per_sample = size.width / visible_samples as f32;
        let start_secs = start as f64 / sample_rate;
        let visible_secs = visible_samples as f64 / sample_rate;
        let major_step = Self::nice_time_step(visible_secs);
        let end_secs = start_secs + visible_secs;
        let label_y = size.height + TIME_MARKER_HEIGHT - 2.0;
        let line_bottom = size.height;
        let sample_point_mode = view.sample_point_mode(size.width, visible_samples);

        let tick_x = |tick_secs: f64| -> f32 {
            let sample_pos = ((tick_secs - start_secs) / visible_secs) * visible_samples as f64;
            (sample_pos as f32 - phase) * px_per_sample
        };

        let draw_if_visible =
            |frame: &mut Frame, x: f32, tier: TimeMarkerTier, label: Option<&str>| {
                if (0.0..=size.width).contains(&x) {
                    Self::draw_time_tick(
                        frame,
                        palette,
                        x,
                        line_bottom,
                        label_y,
                        label,
                        tier,
                    );
                }
            };

        if sample_point_mode {
            let step = ((visible_samples as f32 / size.width).ceil() as usize).max(1);
            for offset in (0..visible_samples).step_by(step) {
                let sample_index = start + offset;
                let tick_secs = sample_index as f64 / sample_rate;
                if Self::is_on_time_grid(tick_secs, major_step)
                    || Self::is_on_time_grid(tick_secs, 1.0)
                {
                    continue;
                }
                let x = (offset as f32 + 0.5 - phase) * px_per_sample;
                draw_if_visible(frame, x, TimeMarkerTier::Sample, None);
            }
        } else if let Some(minor_step) = Self::minor_time_step(major_step, visible_secs, size.width) {
            let mut tick = ((start_secs / minor_step).ceil() * minor_step).max(0.0);
            while tick <= end_secs + minor_step * 0.001 {
                if !Self::is_on_time_grid(tick, major_step)
                    && !Self::is_on_time_grid(tick, 1.0)
                {
                    draw_if_visible(
                        frame,
                        tick_x(tick),
                        TimeMarkerTier::Subsecond,
                        None,
                    );
                }
                tick += minor_step;
            }
        }

        if Self::tick_step_visible(1.0, visible_secs, size.width) {
            let mut second = ((start_secs / 1.0).ceil() * 1.0).max(0.0);
            while second <= end_secs + 0.001 {
                if !Self::is_on_time_grid(second, major_step) {
                    draw_if_visible(frame, tick_x(second), TimeMarkerTier::Second, None);
                }
                second += 1.0;
            }
        }

        let mut major = (start_secs / major_step).ceil() * major_step;
        while major <= end_secs + major_step * 0.001 {
            draw_if_visible(
                frame,
                tick_x(major),
                TimeMarkerTier::Major,
                Some(&Self::format_time(major, major_step)),
            );
            major += major_step;
        }
    }

    fn handle_input(
        &self,
        state: &mut WaveFormState,
        view: &mut WaveFormView,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> bool {
        if !cursor.is_over(bounds) {
            return false;
        }

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if self.modifiers.shift() {
                    let (x, y) = scroll_lines(*delta);
                    let pan_delta = if x.abs() > y.abs() { -x } else { -y };
                    if pan_delta == 0.0 {
                        return false;
                    }
                    view.apply_pan_delta(
                        pan_delta as f64 * PAN_STEP as f64,
                        self.sample_count(),
                    );
                    return true;
                }
                let plot = PlotArea::from_size(bounds.size());
                if plot.width <= 0.0 {
                    return false;
                }
                let anchor_x = cursor
                    .position_in(bounds)
                    .map(|point| ((point.x - plot.x) / plot.width).clamp(0.0, 1.0))
                    .unwrap_or(0.5);
                let lines = WaveFormView::wheel_lines(*delta);
                view.accumulate_wheel(lines, anchor_x, self.sample_count(), &mut state.wheel_lines)
            }
            _ => false,
        }
    }
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
        let live = state.last_pan_view.is_some() || view.content_transform_active();

        let plot = PlotArea::from_size(size);
        let plot_size = plot.size();
        let plot_clip = Rectangle::new(
            Point::new(plot.x - PLOT_CLIP_BLEED_LEFT, plot.y),
            Size::new(plot.width + PLOT_CLIP_BLEED_LEFT, plot.height + TIME_MARKER_HEIGHT),
        );

        let mut bg_frame = Frame::new(renderer, size);
        self.draw_background(&mut bg_frame, theme, size);
        self.draw_amplitude_axis(&mut bg_frame, theme, size);
        let mut layers = vec![bg_frame.into_geometry()];

        if live {
            let mut frame = Frame::new(renderer, size);
            frame.with_clip(plot_clip, |frame| {
                Self::with_plot_origin(frame, plot, |frame| {
                    let draw_content = |frame: &mut Frame| {
                        self.draw_waveform_content(frame, theme, view, plot_size);
                    };
                    if view.content_transform_active() {
                        Self::with_content_transform(
                            frame,
                            view,
                            plot.width,
                            plot.height,
                            self.sample_count(),
                            |frame| {
                                draw_content(frame);
                                if let Some(progress) = progress {
                                    self.draw_playhead_on_frame(
                                        frame,
                                        theme,
                                        plot_size,
                                        progress,
                                        view,
                                    );
                                }
                            },
                        );
                    } else {
                        draw_content(frame);
                    }
                });
            });
            layers.push(frame.into_geometry());
        } else {
            self.sync_content_cache(theme, plot_size.width);
            layers.push(self.cache.draw(renderer, size, |frame| {
                frame.with_clip(plot_clip, |frame| {
                    Self::with_plot_origin(frame, plot, |frame| {
                        self.draw_waveform_content(frame, theme, view, plot_size);
                    });
                });
            }));
        }

        if let Some(progress) = progress
            && !view.content_transform_active()
            && let Some(playhead) =
                self.draw_playhead(renderer, size, theme, progress, view)
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
        if state.tracked_samples != self.sample_count() {
            state.tracked_samples = self.sample_count();
            state.wheel_lines = 0.0;
        }

        if !self.ui_scrubbing.get() && state.scrub_active {
            state.scrub_active = false;
        }

        if let Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) = event {
            state.file_drag_armed = false;
            state.file_drag_origin = None;
        }

        if let Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) = event
            && state.scrub_active
        {
            state.scrub_active = false;
            self.ui_scrubbing.set(false);
            return Some(
                Action::publish(Message::WaveformScrubEnd(state.last_scrub_progress)).and_capture(),
            );
        }

        if let Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) = event
            && state.pan_anchor.is_some()
        {
            let view = state.last_pan_view.take().unwrap_or(self.view);
            state.pan_anchor = None;
            state.last_pan_x = None;
            return Some(
                Action::publish(Message::WaveformPanEnded(view)).and_capture(),
            );
        }

        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && cursor.is_over(bounds)
            && let Some(position) = cursor.position_in(bounds)
        {
            if self.modifiers.control() {
                state.file_drag_armed = true;
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
                return Some(Action::publish(Message::WaveformPanStarted).and_capture());
            }
            if let Some(progress) = self.progress_at_x(
                state.last_pan_view.unwrap_or(self.view),
                PlotArea::from_size(bounds.size()),
                position.x,
            ) {
                state.scrub_active = true;
                state.last_scrub_progress = progress;
                self.ui_scrubbing.set(true);
                return Some(Action::publish(Message::WaveformScrub(progress)).and_capture());
            }
        }

        if state.file_drag_armed
            && let Event::Mouse(mouse::Event::CursorMoved { .. }) = event
            && let Some(position) = cursor.position_in(bounds)
            && let Some(origin) = state.file_drag_origin
        {
            let dx = position.x - origin.x;
            let dy = position.y - origin.y;
            if dx * dx + dy * dy >= FILE_DRAG_THRESHOLD * FILE_DRAG_THRESHOLD {
                state.file_drag_armed = false;
                state.file_drag_origin = None;
                return Some(Action::publish(Message::WaveformFileDragStart).and_capture());
            }
        }

        if state.scrub_active
            && let Event::Mouse(mouse::Event::CursorMoved { .. }) = event
            && let Some(position) = cursor.position_in(bounds)
            && let Some(progress) = self.progress_at_x(
                state.last_pan_view.unwrap_or(self.view),
                PlotArea::from_size(bounds.size()),
                position.x,
            )
        {
            state.last_scrub_progress = progress;
            return Some(Action::publish(Message::WaveformScrub(progress)).and_capture());
        }

        if state.pan_anchor.is_some()
            && let Event::Mouse(mouse::Event::CursorMoved { .. }) = event
            && let Some(position) = cursor.position_in(bounds)
            && let Some(anchor) = state.pan_anchor
        {
            let last_x = state.last_pan_x.unwrap_or(position.x);
            let plot = PlotArea::from_size(bounds.size());
            let visible = visible_fraction_of(self.sample_count(), self.view.zoom);
            let step = -(position.x - last_x) as f64 / plot.width as f64 * visible;
            state.last_pan_x = Some(position.x);

            let mut view = state.last_pan_view.unwrap_or(WaveFormView {
                zoom: self.view.zoom,
                offset: anchor.view_offset,
                overscroll: anchor.overscroll,
            });
            view.apply_pan_delta(step, self.sample_count());
            state.last_pan_view = Some(view);
            return Some(Action::request_redraw().and_capture());
        }

        let mut view = self.view;
        if !self.handle_input(state, &mut view, event, bounds, cursor) {
            return None;
        }

        Some(
            Action::publish(Message::WaveformViewChanged(view)).and_capture(),
        )
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> iced::mouse::Interaction {
        if cursor.is_over(bounds) {
            if state.scrub_active {
                return iced::mouse::Interaction::Pointer;
            }
            if state.pan_anchor.is_some() {
                return iced::mouse::Interaction::Grabbing;
            }
            if self.modifiers.control() {
                return iced::mouse::Interaction::Grab;
            }
            if self.modifiers.shift() {
                return iced::mouse::Interaction::Grab;
            }
            return iced::mouse::Interaction::Pointer;
        }
        iced::mouse::Interaction::default()
    }
}

#[cfg(test)]
mod wheel_zoom_tests {
    use super::WaveFormView;

    #[test]
    fn direction_change_does_not_zoom_same_way() {
        let mut view = WaveFormView::default();
        let mut pending = 0.0;
        let samples = 10_000;

        view.accumulate_wheel(0.75, 0.5, samples, &mut pending);
        let zoom_before = view.zoom;
        assert!(
            pending > 0.0,
            "expected leftover down-scroll pending, got {pending}"
        );

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
        let (start, end, phase) = view.sample_window(samples);
        let before = start as f64 + phase as f64 + anchor_x as f64 * (end - start) as f64;

        view.apply_zoom_at(2.0, anchor_x, samples);

        let (start, end, phase) = view.sample_window(samples);
        let after = start as f64 + phase as f64 + anchor_x as f64 * (end - start) as f64;
        assert!(
            (after - before).abs() < 2.0,
            "cursor sample moved: before={before} after={after}"
        );
    }

    #[test]
    fn pan_to_max_shows_last_sample() {
        let mut view = WaveFormView {
            zoom: 4.0,
            ..Default::default()
        };
        view.offset = view.max_offset(100_000);
        let (_, end, _) = view.sample_window(100_000);
        assert_eq!(end, 100_000);
    }

    #[test]
    fn playhead_and_first_sample_start_at_plot_origin() {
        let view = WaveFormView::default();
        let plot_width = 800.0;
        let sample_count = 48_000;
        let (start, end, phase) = view.sample_window(sample_count);
        assert_eq!((start, phase), (0, 0.0));
        let layout = view.waveform_layout(plot_width, end - start);
        let x_shift = -phase * layout.px_per_sample;
        let first_col_x = 0.5 * layout.column_width + x_shift;
        assert!(
            first_col_x - layout.column_width * 0.5 >= -f32::EPSILON,
            "first column envelope should reach the left plot edge"
        );
        let sample_pos = 0.0_f32 - phase;
        let playhead_x = sample_pos * layout.px_per_sample;
        assert!((playhead_x - 0.0).abs() < f32::EPSILON);
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
}

#[cfg(test)]
mod time_marker_tests {
    use super::{WaveForm, MIN_TICK_GAP_PX};

    #[test]
    fn far_zoom_hides_subsecond_and_second_ticks() {
        let visible_secs = 200.0;
        let width = 800.0;
        assert!(!WaveForm::tick_step_visible(0.1, visible_secs, width));
        assert!(!WaveForm::tick_step_visible(1.0, visible_secs, width));
        assert!(WaveForm::tick_step_visible(10.0, visible_secs, width));
        assert!(
            WaveForm::tick_spacing_px(1.0, visible_secs, width) < MIN_TICK_GAP_PX
        );
    }

    #[test]
    fn close_zoom_keeps_subsecond_ticks() {
        let visible_secs = 2.0;
        let width = 800.0;
        assert!(WaveForm::tick_step_visible(0.1, visible_secs, width));
        let minor = WaveForm::minor_time_step(1.0, visible_secs, width)
            .expect("close zoom should keep subsecond ticks");
        assert!(minor < 1.0);
        assert!(WaveForm::tick_step_visible(minor, visible_secs, width));
    }

    #[test]
    fn far_zoom_picks_minor_step_that_still_has_gap() {
        let visible_secs = 200.0;
        let width = 800.0;
        let major = WaveForm::nice_time_step(visible_secs);
        let minor = WaveForm::minor_time_step(major, visible_secs, width)
            .expect("far zoom should still have a sparse minor grid");
        assert!(minor < major);
        assert!(WaveForm::tick_step_visible(minor, visible_secs, width));
        assert!(minor >= 1.0, "blended 0.1s/1s ticks must not be chosen");
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::{max_zoom, ColumnSample, WaveForm, WaveFormView};
    use iced::Color;
    use iced::widget::canvas::Gradient;
    use iced::Theme;

    #[test]
    fn column_extents_preserves_asymmetric_lobes() {
        let positive_only = [0.2, 0.5, 0.8];
        let (min, max) = WaveForm::column_extents(&positive_only);
        assert!(min >= 0.0);
        assert_eq!(max, 0.8);

        let negative_only = [-0.7, -0.4, -0.1];
        let (min, max) = WaveForm::column_extents(&negative_only);
        assert_eq!(min, -0.7);
        assert!(max <= 0.0);
    }

    #[test]
    fn sample_point_mode_when_one_sample_per_column() {
        let view = WaveFormView { zoom: 4096.0, ..Default::default() };
        let width = 900.0;
        let visible = 400;
        let layout = view.waveform_layout(width, visible);
        assert_eq!(layout.samples_per_col, 1);
        assert!(layout.px_per_sample >= 1.0);
        assert!(layout.sample_point_mode);
    }

    #[test]
    fn sample_point_mode_has_no_visible_count_cap() {
        let view = WaveFormView::default();
        let width = 5000.0;
        let visible = 5000;
        let layout = view.waveform_layout(width, visible);
        assert_eq!(layout.samples_per_col, 1);
        assert!(layout.sample_point_mode);
    }

    #[test]
    fn zoom_can_reach_sample_level_on_long_files() {
        let mut view = WaveFormView::default();
        let sample_count = 8_640_000;
        let width = 800.0;
        for _ in 0..80 {
            view.zoom_in(sample_count);
        }
        assert_eq!(view.zoom, max_zoom(sample_count));
        let (start, end, _) = view.sample_window(sample_count);
        let visible = end.saturating_sub(start);
        let layout = view.waveform_layout(width, visible);
        assert_eq!(visible, 1);
        assert_eq!(layout.samples_per_col, 1);
        assert!(layout.sample_point_mode);
    }

    #[test]
    fn column_count_stays_at_or_below_width() {
        let view = WaveFormView { zoom: 28.0, ..Default::default() };
        let width = 900.0;
        let visible = 132_300;
        let layout = view.waveform_layout(width, visible);
        assert!(
            layout.column_count <= width.ceil() as usize,
            "got {}",
            layout.column_count
        );
    }

    #[test]
    fn column_envelope_y_matches_sample_y() {
        let center = 100.0;
        let (min, max) = WaveForm::column_extents(&[0.25, 0.75]);
        let y_top = center - max * center;
        let y_bottom = center - min * center;
        assert!((y_top - 25.0).abs() < f32::EPSILON);
        assert!((y_bottom - 75.0).abs() < f32::EPSILON);
        assert!(y_top < y_bottom);
    }

    #[test]
    fn flip_y_mirrors_across_center() {
        let center = 100.0;
        assert!((WaveForm::flip_y(25.0, center) - 175.0).abs() < f32::EPSILON);
        assert!((WaveForm::flip_y(center, center) - center).abs() < f32::EPSILON);
    }

    #[test]
    fn up_lobe_mirrors_below_and_down_lobe_mirrors_above() {
        let center = 100.0;
        let up = ColumnSample {
            x: 0.0,
            stroke_y_min: center,
            stroke_y_max: 20.0,
        };
        let (outline, end) = WaveForm::upper_fill_span(&up, center, true).expect("up span");
        assert!((outline - 20.0).abs() < f32::EPSILON);
        assert!((end - 180.0).abs() < f32::EPSILON);
        assert!(WaveForm::lower_fill_span(&up, center, true).is_none());

        let down = ColumnSample {
            x: 0.0,
            stroke_y_min: 180.0,
            stroke_y_max: center,
        };
        let (outline, end) = WaveForm::lower_fill_span(&down, center, true).expect("down span");
        assert!((outline - 180.0).abs() < f32::EPSILON);
        assert!((end - 20.0).abs() < f32::EPSILON);
        assert!(WaveForm::upper_fill_span(&down, center, true).is_none());
    }

    #[test]
    fn bipolar_column_does_not_fill_past_outline() {
        let center = 100.0;
        let column = ColumnSample {
            x: 0.0,
            stroke_y_min: 160.0,
            stroke_y_max: 20.0,
        };
        let (up_outline, up_end) =
            WaveForm::upper_fill_span(&column, center, true).expect("up span");
        assert!((up_outline - 20.0).abs() < f32::EPSILON);
        assert!(up_end <= center + f32::EPSILON);
        assert!(up_end <= column.stroke_y_min);
        let (down_outline, down_end) =
            WaveForm::lower_fill_span(&column, center, true).expect("down span");
        assert!((down_outline - 160.0).abs() < f32::EPSILON);
        assert!(down_end >= center - f32::EPSILON);
        assert!(down_end >= column.stroke_y_max);
    }

    #[test]
    fn mixed_sides_clip_fill_at_center() {
        let center = 100.0;
        let up = ColumnSample {
            x: 0.0,
            stroke_y_min: center,
            stroke_y_max: 20.0,
        };
        let (outline, end) = WaveForm::upper_fill_span(&up, center, false).expect("up span");
        assert!((outline - 20.0).abs() < f32::EPSILON);
        assert!((end - center).abs() < f32::EPSILON);
    }

    #[test]
    fn two_outline_view_uses_solid_fill_not_gradient() {
        let center = 100.0;
        let columns = vec![
            ColumnSample {
                x: 0.0,
                stroke_y_min: 160.0,
                stroke_y_max: 20.0,
            },
            ColumnSample {
                x: 4.0,
                stroke_y_min: 150.0,
                stroke_y_max: 40.0,
            },
        ];
        let ratio = WaveForm::two_outline_ratio(&columns, center);
        assert!(WaveForm::two_outline_should_arm(ratio, false));
    }

    #[test]
    fn two_outline_fill_uses_hysteresis() {
        assert!(!WaveForm::two_outline_should_arm(0.50, false));
        assert!(WaveForm::two_outline_should_arm(0.50, true));
        assert!(WaveForm::two_outline_should_arm(0.70, false));
        assert!(!WaveForm::two_outline_should_arm(0.30, true));
    }

    #[test]
    fn gradient_is_opaque_at_outline_and_clear_at_mirror() {
        let color = Color::from_rgb(0.2, 0.4, 1.0).scale_alpha(0.22);
        let Gradient::Linear(linear) =
            WaveForm::mirror_fill_gradient(color, 20.0, 180.0, 100.0);
        assert!((linear.start.y - 20.0).abs() < f32::EPSILON);
        assert!((linear.end.y - 180.0).abs() < f32::EPSILON);
        let stops: Vec<_> = linear.stops.iter().flatten().collect();
        assert!((stops[0].color.a - 0.22).abs() < f32::EPSILON);
        assert_eq!(stops.last().map(|stop| stop.color.a), Some(0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.5 && stop.color.a > 0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.85 && stop.color.a == 0.0));
    }

    #[test]
    fn detected_peak_y_picks_extreme_lobe() {
        let center = 100.0;
        let columns = vec![
            ColumnSample {
                x: 0.0,
                stroke_y_min: center,
                stroke_y_max: 40.0,
            },
            ColumnSample {
                x: 4.0,
                stroke_y_min: center,
                stroke_y_max: 15.0,
            },
        ];
        assert_eq!(WaveForm::detected_peak_y(&columns, center, true), Some(15.0));
        assert_eq!(WaveForm::detected_peak_y(&columns, center, false), None);
    }

    #[test]
    fn draw_cache_key_includes_plot_width() {
        let view = WaveFormView::default();
        let theme = Theme::Dark;
        let narrow = view.draw_cache_key(10_000, 400.0, &theme);
        let wide = view.draw_cache_key(10_000, 800.0, &theme);
        assert_ne!(narrow, wide);
    }

    #[test]
    fn lanczos3_is_one_at_origin_and_zero_outside_window() {
        assert!((WaveForm::lanczos3(0.0) - 1.0).abs() < 1e-12);
        assert_eq!(WaveForm::lanczos3(3.0), 0.0);
        assert_eq!(WaveForm::lanczos3(-3.0), 0.0);
        assert_eq!(WaveForm::lanczos3(4.0), 0.0);
        assert_eq!(WaveForm::lanczos3(f64::NAN), 0.0);
    }

    #[test]
    fn interpolate_at_reconstructs_integer_samples() {
        let samples = [0.0, 0.5, -0.25, 1.0];
        for (index, &sample) in samples.iter().enumerate() {
            let value = WaveForm::interpolate_at(&samples, index as f64);
            assert!(
                (value - sample).abs() < 1e-6,
                "t={index}: got {value}, want {sample}"
            );
        }
    }

    #[test]
    fn interpolate_at_is_safe_at_boundaries() {
        assert_eq!(WaveForm::interpolate_at(&[], 0.0), 0.0);
        assert_eq!(WaveForm::interpolate_at(&[0.8], f64::NAN), 0.0);
        let edge = WaveForm::interpolate_at(&[1.0, 0.0, -1.0], -1.5);
        let past = WaveForm::interpolate_at(&[1.0, 0.0, -1.0], 8.0);
        assert!(edge.is_finite());
        assert!(past.is_finite());
        assert!(past.abs() < 1e-6);
        assert_eq!(WaveForm::interpolate_at(&[1.0, 0.0, -1.0], 1e300), 0.0);
    }

    #[test]
    fn sample_index_at_x_matches_stem_centers() {
        let start = 10usize;
        let phase = 0.25_f32;
        let px = 12.0_f32;
        let x = (2.0 + 0.5) * px - phase * px;
        let t = WaveForm::sample_index_at_x(x, start, phase, px);
        assert!((t - (start + 2) as f64).abs() < 1e-6);
    }
}
