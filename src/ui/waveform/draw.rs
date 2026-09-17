//! Drawing the waveform: background, amplitude axis, envelope or individual
//! samples, time ruler, and playhead.
//!
//! Zoomed out, each plot column shows the min/max envelope of its samples.
//! Zoomed in to about a sample per pixel, samples are drawn as stems with a
//! Lanczos-interpolated trace through them.

use super::view::{WaveFormView, WaveformLayout};
use super::{PLOT_CLIP_BLEED_LEFT, PlotArea, TIME_MARKER_HEIGHT, WaveForm};
use crate::waveform_peaks::WaveformPeaks;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::canvas::{Frame, Geometry, Gradient, LineCap, LineJoin, Path, Stroke, Text, gradient, path};
use iced::{Color, Pixels, Point, Renderer, Size, Theme, Vector};

/// Share of columns with both lobes at which fills switch to plain two-outline mode, and back.
const TWO_OUTLINE_ENTER: f32 = 0.65;
const TWO_OUTLINE_EXIT: f32 = 0.40;
const MIN_TICK_GAP_PX: f32 = 8.0;
const AMPLITUDE_TICKS: [f32; 5] = [1.0, 0.5, 0.0, -0.5, -1.0];
const WAVEFORM_CORNER_RADIUS: f32 = 8.0;
/// Lanczos kernel half-width.
const LANCZOS_A: i32 = 3;

/// Changes when the colors the cached drawing used change.
pub(super) fn theme_cache_key(theme: &Theme) -> u32 {
    let palette = theme.extended_palette();
    let (primary, background) = (palette.primary.base.color, palette.background.base.color);
    [
        primary.r,
        primary.g,
        primary.b,
        background.r,
        background.g,
        background.b,
    ]
    .iter()
    .fold(0, |key, channel| key ^ channel.to_bits())
}

/// One plot column's envelope, in plot coordinates (y grows downward).
struct ColumnSample {
    x: f32,
    stroke_y_min: f32,
    stroke_y_max: f32,
}

/// Above or below the center line.
#[derive(Clone, Copy)]
enum Side {
    Up,
    Down,
}

impl Side {
    fn opposite(self) -> Self {
        match self {
            Side::Up => Side::Down,
            Side::Down => Side::Up,
        }
    }

    /// The column's outline on this side, if its envelope reaches past the center line there.
    fn outline(self, column: &ColumnSample, center: f32) -> Option<f32> {
        match self {
            Side::Up => (column.stroke_y_max + 0.5 < center).then_some(column.stroke_y_max),
            Side::Down => (column.stroke_y_min > center + 0.5).then_some(column.stroke_y_min),
        }
    }
}

struct WaveformPalette {
    background: Color,
    axis: Color,
    fill: Color,
    stroke: Color,
    marker: Color,
    marker_label: Color,
}

impl WaveformPalette {
    fn from_theme(theme: &Theme) -> Self {
        let palette = theme.extended_palette();
        let primary = palette.primary.base.color;
        let base = palette.background.base.color;
        Self {
            background: Color::from_rgb(base.r * 0.76, base.g * 0.76, base.b * 0.78),
            axis: palette.background.strong.color.scale_alpha(0.35),
            fill: primary.scale_alpha(0.22),
            stroke: primary.scale_alpha(0.92),
            marker: palette.background.strong.color.scale_alpha(0.55),
            marker_label: palette.background.base.text.scale_alpha(0.72),
        }
    }
}

impl WaveForm {
    /// Applies the rubber-band stretch while drawing; `map_content_x` mirrors it.
    pub(super) fn with_content_transform(
        frame: &mut Frame,
        view: WaveFormView,
        size: Size,
        sample_count: usize,
        draw: impl FnOnce(&mut Frame),
    ) {
        let (scale_x, scale_y) = view.content_scale();
        let origin = Vector::new(
            view.content_transform_origin_x(size.width, sample_count),
            size.height / 2.0,
        );
        frame.push_transform();
        frame.translate(Vector::new(view.content_translate_x(size.width), 0.0) + origin);
        frame.scale_nonuniform(Vector::new(scale_x, scale_y));
        frame.translate(Vector::new(-origin.x, -origin.y));
        draw(frame);
        frame.pop_transform();
    }

