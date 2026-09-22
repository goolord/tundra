//! Drawing the waveform: background, amplitude axis, envelope or individual
//! samples, time ruler, and playhead.
//!
//! Zoomed out, each plot column shows the min/max envelope of its samples.
//! Zoomed in to about a sample per pixel, samples are drawn as stems with a
//! Lanczos-interpolated trace through them.

use super::view::{WaveFormView, WaveformLayout};
use super::{PlotArea, TIME_MARKER_HEIGHT, WaveForm};
use crate::waveform_peaks::WaveformPeaks;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::canvas::{Frame, Gradient, LineCap, LineJoin, Path, Stroke, Text, gradient, path};
use iced::{Color, Pixels, Point, Size, Theme};

/// Share of columns with both lobes at which fills switch to plain two-outline mode, and back.
const TWO_OUTLINE_ENTER: f32 = 0.65;
const TWO_OUTLINE_EXIT: f32 = 0.40;
const MIN_TICK_GAP_PX: f32 = 8.0;
const WAVEFORM_CORNER_RADIUS: f32 = 8.0;

/// The samples on screen: the first one, how many, and the sub-sample offset.
type Window = (usize, usize, f32);

/// Changes when the colors the cached drawing used change.
pub(super) fn theme_cache_key(theme: &Theme) -> u32 {
    let palette = theme.extended_palette();
    let (p, b) = (palette.primary.base.color, palette.background.base.color);
    [p.r, p.g, p.b, b.r, b.g, b.b].iter().fold(0, |key, channel| key ^ channel.to_bits())
}

/// One plot column's envelope, in plot coordinates (y grows downward, so `top` is the maximum).
struct Column {
    x: f32,
    top: f32,
    bottom: f32,
}

impl Column {
    /// The outline above (`up`) or below the center line, if the envelope reaches past it there.
    fn lobe(&self, center: f32, up: bool) -> Option<f32> {
        if up {
            (self.top + 0.5 < center).then_some(self.top)
        } else {
            (self.bottom > center + 0.5).then_some(self.bottom)
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
        let (primary, base) = (palette.primary.base.color, palette.background.base.color);
        Self {
            background: Color::from_rgb(base.r * 0.76, base.g * 0.76, base.b * 0.78),
            axis: palette.background.strong.color.scale_alpha(0.35),
            fill: primary.scale_alpha(0.22),
            stroke: primary.scale_alpha(0.92),
            marker: palette.background.strong.color.scale_alpha(0.55),
            marker_label: palette.background.base.text.scale_alpha(0.72),
        }
    }

    fn label(&self, frame: &mut Frame, content: String, position: Point, align_x: Horizontal, align_y: Vertical) {
        frame.fill_text(Text {
            content,
            position,
            color: self.marker_label,
            size: Pixels(10.0),
            align_x: align_x.into(),
            align_y,
            ..Default::default()
        });
    }
}

fn line(color: Color, width: f32) -> Stroke<'static> {
    Stroke::default().with_color(color).with_width(width)
}

/// Draws the rounded background and the amplitude axis in the gutter.
pub(super) fn draw_background(frame: &mut Frame, theme: &Theme, size: Size) {
    let palette = WaveformPalette::from_theme(theme);
    let background = Path::rounded_rectangle(Point::ORIGIN, size, WAVEFORM_CORNER_RADIUS.into());
    frame.fill(&background, palette.background);
    let plot = PlotArea::from_size(size);
    // The end labels sit inside the plot rather than centered on its edges.
    for (amplitude, text, align_y) in [
        (1.0, "1.0", Vertical::Top),
        (0.5, "0.5", Vertical::Center),
        (0.0, "0", Vertical::Center),
        (-0.5, "-0.5", Vertical::Center),
        (-1.0, "-1.0", Vertical::Bottom),
    ] {
        let y = plot.amplitude_y(amplitude);
        let grid = if amplitude == 0.0 { palette.axis } else { palette.marker.scale_alpha(0.22) };
        let hline = |x0, x1| Path::line(Point::new(x0, y), Point::new(x1, y));
        frame.stroke(&hline(plot.x, plot.x + plot.width), line(grid, 1.0));
        frame.stroke(&hline(plot.x - 4.0, plot.x), line(palette.marker, 1.0));
        palette.label(frame, text.into(), Point::new(plot.x - 6.0, y), Horizontal::Right, align_y);
    }
}

