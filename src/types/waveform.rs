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
use std::sync::Arc;

use crate::source::arc_samples::PlaybackPosition;

const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 4096.0;
const ZOOM_FACTOR: f32 = 1.25;
const MAX_COLUMNS_FACTOR: f32 = 4.0;
const SAMPLE_POINTS_MIN_PX: f32 = 1.0;
const PAN_STEP: f32 = 0.08;
const TIME_MARKER_HEIGHT: f32 = 16.0;
const MAX_OVERSCROLL: f32 = 0.14;
const OVERSCROLL_SPRING: f32 = 0.78;
const OVERSCROLL_STOP: f32 = 0.002;
const WHEEL_ZOOM_TAIL: f32 = 0.01;
const WHEEL_ZOOM_MAX: f32 = 0.35;
const EDGE_RUBBER_BAND: f32 = 0.35;
const WAVEFORM_CORNER_RADIUS: f32 = 8.0;

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
        ScrollDelta::Pixels { x, y } => (x / 48.0, y / 48.0),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveFormView {
    pub zoom: f32,
    pub offset: f32,
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
        *pending += lines;
        if pending.abs() < WHEEL_ZOOM_TAIL {
            return false;
        }
        let step = pending.clamp(-WHEEL_ZOOM_MAX, WHEEL_ZOOM_MAX);
        self.apply_zoom_at(ZOOM_FACTOR.powf(step), anchor_x, sample_count);
        *pending -= step;
        true
    }

    pub fn pan(&mut self, delta: f32) {
        self.apply_pan_delta(delta * self.visible_fraction());
    }

    pub fn apply_pan_delta(&mut self, offset_delta: f32) {
        let visible = self.visible_fraction();
        let max = self.max_offset();
        let edge_pull = offset_delta / visible.max(1e-6) * EDGE_RUBBER_BAND;

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
            let overflow = -target / visible.max(1e-6);
            self.overscroll = Self::rubber_band(0.0, -overflow * EDGE_RUBBER_BAND);
        } else if target > max {
            self.offset = max;
            let overflow = (target - max) / visible.max(1e-6);
            self.overscroll = Self::rubber_band(0.0, overflow * EDGE_RUBBER_BAND);
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

    fn timeline_window(&self, sample_count: usize) -> (f32, f32) {
        if sample_count == 0 {
            return (0.0, 1.0);
        }
        let (start, end, phase) = self.sample_window(sample_count);
        let width = (end - start) as f32 / sample_count as f32;
        let left = (start as f32 + phase) / sample_count as f32;
        (left, width)
    }

    fn set_timeline_left(&mut self, left: f32, sample_count: usize) {
        if sample_count == 0 {
            self.offset = 0.0;
            return;
        }
        let visible = (sample_count as f32 * self.visible_fraction())
            .ceil()
            .clamp(1.0, sample_count as f32) as usize;
        let max_start = sample_count.saturating_sub(visible);
        if max_start == 0 {
            self.offset = 0.0;
            return;
        }
        let raw_start = (left * sample_count as f32).clamp(0.0, max_start as f32);
        let start = raw_start.round() as usize;
        self.offset = (start as f32 / max_start as f32).clamp(0.0, self.max_offset());
    }

    fn apply_zoom_at(&mut self, factor: f32, anchor_x: f32, sample_count: usize) {
        if sample_count == 0 {
            return;
        }
        let anchor_x = anchor_x.clamp(0.0, 1.0);
        let (old_left, old_width) = self.timeline_window(sample_count);
        let anchor_timeline = old_left + anchor_x * old_width;

        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);

        let (_, new_width) = self.timeline_window(sample_count);
        let new_left = anchor_timeline - anchor_x * new_width;
        self.set_timeline_left(new_left, sample_count);
        self.overscroll = 0.0;
    }

    pub(crate) fn visible_fraction(&self) -> f32 {
        1.0 / self.zoom
    }

    pub(crate) fn max_offset(&self) -> f32 {
        (1.0 - self.visible_fraction()).max(0.0)
    }

    /// Visible sample window: `(start, end, sub-sample phase)`.
    fn sample_window(&self, sample_count: usize) -> (usize, usize, f32) {
        if sample_count == 0 {
            return (0, 0, 0.0);
        }

        let visible = (sample_count as f32 * self.visible_fraction())
            .ceil()
            .clamp(1.0, sample_count as f32) as usize;
        let max_start = sample_count.saturating_sub(visible);
        if max_start == 0 {
            return (0, visible.min(sample_count), 0.0);
        }

        let raw_start = self.offset * max_start as f32;
        let start = raw_start.floor() as usize;
        let phase = raw_start - start as f32;
        let start = start.min(max_start);
        (start, (start + visible).min(sample_count), phase)
    }

    fn view_cache_key(&self, sample_count: usize) -> (u32, u32, u32) {
        let (start, _, phase) = self.sample_window(sample_count);
        let zoom_q = (self.zoom * 64.0).round() as u32;
        let phase_q = (phase * 8.0).round() as u32;
        (start as u32, zoom_q, phase_q)
    }

    fn draw_cache_key(&self, sample_count: usize, theme: &Theme) -> (u32, u32, u32, u32) {
        let (start, zoom_q, phase_q) = self.view_cache_key(sample_count);
        (start, zoom_q, phase_q, theme_cache_key(theme))
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
        overscroll * width * 0.55
    }

    fn content_transform_active(&self) -> bool {
        self.overscroll != 0.0
    }

    fn column_count(&self, width: f32, visible_samples: usize) -> usize {
        if visible_samples == 0 || width <= 0.0 {
            return 1;
        }

        let max_columns = (width * MAX_COLUMNS_FACTOR).ceil() as usize;
        max_columns.min(visible_samples).max(1)
    }

    fn samples_per_column(&self, width: f32, visible_samples: usize) -> usize {
        if visible_samples == 0 {
            return 1;
        }
        let columns = self.column_count(width, visible_samples);
        visible_samples.div_ceil(columns).next_power_of_two().max(1)
    }

    fn sample_point_mode(&self, width: f32, visible_samples: usize) -> bool {
        if visible_samples == 0 || width <= 0.0 {
            return false;
        }
        self.samples_per_column(width, visible_samples) == 1
            && width / visible_samples as f32 >= SAMPLE_POINTS_MIN_PX
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
                view.pan(-PAN_STEP);
                true
            }
            Key::Named(iced::keyboard::key::Named::ArrowRight) => {
                view.pan(PAN_STEP);
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
    wheel_lines: f32,
    tracked_samples: usize,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct PanAnchor {
    view_offset: f32,
    overscroll: f32,
}

pub struct WaveForm {
    pub samples: Vec<f32>,
    view: WaveFormView,
    sample_rate: u32,
    playback_position: Option<Arc<PlaybackPosition>>,
    scrub_progress: Option<f64>,
    modifiers: Modifiers,
    pan_active: bool,
    cache: Cache,
    content_cache_key: Cell<(u32, u32, u32, u32)>,
}

struct ColumnSample {
    x: f32,
    y_min: f32,
    y_max: f32,
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
    pub fn new(samples: Vec<f32>) -> Self {
        Self {
            samples,
            view: WaveFormView::default(),
            sample_rate: 0,
            playback_position: None,
            scrub_progress: None,
            modifiers: Modifiers::default(),
            pan_active: false,
            cache: Cache::new(),
            content_cache_key: Cell::new((0, 0, 0, 0)),
        }
    }

    fn sync_content_cache(&self, theme: &Theme) {
        let key = self.view.draw_cache_key(self.samples.len(), theme);
        if self.content_cache_key.get() != key {
            self.cache.clear();
            self.content_cache_key.set(key);
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    pub fn set_playback_position(&mut self, position: Arc<PlaybackPosition>) {
        self.playback_position = Some(position);
    }

    pub fn set_scrub_progress(&mut self, progress: Option<f64>) {
        if self.scrub_progress != progress {
            self.scrub_progress = progress;
        }
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
        draw: impl FnOnce(&mut Frame),
    ) {
        let (scale_x, scale_y) = view.content_scale();
        let translate_x = view.content_translate_x(width);
        frame.push_transform();
        frame.translate(Vector::new(translate_x, 0.0));
        frame.translate(Vector::new(width / 2.0, height / 2.0));
        frame.scale_nonuniform(Vector::new(scale_x, scale_y));
        frame.translate(Vector::new(-width / 2.0, -height / 2.0));
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

    fn playhead_content_x(&self, view: WaveFormView, width: f32, progress: f64) -> Option<f32> {
        if self.samples.is_empty() || width <= 0.0 {
            return None;
        }

        let frame_count = self
            .playback_position
            .as_ref()
            .map(|position| position.total_frames() as usize)
            .unwrap_or(self.samples.len());
        if frame_count == 0 {
            return None;
        }

        let progress_frame = ((progress * frame_count as f64).round() as usize)
            .min(frame_count.saturating_sub(1));
        let (start, end, phase) = view.sample_window(self.samples.len());
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let px_per_sample = width / visible as f32;
        let sample_pos = progress_frame.saturating_sub(start) as f32 - phase;
        Some(sample_pos * px_per_sample)
    }

    fn playhead_screen_x(&self, view: WaveFormView, width: f32, progress: f64) -> Option<f32> {
        let x = self.playhead_content_x(view, width, progress)?;
        Some(self.map_content_x(view, width, x))
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

    fn progress_at_x(&self, view: WaveFormView, width: f32, x: f32) -> Option<f64> {
        if self.samples.is_empty() || width <= 0.0 {
            return None;
        }

        let frame_count = self
            .playback_position
            .as_ref()
            .map(|position| position.total_frames() as usize)
            .unwrap_or(self.samples.len());
        if frame_count == 0 {
            return None;
        }

        let (start, end, phase) = view.sample_window(self.samples.len());
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let px_per_sample = width / visible as f32;
        let content_x = self.unmap_content_x(view, width, x);
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
        let x = self.playhead_screen_x(view, size.width, progress)?;
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

    fn peak_amplitude(chunk: &[f32]) -> f32 {
        chunk
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0f32, |peak, sample| peak.max(sample))
            .clamp(0.0, 1.0)
    }

    fn columns_from_window(
        &self,
        view: WaveFormView,
        width: f32,
        height: f32,
        start: usize,
        end: usize,
        phase: f32,
    ) -> Vec<ColumnSample> {
        let center = height / 2.0;
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return Vec::new();
        }

        let samples_per_col = view.samples_per_column(width, visible_count);
        let column_count = visible_count.div_ceil(samples_per_col);
        let column_width = width / column_count as f32;
        let px_per_sample = width / visible_count as f32;
        let x_shift = -phase * px_per_sample;
        let slice = &self.samples[start..end];

        let mut out = Vec::with_capacity(column_count);
        for col in 0..column_count {
            let chunk_start = col * samples_per_col;
            if chunk_start >= visible_count {
                break;
            }
            let chunk_end = (chunk_start + samples_per_col).min(visible_count);
            let peak = Self::peak_amplitude(&slice[chunk_start..chunk_end]);
            let x = (col as f32 + 0.5) * column_width + x_shift;
            out.push(ColumnSample {
                x,
                y_min: center + peak * center,
                y_max: center - peak * center,
            });
        }

        out
    }

    fn envelope_path(columns: &[ColumnSample]) -> Path {
        if columns.is_empty() {
            return path::Builder::new().build();
        }

        let mut builder = path::Builder::new();
        builder.move_to(Point {
            x: columns[0].x,
            y: columns[0].y_max,
        });
        for column in columns.iter().skip(1) {
            builder.line_to(Point {
                x: column.x,
                y: column.y_max,
            });
        }
        for column in columns.iter().rev() {
            builder.line_to(Point {
                x: column.x,
                y: column.y_min,
            });
        }
        builder.close();
        builder.build()
    }

    fn envelope_line_path(columns: &[ColumnSample], upper: bool) -> Path {
        if columns.is_empty() {
            return path::Builder::new().build();
        }

        let y = |column: &ColumnSample| if upper { column.y_max } else { column.y_min };
        let mut builder = path::Builder::new();
        builder.move_to(Point {
            x: columns[0].x,
            y: y(&columns[0]),
        });
        for column in columns.iter().skip(1) {
            builder.line_to(Point {
                x: column.x,
                y: y(column),
            });
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

    fn draw_waveform_content(&self, frame: &mut Frame, theme: &Theme, view: WaveFormView) {
        let palette = WaveformPalette::from_theme(theme);
        let size = frame.size();
        let center = size.height / 2.0;

        if self.samples.is_empty() || size.width <= 0.0 || size.height <= 0.0 {
            return;
        }

        let (start, end, phase) = view.sample_window(self.samples.len());
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return;
        }

        let axis = Path::line(
            Point::new(0.0, center),
            Point::new(size.width, center),
        );
        frame.stroke(
            &axis,
            Stroke::default()
                .with_color(palette.axis)
                .with_width(1.0),
        );

        if view.sample_point_mode(size.width, visible_count) {
            self.draw_sample_points(frame, &palette, size, start, end, phase, center);
        } else {
            let columns = self.columns_from_window(view, size.width, size.height, start, end, phase);
            if columns.is_empty() {
                return;
            }

            frame.fill(&Self::envelope_path(&columns), palette.fill);

            let stroke = Stroke::default()
                .with_color(palette.stroke)
                .with_width(1.0)
                .with_line_cap(LineCap::Round)
                .with_line_join(LineJoin::Round);

            frame.stroke(&Self::envelope_line_path(&columns, true), stroke);
            frame.stroke(&Self::envelope_line_path(&columns, false), stroke);
        }

        self.draw_time_markers(frame, &palette, size, view);
    }

    fn draw_sample_points(
        &self,
        frame: &mut Frame,
        palette: &WaveformPalette,
        size: Size,
        start: usize,
        end: usize,
        phase: f32,
        center: f32,
    ) {
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return;
        }

        let px_per_sample = size.width / visible_count as f32;
        let x_shift = -phase * px_per_sample;
        let slice = &self.samples[start..end];

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
        let mut trace_builder = path::Builder::new();
        let mut trace_started = false;

        for (index, sample) in slice.iter().enumerate() {
            let x = (index as f32 + 0.5) * px_per_sample + x_shift;
            let sample = sample.clamp(-1.0, 1.0);
            let y = center - sample * center;

            if (y - center).abs() > 0.35 {
                stem_builder.move_to(Point::new(x, center));
                stem_builder.line_to(Point::new(x, y));
            }

            if trace_started {
                trace_builder.line_to(Point::new(x, y));
            } else {
                trace_builder.move_to(Point::new(x, y));
                trace_started = true;
            }
        }

        if trace_started {
            frame.stroke(&stem_builder.build(), stem_stroke);
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

    fn minor_time_step(major_step: f64) -> Option<f64> {
        if major_step >= 1.0 {
            Some(0.1)
        } else if major_step >= 0.2 {
            Some(0.05)
        } else if major_step >= 0.05 {
            Some(0.01)
        } else {
            None
        }
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
        if self.sample_rate == 0 || self.samples.is_empty() || size.width <= 0.0 {
            return;
        }

        let sample_rate = self.sample_rate as f64;
        let (start, end, phase) = view.sample_window(self.samples.len());
        let visible_samples = end.saturating_sub(start);
        if visible_samples == 0 {
            return;
        }

        let px_per_sample = size.width / visible_samples as f32;
        let start_secs = start as f64 / sample_rate;
        let visible_secs = visible_samples as f64 / sample_rate;
        let major_step = Self::nice_time_step(visible_secs);
        let end_secs = start_secs + visible_secs;
        let label_y = size.height - 2.0;
        let line_bottom = size.height - TIME_MARKER_HEIGHT;
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
        } else if let Some(minor_step) = Self::minor_time_step(major_step) {
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

        let mut second = ((start_secs / 1.0).ceil() * 1.0).max(0.0);
        while second <= end_secs + 0.001 {
            if !Self::is_on_time_grid(second, major_step) {
                draw_if_visible(frame, tick_x(second), TimeMarkerTier::Second, None);
            }
            second += 1.0;
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
                    view.apply_pan_delta(pan_delta * PAN_STEP);
                    return true;
                }
                if bounds.width <= 0.0 {
                    return false;
                }
                let anchor_x = cursor
                    .position_in(bounds)
                    .map(|point| (point.x / bounds.width).clamp(0.0, 1.0))
                    .unwrap_or(0.5);
                let lines = WaveFormView::wheel_lines(*delta);
                view.accumulate_wheel(lines, anchor_x, self.samples.len(), &mut state.wheel_lines)
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
        let clip_bounds = Rectangle::new(Point::ORIGIN, size);

        let mut bg_frame = Frame::new(renderer, size);
        self.draw_background(&mut bg_frame, theme, size);
        let mut layers = vec![bg_frame.into_geometry()];

        if live {
            let mut frame = Frame::new(renderer, size);
            frame.with_clip(clip_bounds, |frame| {
                let draw_content = |frame: &mut Frame| {
                    self.draw_waveform_content(frame, theme, view);
                };
                if view.content_transform_active() {
                    Self::with_content_transform(
                        frame,
                        view,
                        size.width,
                        size.height,
                        |frame| {
                            draw_content(frame);
                            if let Some(progress) = progress {
                                self.draw_playhead_on_frame(
                                    frame,
                                    theme,
                                    size,
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
            layers.push(frame.into_geometry());
        } else {
            self.sync_content_cache(theme);
            layers.push(self.cache.draw(renderer, size, |frame| {
                frame.with_clip(clip_bounds, |frame| {
                    self.draw_waveform_content(frame, theme, view);
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
        if state.tracked_samples != self.samples.len() {
            state.tracked_samples = self.samples.len();
            state.wheel_lines = 0.0;
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
            if self.modifiers.shift() {
                state.pan_anchor = Some(PanAnchor {
                    view_offset: self.view.offset,
                    overscroll: self.view.overscroll,
                });
                state.last_pan_x = Some(position.x);
                state.last_pan_view = None;
                return Some(Action::publish(Message::WaveformPanStarted).and_capture());
            } else if let Some(progress) = self.progress_at_x(
                state.last_pan_view.unwrap_or(self.view),
                bounds.width,
                position.x,
            ) {
                return Some(Action::publish(Message::WaveformSeek(progress)).and_capture());
            }
        }

        if state.pan_anchor.is_some()
            && let Event::Mouse(mouse::Event::CursorMoved { .. }) = event
            && let Some(position) = cursor.position_in(bounds)
            && let Some(anchor) = state.pan_anchor
        {
            let visible = self.view.visible_fraction();
            let last_x = state.last_pan_x.unwrap_or(position.x);
            let step = -(position.x - last_x) / bounds.width * visible;
            state.last_pan_x = Some(position.x);

            let mut view = state.last_pan_view.unwrap_or(WaveFormView {
                zoom: self.view.zoom,
                offset: anchor.view_offset,
                overscroll: anchor.overscroll,
            });
            view.apply_pan_delta(step);
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
            if state.pan_anchor.is_some() {
                return iced::mouse::Interaction::Grabbing;
            }
            if self.modifiers.shift() {
                return iced::mouse::Interaction::Grab;
            }
            return iced::mouse::Interaction::Pointer;
        }
        iced::mouse::Interaction::default()
    }
}