    pub(super) fn with_plot_origin(frame: &mut Frame, plot: PlotArea, draw: impl FnOnce(&mut Frame)) {
        frame.push_transform();
        frame.translate(Vector::new(plot.x, plot.y));
        draw(frame);
        frame.pop_transform();
    }

    pub(super) fn draw_playhead_on_frame(
        &self,
        frame: &mut Frame,
        theme: &Theme,
        size: Size,
        progress: f64,
        view: WaveFormView,
    ) {
        if let Some(x) = self.playhead_content_x(view, size.width, progress) {
            stroke_playhead(frame, x, size.height, theme);
        }
    }

    pub(super) fn draw_playhead(
        &self,
        renderer: &Renderer,
        size: Size,
        theme: &Theme,
        progress: f64,
        view: WaveFormView,
    ) -> Option<Geometry> {
        let plot = PlotArea::from_size(size);
        let x = self.playhead_screen_x(view, plot, progress)?;
        // This layer is drawn unclipped, so an off-window playhead would streak across the
        // amplitude gutter instead of simply being out of view. Matches `plot_clip`, which the
        // transformed path draws through, so the head does not jump when overscroll settles.
        if x < plot.x - PLOT_CLIP_BLEED_LEFT || x > plot.x + plot.width {
            return None;
        }
        let mut frame = Frame::new(renderer, size);
        stroke_playhead(&mut frame, x, size.height, theme);
        Some(frame.into_geometry())
    }

    pub(super) fn draw_background(&self, frame: &mut Frame, theme: &Theme, size: Size) {
        let background = Path::rounded_rectangle(Point::ORIGIN, size, WAVEFORM_CORNER_RADIUS.into());
        frame.fill(&background, WaveformPalette::from_theme(theme).background);
    }

    pub(super) fn draw_amplitude_axis(&self, frame: &mut Frame, theme: &Theme, size: Size) {
        let palette = WaveformPalette::from_theme(theme);
        let plot = PlotArea::from_size(size);

        for amplitude in AMPLITUDE_TICKS {
            let y = plot.amplitude_y(amplitude);
            let grid_color = if amplitude == 0.0 {
                palette.axis
            } else {
                palette.marker.scale_alpha(0.22)
            };
            let grid = Path::line(Point::new(plot.x, y), Point::new(plot.x + plot.width, y));
            frame.stroke(&grid, Stroke::default().with_color(grid_color).with_width(1.0));
            let tick = Path::line(Point::new(plot.x - 4.0, y), Point::new(plot.x, y));
            frame.stroke(&tick, Stroke::default().with_color(palette.marker).with_width(1.0));

            // The end labels sit inside the plot rather than centered on its edges.
            let (align_y, label_y) = if amplitude == 1.0 {
                (Vertical::Top, plot.y)
            } else if amplitude == -1.0 {
                (Vertical::Bottom, plot.y + plot.height)
            } else {
                (Vertical::Center, y)
            };
            frame.fill_text(Text {
                content: if amplitude == 0.0 {
                    "0".into()
                } else {
                    format!("{amplitude:.1}")
                },
                position: Point::new(plot.x - 6.0, label_y),
                color: palette.marker_label,
                size: Pixels(10.0),
                align_x: Horizontal::Right.into(),
                align_y,
                ..Default::default()
            });
        }
    }