pub(super) fn stroke_playhead(frame: &mut Frame, x: f32, height: f32, theme: &Theme) {
    let accent = theme.extended_palette().primary.base.color;
    let playhead = Path::line(Point::new(x, 0.0), Point::new(x, height));
    for (color, width) in [(accent, 2.0), (accent.scale_alpha(0.35), 6.0)] {
        frame.stroke(&playhead, line(color, width).with_line_cap(LineCap::Round));
    }
}

impl WaveForm {
    pub(super) fn draw_waveform_content(&self, frame: &mut Frame, theme: &Theme, view: WaveFormView, size: Size) {
        let window = view.sample_window(self.sample_count());
        if window.1 == 0 || size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let palette = WaveformPalette::from_theme(theme);
        let center = size.height / 2.0;
        let layout = WaveformLayout::new(size.width, window.1);

        if let Ok(peaks) = self.peaks.lock() {
            if layout.sample_point_mode {
                draw_sample_points(frame, &palette, &peaks, window, center, layout);
            } else {
                let columns = columns_from_window(&peaks, center, window, layout);
                self.draw_lobe_fills(frame, &columns, layout.column_width, center, palette.fill);
                let stroke = line(palette.stroke, 1.0).with_line_cap(LineCap::Round).with_line_join(LineJoin::Round);
                frame.stroke(&polyline(columns.iter().map(|c| Point::new(c.x, c.top))), stroke);
                frame.stroke(&polyline(columns.iter().map(|c| Point::new(c.x, c.bottom))), stroke);
            }
        }
        self.draw_time_markers(frame, &palette, size, window, layout);
    }