    pub(super) fn draw_waveform_content(&self, frame: &mut Frame, theme: &Theme, view: WaveFormView, size: Size) {
        if self.sample_count == 0 || size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let (start, end, phase) = view.sample_window(self.sample_count);
        let window = Window {
            start,
            visible: end.saturating_sub(start),
            phase,
        };
        if window.visible == 0 {
            return;
        }
        let palette = WaveformPalette::from_theme(theme);
        let center = size.height / 2.0;
        let layout = view.waveform_layout(size.width, window.visible);

        if let Ok(peaks) = self.peaks.lock() {
            if layout.sample_point_mode {
                draw_sample_points(frame, &palette, &peaks, window, center, layout);
            } else {
                let columns = columns_from_window(&peaks, center, window, layout);
                if columns.is_empty() {
                    return;
                }
                self.draw_lobe_fills(frame, &columns, layout.column_width, center, palette.fill);
                let stroke = Stroke::default()
                    .with_color(palette.stroke)
                    .with_width(1.0)
                    .with_line_cap(LineCap::Round)
                    .with_line_join(LineJoin::Round);
                let edge = |y: fn(&ColumnSample) -> f32| {
                    polyline(columns.iter().map(|column| Point::new(column.x, y(column))))
                };
                frame.stroke(&edge(|column| column.stroke_y_max), stroke);
                frame.stroke(&edge(|column| column.stroke_y_min), stroke);
            }
        }

        self.draw_time_markers(frame, &palette, size, view);
    }

    /// Fills each lobe from its outline toward the center line. When the whole view has lobes
    /// on only one side, the fill continues past the center as a fading mirror image.
    fn draw_lobe_fills(
        &self,
        frame: &mut Frame,
        columns: &[ColumnSample],
        column_width: f32,
        center: f32,
        fill: Color,
    ) {
        let half = column_width * 0.5;
        let quad = |builder: &mut path::Builder, x: f32, y0: f32, y1: f32| {
            builder.move_to(Point::new(x - half, y0));
            builder.line_to(Point::new(x + half, y0));
            builder.line_to(Point::new(x + half, y1));
            builder.line_to(Point::new(x - half, y1));
            builder.close();
        };

        let two_outline = two_outline_should_arm(two_outline_ratio(columns, center), self.two_outline_armed.get());
        self.two_outline_armed.set(two_outline);
        if two_outline {
            let mut builder = path::Builder::new();
            for column in columns {
                quad(&mut builder, column.x, column.stroke_y_max, column.stroke_y_min);
            }
            frame.fill(&builder.build(), fill);
            return;
        }

        let any_on = |side: Side| columns.iter().any(|column| side.outline(column, center).is_some());
        let allow_mirror = any_on(Side::Up) != any_on(Side::Down);
        let (mut solid, mut mirror) = (path::Builder::new(), path::Builder::new());
        let (mut solid_any, mut mirror_any) = (false, false);
        // The outline farthest from the center; sets where the mirror image fades out.
        let mut peak: Option<f32> = None;

        for column in columns {
            for side in [Side::Up, Side::Down] {
                let Some((outline, end)) = fill_span(column, center, allow_mirror, side) else {
                    continue;
                };
                let solid_end = if allow_mirror { center } else { end };
                if (solid_end - outline).abs() > 0.5 {
                    quad(&mut solid, column.x, outline, solid_end);
                    solid_any = true;
                }
                if allow_mirror {
                    quad(&mut mirror, column.x, center, end);
                    mirror_any = true;
                }
                if peak.is_none_or(|peak| (outline - center).abs() > (peak - center).abs()) {
                    peak = Some(outline);
                }
            }
        }

        if solid_any {
            frame.fill(&solid.build(), fill);
        }
        if mirror_any && let Some(peak) = peak {
            frame.fill(
                &mirror.build(),
                mirror_fill_gradient(fill, center, flip_y(peak, center), center),
            );
        }
    }

    fn draw_time_markers(&self, frame: &mut Frame, palette: &WaveformPalette, size: Size, view: WaveFormView) {
        if self.sample_rate == 0 || self.sample_count == 0 || size.width <= 0.0 {
            return;
        }
        let sample_rate = f64::from(self.sample_rate);
        let (start, end, phase) = view.sample_window(self.sample_count);
        let visible_samples = end.saturating_sub(start);
        if visible_samples == 0 {
            return;
        }

        let px_per_sample = size.width / visible_samples as f32;
        let start_secs = start as f64 / sample_rate;
        let visible_secs = visible_samples as f64 / sample_rate;
        let end_secs = start_secs + visible_secs;
        let major_step = nice_time_step(visible_secs);

        let tick_x = |tick_secs: f64| {
            let sample_pos = (tick_secs - start_secs) / visible_secs * visible_samples as f64;
            (sample_pos as f32 - phase) * px_per_sample
        };
        let mut draw_tick = |x: f32, alpha: f32, label: Option<String>| {
            if !(0.0..=size.width).contains(&x) {
                return;
            }
            let line = Path::line(Point::new(x, 0.0), Point::new(x, size.height));
            frame.stroke(
                &line,
                Stroke::default()
                    .with_color(palette.marker.scale_alpha(alpha))
                    .with_width(1.0),
            );
            if let Some(content) = label {
                frame.fill_text(Text {
                    content,
                    position: Point::new(x, size.height + TIME_MARKER_HEIGHT - 2.0),
                    color: palette.marker_label,
                    size: Pixels(10.0),
                    align_x: Horizontal::Center.into(),
                    align_y: Vertical::Bottom,
                    ..Default::default()
                });
            }
        };
        // Ticks every `step` seconds across the view, skipping those a coarser tier draws.
        let ticks = |step: f64, from: f64| {
            std::iter::successors(Some(from), move |tick| Some(tick + step))
                .take_while(move |tick| *tick <= end_secs + step * 0.001)
        };

        if view.sample_point_mode(size.width, visible_samples) {
            let stride = ((visible_samples as f32 / size.width).ceil() as usize).max(1);
            for offset in (0..visible_samples).step_by(stride) {
                let tick_secs = (start + offset) as f64 / sample_rate;
                if !is_on_time_grid(tick_secs, major_step) && !is_on_time_grid(tick_secs, 1.0) {
                    draw_tick((offset as f32 + 0.5 - phase) * px_per_sample, 0.16, None);
                }
            }
        } else if let Some(minor_step) = minor_time_step(major_step, visible_secs, size.width) {
            for tick in ticks(minor_step, ((start_secs / minor_step).ceil() * minor_step).max(0.0)) {
                if !is_on_time_grid(tick, major_step) && !is_on_time_grid(tick, 1.0) {
                    draw_tick(tick_x(tick), 0.28, None);
                }
            }
        }

        if tick_step_visible(1.0, visible_secs, size.width) {
            for second in ticks(1.0, start_secs.ceil().max(0.0)) {
                if !is_on_time_grid(second, major_step) {
                    draw_tick(tick_x(second), 0.82, None);
                }
            }
        }

        for major in ticks(major_step, (start_secs / major_step).ceil() * major_step) {
            draw_tick(tick_x(major), 1.0, Some(format_time(major, major_step)));
        }
    }
}

fn stroke_playhead(frame: &mut Frame, x: f32, height: f32, theme: &Theme) {
    let accent = theme.extended_palette().primary.base.color;
    let line = Path::line(Point::new(x, 0.0), Point::new(x, height));
    for (color, width) in [(accent, 2.0), (accent.scale_alpha(0.35), 6.0)] {
        frame.stroke(
            &line,
            Stroke::default()
                .with_color(color)
                .with_width(width)
                .with_line_cap(LineCap::Round),
        );
    }
}

fn polyline(points: impl IntoIterator<Item = Point>) -> Path {
    let mut builder = path::Builder::new();
    let mut points = points.into_iter();
    if let Some(first) = points.next() {
        builder.move_to(first);
        points.for_each(|point| builder.line_to(point));
    }
    builder.build()
}

/// The samples on screen: the first one, how many, and the sub-sample offset.
#[derive(Clone, Copy)]
struct Window {
    start: usize,
    visible: usize,
    phase: f32,
}