    /// Fills each lobe from its outline toward the center line. When the whole view has lobes
    /// on only one side, the fill continues past the center as a fading mirror image.
    fn draw_lobe_fills(&self, frame: &mut Frame, columns: &[Column], column_width: f32, center: f32, fill: Color) {
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
                quad(&mut builder, column.x, column.top, column.bottom);
            }
            frame.fill(&builder.build(), fill);
            return;
        }

        let any_on = |up: bool| columns.iter().any(|column| column.lobe(center, up).is_some());
        let allow_mirror = any_on(true) != any_on(false);
        let (mut solid, mut mirror) = (path::Builder::new(), path::Builder::new());
        // The outline farthest from the center; sets where the mirror image fades out.
        let mut peak: Option<f32> = None;
        for column in columns {
            for outline in [true, false].into_iter().filter_map(|up| column.lobe(center, up)) {
                quad(&mut solid, column.x, outline, center);
                if allow_mirror {
                    quad(&mut mirror, column.x, center, flip_y(outline, center));
                }
                if peak.is_none_or(|peak| (outline - center).abs() > (peak - center).abs()) {
                    peak = Some(outline);
                }
            }
        }

        let Some(peak) = peak else {
            return;
        };
        frame.fill(&solid.build(), fill);
        if allow_mirror {
            let gradient = mirror_fill_gradient(fill, center, flip_y(peak, center), center);
            frame.fill(&mirror.build(), gradient);
        }
    }

    fn draw_time_markers(
        &self,
        frame: &mut Frame,
        palette: &WaveformPalette,
        size: Size,
        (start, visible, phase): Window,
        layout: WaveformLayout,
    ) {
        if self.sample_rate == 0 {
            return;
        }
        let sample_rate = f64::from(self.sample_rate);
        let px_per_sample = layout.px_per_sample;
        let start_secs = start as f64 / sample_rate;
        let visible_secs = visible as f64 / sample_rate;
        let end_secs = start_secs + visible_secs;
        let major_step = nice_time_step(visible_secs);

        let tick_x = |tick_secs: f64| {
            let sample_pos = (tick_secs - start_secs) / visible_secs * visible as f64;
            (sample_pos as f32 - phase) * px_per_sample
        };
        let mut draw_tick = |x: f32, alpha: f32, text: Option<String>| {
            if !(0.0..=size.width).contains(&x) {
                return;
            }
            let tick = Path::line(Point::new(x, 0.0), Point::new(x, size.height));
            frame.stroke(&tick, line(palette.marker.scale_alpha(alpha), 1.0));
            if let Some(text) = text {
                let position = Point::new(x, size.height + TIME_MARKER_HEIGHT - 2.0);
                palette.label(frame, text, position, Horizontal::Center, Vertical::Bottom);
            }
        };
        // Ticks every `step` seconds across the view, skipping those a coarser tier draws.
        let ticks = |step: f64, from: f64| {
            std::iter::successors(Some(from), move |tick| Some(tick + step))
                .take_while(move |tick| *tick <= end_secs + step * 0.001)
        };
        let unlabeled = |tick: f64| !is_on_time_grid(tick, major_step) && !is_on_time_grid(tick, 1.0);

        if layout.sample_point_mode {
            let stride = ((visible as f32 / size.width).ceil() as usize).max(1);
            for offset in (0..visible).step_by(stride) {
                if unlabeled((start + offset) as f64 / sample_rate) {
                    draw_tick((offset as f32 + 0.5 - phase) * px_per_sample, 0.16, None);
                }
            }
        } else if let Some(minor_step) = minor_time_step(major_step, visible_secs, size.width) {
            for tick in ticks(minor_step, ((start_secs / minor_step).ceil() * minor_step).max(0.0)) {
                if unlabeled(tick) {
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

fn polyline(points: impl IntoIterator<Item = Point>) -> Path {
    let mut builder = path::Builder::new();
    let mut points = points.into_iter();
    if let Some(first) = points.next() {
        builder.move_to(first);
        points.for_each(|point| builder.line_to(point));
    }
    builder.build()
}

fn columns_from_window(
    peaks: &WaveformPeaks,
    center: f32,
    (start, visible, phase): Window,
    layout: WaveformLayout,
) -> Vec<Column> {
    let x_shift = -phase * layout.px_per_sample;
    (0..layout.column_count)
        .map(|col| {
            let chunk_start = col * layout.samples_per_col;
            let chunk_end = (chunk_start + layout.samples_per_col).min(visible);
            let (min, max) = peaks.extents_for_range(start + chunk_start, start + chunk_end);
            Column {
                x: (col as f32 + 0.5) * layout.column_width + x_shift,
                top: center - max * center,
                bottom: center - min * center,
            }
        })
        .collect()
}

fn draw_sample_points(
    frame: &mut Frame,
    palette: &WaveformPalette,
    peaks: &WaveformPeaks,
    (start, visible, phase): Window,
    center: f32,
    layout: WaveformLayout,
) {
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
    for point in (0..visible).map(sample_point) {
        if (point.y - center).abs() > 0.35 {
            stems.move_to(Point::new(point.x, center));
            stems.line_to(point);
        }
    }
    let stem_stroke = line(palette.stroke.scale_alpha(0.55), 1.0).with_line_cap(LineCap::Round);
    frame.stroke(&stems.build(), stem_stroke);

    // With room between samples, draw a smooth trace through them; otherwise join the dots.
    let trace = if px_per_sample >= 1.5 {
        let pixels = layout.width.ceil().max(1.0) as usize;
        polyline((0..pixels).map(|px| {
            let x = px as f32 + 0.5;
            let t = sample_index_at_x(x, start, phase, px_per_sample);
            Point::new(x, sample_y(interpolate_peak_at(peaks, t)))
        }))
    } else {
        polyline((0..visible).map(sample_point))
    };
    let trace_stroke =
        line(palette.stroke.scale_alpha(0.92), 1.25).with_line_cap(LineCap::Round).with_line_join(LineJoin::Round);
    frame.stroke(&trace, trace_stroke);
}

fn flip_y(y: f32, center: f32) -> f32 {
    2.0 * center - y
}

/// The Lanczos-3 kernel, `sinc(x) sinc(x/3)` for `|x| < 3`.
fn lanczos3(x: f64) -> f64 {
    let sinc = |x: f64| {
        let pix = std::f64::consts::PI * x;
        if x.abs() < 1e-12 { 1.0 } else { pix.sin() / pix }
    };
    if x.abs() < 3.0 { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// Lanczos-3 at index `t` over `len` samples read through `sample`;
/// zero-extended outside `[0, len)`.
fn lanczos_at(len: usize, t: f64, sample: impl Fn(usize) -> f32) -> f32 {
    if len == 0 || !t.is_finite() {
        return 0.0;
    }
    let t = t.clamp(-3.0, len as f64 + 3.0);
    let center = t.floor() as i64;
    let (first, last) = ((center - 2).max(0), (center + 3).min(len as i64 - 1));
    (first..=last).map(|i| f64::from(sample(i as usize)) * lanczos3(t - i as f64)).sum::<f64>() as f32
}

/// Lanczos-3 at mono frame index `t` using peak midpoints.
fn interpolate_peak_at(peaks: &WaveformPeaks, t: f64) -> f32 {
    lanczos_at(peaks.sample_count, t, |i| peaks.midpoint_at(i))
}

/// The fractional sample index under plot x, matching where stems are drawn.
fn sample_index_at_x(x: f32, start: usize, phase: f32, px_per_sample: f32) -> f64 {
    start as f64 - 0.5 + f64::from(phase) + f64::from(x) / f64::from(px_per_sample)
}

/// Fades from `fill` at `outline_y` to clear toward `mirror_y`.
fn mirror_fill_gradient(fill: Color, outline_y: f32, mirror_y: f32, center: f32) -> Gradient {
    let clear = Color { a: 0.0, ..fill };
    let delta = mirror_y - outline_y;
    // A gradient needs distinct end points; nudge a degenerate one away from the mirror side.
    let outline_y = if delta.abs() < 1.0 {
        let direction = [delta, center - outline_y].into_iter().find(|d| *d != 0.0).map_or(1.0, f32::signum);
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

fn two_outline_ratio(columns: &[Column], center: f32) -> f32 {
    let bipolar = columns
        .iter()
        .filter(|column| column.lobe(center, true).is_some() && column.lobe(center, false).is_some())
        .count();
    bipolar as f32 / columns.len().max(1) as f32
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
    let nice = [1.0, 2.0, 5.0].into_iter().find(|nice| raw / magnitude <= *nice).unwrap_or(10.0);
    (nice * magnitude).max(0.001)
}

fn tick_step_visible(step_secs: f64, visible_secs: f64, width: f32) -> bool {
    visible_secs > 0.0 && step_secs > 0.0 && (step_secs as f32 / visible_secs as f32) * width >= MIN_TICK_GAP_PX
}

/// The finest unlabeled tick step below `major_step` that still leaves a visible gap.
fn minor_time_step(major_step: f64, visible_secs: f64, width: f32) -> Option<f64> {
    [0.01, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
        .into_iter()
        .find(|&step| step < major_step && tick_step_visible(step, visible_secs, width))
}

/// A ruler label: `h:mm:ss`, `m:ss`, or `s` with as many decimals as `step` needs.
fn format_time(secs: f64, step: f64) -> String {
    let secs = secs.max(0.0);
    let decimals = [1.0, 0.1, 0.01].into_iter().position(|limit| step >= limit).unwrap_or(3);
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
    let remainder = tick_secs.rem_euclid(step);
    step > 0.0 && (remainder < step * 0.05 || (step - remainder) < step * 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(top: f32, bottom: f32) -> Column {
        Column { x: 0.0, top, bottom }
    }

    fn interpolate_at(samples: &[f32], t: f64) -> f32 {
        lanczos_at(samples.len(), t, |i| samples[i])
    }

    #[test]
    fn tick_density_follows_zoom() {
        // Far out, sub-second and second ticks would blur together; the minor grid stays sparse.
        let (far, width) = (200.0, 800.0);
        assert!(!tick_step_visible(0.1, far, width) && !tick_step_visible(1.0, far, width));
        assert!(tick_step_visible(10.0, far, width));
        let major = nice_time_step(far);
        let minor = minor_time_step(major, far, width).expect("far zoom should still have a sparse minor grid");
        assert!(minor < major && minor >= 1.0 && tick_step_visible(minor, far, width));
        // Close in, sub-second ticks show.
        let minor = minor_time_step(1.0, 2.0, width).expect("close zoom should keep subsecond ticks");
        assert!(minor < 1.0 && tick_step_visible(minor, 2.0, width));
        assert!(!tick_step_visible(1.0, 0.0, width));
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
    fn lobes_reach_past_the_center_line() {
        let center = 100.0;
        assert_eq!(flip_y(25.0, center), 175.0);
        for (column, up, down) in [
            (column(20.0, center), Some(20.0), None),
            (column(center, 180.0), None, Some(180.0)),
            (column(20.0, 160.0), Some(20.0), Some(160.0)),
            (column(99.8, 100.2), None, None),
        ] {
            assert_eq!((column.lobe(center, true), column.lobe(center, false)), (up, down));
        }
    }

    #[test]
    fn two_outline_fill_uses_hysteresis() {
        let bipolar = [column(20.0, 160.0), column(40.0, 150.0)];
        assert!(two_outline_should_arm(two_outline_ratio(&bipolar, 100.0), false));
        assert!(!two_outline_should_arm(0.50, false));
        assert!(two_outline_should_arm(0.50, true));
        assert!(two_outline_should_arm(0.70, false));
        assert!(!two_outline_should_arm(0.30, true));
    }

    #[test]
    fn gradient_is_opaque_at_outline_and_clear_at_mirror() {
        let color = Color::from_rgb(0.2, 0.4, 1.0).scale_alpha(0.22);
        let Gradient::Linear(linear) = mirror_fill_gradient(color, 20.0, 180.0, 100.0);
        assert_eq!((linear.start.y, linear.end.y), (20.0, 180.0));
        let stops: Vec<_> = linear.stops.iter().flatten().collect();
        assert!((stops[0].color.a - 0.22).abs() < f32::EPSILON);
        assert_eq!(stops.last().map(|stop| stop.color.a), Some(0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.5 && stop.color.a > 0.0));
        assert!(stops.iter().any(|stop| stop.offset >= 0.85 && stop.color.a == 0.0));
    }

    #[test]
    fn lanczos_reconstructs_samples_and_is_safe_at_boundaries() {
        assert!((lanczos3(0.0) - 1.0).abs() < 1e-12);
        for x in [3.0, -3.0, 4.0, f64::NAN] {
            assert_eq!(lanczos3(x), 0.0);
        }
        let samples = [0.0, 0.5, -0.25, 1.0];
        for (index, &sample) in samples.iter().enumerate() {
            let value = interpolate_at(&samples, index as f64);
            assert!((value - sample).abs() < 1e-6, "t={index}: got {value}, want {sample}");
        }
        assert_eq!(interpolate_at(&[], 0.0), 0.0);
        assert_eq!(interpolate_at(&[0.8], f64::NAN), 0.0);
        assert!(interpolate_at(&[1.0, 0.0, -1.0], -1.5).is_finite());
        assert!(interpolate_at(&[1.0, 0.0, -1.0], 8.0).abs() < 1e-6);
        assert_eq!(interpolate_at(&[1.0, 0.0, -1.0], 1e300), 0.0);
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

        let (start, px_per_sample, phase) = (10, 8.0_f32, 0.25_f32);
        for index in 0..20 {
            let stem = peaks.midpoint_at(start + index);
            let x = (index as f32 + 0.5 - phase) * px_per_sample;
            let t = sample_index_at_x(x, start, phase, px_per_sample);
            assert!((t - (start + index) as f64).abs() < 1e-6, "sample {index}: index {t}");
            let trace = interpolate_peak_at(&peaks, t);
            assert!((trace - stem).abs() < 1e-5, "sample {index} at x {x}: stem {stem}, trace {trace}");
        }
    }
}