fn columns_from_window(
    peaks: &WaveformPeaks,
    center: f32,
    window: Window,
    layout: WaveformLayout,
) -> Vec<ColumnSample> {
    debug_assert!(layout.column_count <= layout.width.ceil() as usize);
    let x_shift = -window.phase * layout.px_per_sample;
    (0..layout.column_count)
        .map(|col| (col, col * layout.samples_per_col))
        .take_while(|&(_, chunk_start)| chunk_start < window.visible)
        .map(|(col, chunk_start)| {
            let chunk_end = (chunk_start + layout.samples_per_col).min(window.visible);
            let (min_sample, max_sample) =
                peaks.extents_for_range(window.start + chunk_start, window.start + chunk_end);
            ColumnSample {
                x: (col as f32 + 0.5) * layout.column_width + x_shift,
                stroke_y_min: center - min_sample * center,
                stroke_y_max: center - max_sample * center,
            }
        })
        .collect()
}

fn draw_sample_points(
    frame: &mut Frame,
    palette: &WaveformPalette,
    peaks: &WaveformPeaks,
    window: Window,
    center: f32,
    layout: WaveformLayout,
) {
    let Window {
        start,
        visible: visible_count,
        phase,
    } = window;
    let px_per_sample = layout.px_per_sample;
    if px_per_sample <= 0.0 || !px_per_sample.is_finite() {
        return;
    }
    let sample_y = |sample: f32| center - sample.clamp(-1.0, 1.0) * center;
    let sample_point = |index: usize| {
        let x = (index as f32 + 0.5 - phase) * px_per_sample;
        Point::new(x, sample_y(peaks.midpoint_at(start + index)))
    };

    let mut stems = path::Builder::new();
    for point in (0..visible_count).map(sample_point) {
        if (point.y - center).abs() > 0.35 {
            stems.move_to(Point::new(point.x, center));
            stems.line_to(point);
        }
    }
    frame.stroke(
        &stems.build(),
        Stroke::default()
            .with_color(palette.stroke.scale_alpha(0.55))
            .with_width(1.0)
            .with_line_cap(LineCap::Round),
    );

    // With room between samples, draw a smooth trace through them; otherwise join the dots.
    let trace = if px_per_sample >= 1.5 {
        let pixels = layout.width.ceil().max(1.0) as usize;
        polyline((0..pixels).map(|px| {
            let x = px as f32 + 0.5;
            let t = sample_index_at_x(x, start, phase, px_per_sample);
            Point::new(x, sample_y(interpolate_peak_at(peaks, t)))
        }))
    } else {
        polyline((0..visible_count).map(sample_point))
    };
    frame.stroke(
        &trace,
        Stroke::default()
            .with_color(palette.stroke.scale_alpha(0.92))
            .with_width(1.25)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    );
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
        sinc_pi(x) * sinc_pi(x / a)
    }
}

/// Lanczos-3 at index `t` over `len` samples read through `sample`;
/// zero-extended outside `[0, len)`.
fn lanczos_at(len: usize, t: f64, sample: impl Fn(usize) -> f32) -> f32 {
    if len == 0 || !t.is_finite() {
        return 0.0;
    }
    let a = i64::from(LANCZOS_A);
    let t = t.clamp(-a as f64, len as f64 + a as f64);
    let center = t.floor() as i64;
    let first = (center - a + 1).max(0);
    let last = (center + a).min(len as i64 - 1);
    (first..=last)
        .map(|i| f64::from(sample(i as usize)) * lanczos3(t - i as f64))
        .sum::<f64>() as f32
}

/// Lanczos-3 at mono frame index `t` using peak midpoints.
fn interpolate_peak_at(peaks: &WaveformPeaks, t: f64) -> f32 {
    lanczos_at(peaks.sample_count, t, |i| peaks.midpoint_at(i))
}

/// The fractional sample index under plot x, matching where stems are drawn.
fn sample_index_at_x(x: f32, start: usize, phase: f32, px_per_sample: f32) -> f64 {
    if px_per_sample <= 0.0 || !px_per_sample.is_finite() {
        return start as f64;
    }
    start as f64 - 0.5 + f64::from(phase) + f64::from(x) / f64::from(px_per_sample)
}

/// Fades from `fill` at `outline_y` to clear toward `mirror_y`.
fn mirror_fill_gradient(fill: Color, outline_y: f32, mirror_y: f32, center: f32) -> Gradient {
    let clear = Color { a: 0.0, ..fill };
    let delta = mirror_y - outline_y;
    // A gradient needs distinct end points; nudge a degenerate one away from the mirror side.
    let outline_y = if delta.abs() < 1.0 {
        let direction = [delta, center - outline_y]
            .into_iter()
            .find(|d| *d != 0.0)
            .map_or(1.0, f32::signum);
        outline_y - direction
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

/// The `(outline, end)` y range to fill for one side of a column, if that side has a lobe.
/// Fills stop at the center line, or mirror past it when `allow_mirror` and the other side is empty.
fn fill_span(column: &ColumnSample, center: f32, allow_mirror: bool, side: Side) -> Option<(f32, f32)> {
    let outline = side.outline(column, center)?;
    let mirrors = allow_mirror && side.opposite().outline(column, center).is_none();
    let end = if mirrors { flip_y(outline, center) } else { center };
    ((end - outline).abs() > 0.5).then_some((outline, end))
}

fn two_outline_ratio(columns: &[ColumnSample], center: f32) -> f32 {
    if columns.is_empty() {
        return 0.0;
    }
    let bipolar = columns
        .iter()
        .filter(|column| Side::Up.outline(column, center).is_some() && Side::Down.outline(column, center).is_some())
        .count();
    bipolar as f32 / columns.len() as f32
}

/// Hysteresis, so the fill style does not flicker while panning across a threshold.
fn two_outline_should_arm(ratio: f32, armed: bool) -> bool {
    ratio >= if armed { TWO_OUTLINE_EXIT } else { TWO_OUTLINE_ENTER }
}

/// A 1-2-5 step giving about eight labeled ticks across the view.
fn nice_time_step(visible_secs: f64) -> f64 {
    if visible_secs <= 0.0 {
        return 1.0;
    }
    let raw = visible_secs / 8.0;
    let magnitude = 10_f64.powf(raw.log10().floor());
    let nice = [1.0, 2.0, 5.0]
        .into_iter()
        .find(|nice| raw / magnitude <= *nice)
        .unwrap_or(10.0);
    (nice * magnitude).max(0.001)
}

fn tick_spacing_px(step_secs: f64, visible_secs: f64, width: f32) -> f32 {
    if step_secs <= 0.0 || visible_secs <= 0.0 || width <= 0.0 {
        return 0.0;
    }
    (step_secs as f32 / visible_secs as f32) * width
}

fn tick_step_visible(step_secs: f64, visible_secs: f64, width: f32) -> bool {
    tick_spacing_px(step_secs, visible_secs, width) >= MIN_TICK_GAP_PX
}

/// The finest unlabeled tick step below `major_step` that still leaves a visible gap.
fn minor_time_step(major_step: f64, visible_secs: f64, width: f32) -> Option<f64> {
    const CANDIDATES: [f64; 11] = [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0];
    CANDIDATES
        .into_iter()
        .find(|&step| step < major_step && tick_step_visible(step, visible_secs, width))
}

/// A ruler label: `h:mm:ss`, `m:ss`, or `s` with as many decimals as `step` needs.
fn format_time(secs: f64, step: f64) -> String {
    let secs = secs.max(0.0);
    let decimals = [1.0, 0.1, 0.01]
        .into_iter()
        .position(|limit| step >= limit)
        .unwrap_or(3);
    let width = if decimals == 0 { 2 } else { decimals + 3 };
    let hours = (secs / 3600.0).floor() as u32;
    let minutes = ((secs % 3600.0) / 60.0).floor() as u32;
    let seconds = secs % 60.0;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:0width$.decimals$}")
    } else if minutes > 0 {
        format!("{minutes}:{seconds:0width$.decimals$}")
    } else {
        format!("{seconds:.decimals$}s")
    }
}

fn is_on_time_grid(tick_secs: f64, step: f64) -> bool {
    if step <= 0.0 {
        return false;
    }
    let remainder = tick_secs.rem_euclid(step);
    remainder < step * 0.05 || (step - remainder) < step * 0.05
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(stroke_y_max: f32, stroke_y_min: f32) -> ColumnSample {
        ColumnSample {
            x: 0.0,
            stroke_y_min,
            stroke_y_max,
        }
    }

    fn interpolate_at(samples: &[f32], t: f64) -> f32 {
        lanczos_at(samples.len(), t, |i| samples[i])
    }

    #[test]
    fn far_zoom_hides_subsecond_and_second_ticks() {
        let (visible_secs, width) = (200.0, 800.0);
        assert!(!tick_step_visible(0.1, visible_secs, width));
        assert!(!tick_step_visible(1.0, visible_secs, width));
        assert!(tick_step_visible(10.0, visible_secs, width));
        assert!(tick_spacing_px(1.0, visible_secs, width) < MIN_TICK_GAP_PX);
    }

    #[test]
    fn close_zoom_keeps_subsecond_ticks() {
        let (visible_secs, width) = (2.0, 800.0);
        assert!(tick_step_visible(0.1, visible_secs, width));
        let minor = minor_time_step(1.0, visible_secs, width).expect("close zoom should keep subsecond ticks");
        assert!(minor < 1.0);
        assert!(tick_step_visible(minor, visible_secs, width));
    }

    #[test]
    fn far_zoom_picks_minor_step_that_still_has_gap() {
        let (visible_secs, width) = (200.0, 800.0);
        let major = nice_time_step(visible_secs);
        let minor =
            minor_time_step(major, visible_secs, width).expect("far zoom should still have a sparse minor grid");
        assert!(minor < major);
        assert!(tick_step_visible(minor, visible_secs, width));
        assert!(minor >= 1.0, "blended 0.1s/1s ticks must not be chosen");
    }

    #[test]
    fn time_labels_carry_the_step_precision() {
        assert_eq!(format_time(12.0, 2.0), "12s");
        assert_eq!(format_time(75.26, 0.1), "1:15.3");
        assert_eq!(format_time(3_725.0, 5.0), "1:02:05");
        assert_eq!(format_time(61.004, 0.005), "1:01.004");
        assert_eq!(nice_time_step(8.0), 1.0);
        assert_eq!(nice_time_step(24.0), 5.0);
    }

    #[test]
    fn flip_y_mirrors_across_center() {
        assert!((flip_y(25.0, 100.0) - 175.0).abs() < f32::EPSILON);
        assert!((flip_y(100.0, 100.0) - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn up_lobe_mirrors_below_and_down_lobe_mirrors_above() {
        let center = 100.0;
        let up = column(20.0, center);
        assert_eq!(fill_span(&up, center, true, Side::Up), Some((20.0, 180.0)));
        assert_eq!(fill_span(&up, center, true, Side::Down), None);

        let down = column(center, 180.0);
        assert_eq!(fill_span(&down, center, true, Side::Down), Some((180.0, 20.0)));
        assert_eq!(fill_span(&down, center, true, Side::Up), None);
    }

    #[test]
    fn bipolar_column_fills_each_lobe_only_to_the_center() {
        let center = 100.0;
        let bipolar = column(20.0, 160.0);
        assert_eq!(fill_span(&bipolar, center, true, Side::Up), Some((20.0, center)));
        assert_eq!(fill_span(&bipolar, center, true, Side::Down), Some((160.0, center)));
    }

    #[test]
    fn mixed_sides_clip_fill_at_center() {
        assert_eq!(
            fill_span(&column(20.0, 100.0), 100.0, false, Side::Up),
            Some((20.0, 100.0))
        );
    }

    #[test]
    fn two_outline_view_uses_solid_fill_not_gradient() {
        let columns = [column(20.0, 160.0), column(40.0, 150.0)];
        assert!(two_outline_should_arm(two_outline_ratio(&columns, 100.0), false));
    }

    #[test]
    fn two_outline_fill_uses_hysteresis() {
        assert!(!two_outline_should_arm(0.50, false));
        assert!(two_outline_should_arm(0.50, true));
        assert!(two_outline_should_arm(0.70, false));
        assert!(!two_outline_should_arm(0.30, true));
    }

    #[test]
    fn gradient_is_opaque_at_outline_and_clear_at_mirror() {
        let color = Color::from_rgb(0.2, 0.4, 1.0).scale_alpha(0.22);
        let Gradient::Linear(linear) = mirror_fill_gradient(color, 20.0, 180.0, 100.0);
        assert!((linear.start.y - 20.0).abs() < f32::EPSILON);
        assert!((linear.end.y - 180.0).abs() < f32::EPSILON);
        let stops: Vec<_> = linear.stops.iter().flatten().collect();
        assert!((stops[0].color.a - 0.22).abs() < f32::EPSILON);
        assert_eq!(stops.last().map(|stop| stop.color.a), Some(0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.5 && stop.color.a > 0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.85 && stop.color.a == 0.0));
    }

    #[test]
    fn lanczos3_is_one_at_origin_and_zero_outside_window() {
        assert!((lanczos3(0.0) - 1.0).abs() < 1e-12);
        assert_eq!(lanczos3(3.0), 0.0);
        assert_eq!(lanczos3(-3.0), 0.0);
        assert_eq!(lanczos3(4.0), 0.0);
        assert_eq!(lanczos3(f64::NAN), 0.0);
    }

    #[test]
    fn interpolate_at_reconstructs_integer_samples() {
        let samples = [0.0, 0.5, -0.25, 1.0];
        for (index, &sample) in samples.iter().enumerate() {
            let value = interpolate_at(&samples, index as f64);
            assert!((value - sample).abs() < 1e-6, "t={index}: got {value}, want {sample}");
        }
    }

    #[test]
    fn trace_meets_each_stem_at_its_own_x() {
        let sample_count = 64;
        let mut peaks = WaveformPeaks::new(sample_count);
        let n_buckets = crate::waveform_peaks::PEAK_BUCKET_COUNT;
        for index in 0..sample_count {
            let bucket = index * n_buckets / sample_count;
            let value = (index as f32 * 0.37).sin();
            peaks.min[bucket] = value;
            peaks.max[bucket] = value;
        }

        let (start, end) = (10, 30);
        let px_per_sample = 8.0_f32;
        let phase = 0.25_f32;
        for index in 0..(end - start) {
            let stem = peaks.midpoint_at(start + index);
            let x = (index as f32 + 0.5 - phase) * px_per_sample;
            let trace = interpolate_peak_at(&peaks, sample_index_at_x(x, start, phase, px_per_sample));
            assert!(
                (trace - stem).abs() < 1e-5,
                "sample {index} at x {x}: stem {stem}, trace {trace}"
            );
        }
    }

    #[test]
    fn interpolate_at_is_safe_at_boundaries() {
        assert_eq!(interpolate_at(&[], 0.0), 0.0);
        assert_eq!(interpolate_at(&[0.8], f64::NAN), 0.0);
        let edge = interpolate_at(&[1.0, 0.0, -1.0], -1.5);
        let past = interpolate_at(&[1.0, 0.0, -1.0], 8.0);
        assert!(edge.is_finite());
        assert!(past.is_finite());
        assert!(past.abs() < 1e-6);
        assert_eq!(interpolate_at(&[1.0, 0.0, -1.0], 1e300), 0.0);
    }

    #[test]
    fn sample_index_at_x_matches_stem_centers() {
        let (start, phase, px) = (10usize, 0.25_f32, 12.0_f32);
        let x = (2.0 + 0.5) * px - phase * px;
        assert!((sample_index_at_x(x, start, phase, px) - (start + 2) as f64).abs() < 1e-6);
    }
}
