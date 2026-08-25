pub use super::common::*;

use iced::keyboard::Key;
use iced::mouse::{self, Cursor, ScrollDelta};
use iced::widget::canvas::Action;
use iced::widget::canvas::*;
use iced::border::Radius;
use iced::keyboard::Modifiers;
use iced::alignment;
use iced::widget::canvas::Text;
use iced::{Color, Pixels, Point, Rectangle, Renderer, Size, Theme};
use std::sync::Arc;

use crate::source::arc_samples::PlaybackPosition;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 64.0;
const ZOOM_FACTOR: f32 = 1.25;
const MAX_COLUMNS_FACTOR: f32 = 4.0;
const PAN_STEP: f32 = 0.08;
const TIME_MARKER_HEIGHT: f32 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveFormView {
    pub zoom: f32,
    pub offset: f32,
}

impl Default for WaveFormView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: 0.0,
        }
    }
}

impl WaveFormView {
    pub fn zoom_in(&mut self) {
        self.apply_zoom(ZOOM_FACTOR);
    }

    pub fn zoom_out(&mut self) {
        self.apply_zoom(1.0 / ZOOM_FACTOR);
    }

    pub fn apply_wheel(&mut self, delta: ScrollDelta) {
        let lines = match delta {
            ScrollDelta::Lines { y, .. } => y,
            ScrollDelta::Pixels { y, .. } => y / 48.0,
        };
        if lines > 0.0 {
            self.apply_zoom(ZOOM_FACTOR.powf(lines.abs()));
        } else if lines < 0.0 {
            self.apply_zoom((1.0 / ZOOM_FACTOR).powf(lines.abs()));
        }
    }

    pub fn pan(&mut self, delta: f32) {
        if self.zoom <= 1.0 {
            return;
        }
        self.offset = (self.offset + delta * self.visible_fraction()).clamp(0.0, self.max_offset());
    }

    fn apply_zoom(&mut self, factor: f32) {
        let center = self.center_fraction();
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = (center - self.visible_fraction() / 2.0).clamp(0.0, self.max_offset());
    }

    fn visible_fraction(&self) -> f32 {
        if self.zoom >= 1.0 {
            1.0 / self.zoom
        } else {
            1.0
        }
    }

    fn max_offset(&self) -> f32 {
        (1.0 - self.visible_fraction()).max(0.0)
    }

    fn center_fraction(&self) -> f32 {
        self.offset + self.visible_fraction() / 2.0
    }

    fn sample_range(&self, sample_count: usize) -> (usize, usize) {
        if sample_count == 0 {
            return (0, 0);
        }

        let visible = (sample_count as f32 * self.visible_fraction())
            .ceil()
            .clamp(1.0, sample_count as f32) as usize;
        let max_start = sample_count.saturating_sub(visible);
        let start = if max_start == 0 {
            0
        } else {
            (self.offset * max_start as f32).round() as usize
        };
        (start, (start + visible).min(sample_count))
    }

    fn column_count(&self, width: f32, visible_samples: usize) -> usize {
        if visible_samples == 0 || width <= 0.0 {
            return 1;
        }

        if self.zoom < 1.0 {
            return ((width * self.zoom.max(MIN_ZOOM)).ceil() as usize).max(1);
        }

        let max_columns = (width * MAX_COLUMNS_FACTOR).ceil() as usize;
        max_columns.min(visible_samples).max(1)
    }

    pub fn apply_key(view: &mut Self, key: &Key) -> bool {
        match key.as_ref() {
            Key::Character("+") | Key::Character("=") => {
                view.zoom_in();
                true
            }
            Key::Character("-") => {
                view.zoom_out();
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

pub struct WaveFormState;

impl Default for WaveFormState {
    fn default() -> Self {
        WaveFormState
    }
}

pub struct WaveForm {
    pub samples: Vec<f32>,
    view: WaveFormView,
    sample_rate: u32,
    playback_position: Option<Arc<PlaybackPosition>>,
    scrub_progress: Option<f64>,
    modifiers: Modifiers,
    cache: Cache,
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

impl WaveformPalette {
    fn from_theme(theme: &Theme) -> Self {
        let palette = theme.extended_palette();
        let primary = palette.primary.base.color;
        Self {
            background: palette.background.base.color.scale_alpha(0.45),
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
            cache: Cache::new(),
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

    fn scroll_lines(delta: ScrollDelta) -> (f32, f32) {
        match delta {
            ScrollDelta::Lines { x, y } => (x, y),
            ScrollDelta::Pixels { x, y } => (x / 48.0, y / 48.0),
        }
    }

    fn playback_progress(&self) -> Option<f64> {
        if let Some(progress) = self.scrub_progress {
            return Some(progress);
        }
        self.playback_position.as_ref().map(|position| position.progress())
    }

    fn playhead_x(&self, width: f32, progress: f64) -> Option<f32> {
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
        let (start, end) = self.view.sample_range(self.samples.len());
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let fraction = (progress_frame.saturating_sub(start)) as f32 / visible as f32;
        Some(fraction.clamp(0.0, 1.0) * width)
    }

    fn progress_at_x(&self, width: f32, x: f32) -> Option<f64> {
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

        let (start, end) = self.view.sample_range(self.samples.len());
        let visible = end.saturating_sub(start);
        if visible == 0 {
            return None;
        }

        let fraction = (x / width).clamp(0.0, 1.0);
        let progress_frame =
            (start as f32 + fraction * visible as f32).round() as usize;
        Some(progress_frame.min(frame_count.saturating_sub(1)) as f64 / frame_count as f64)
    }

    fn draw_playhead(
        &self,
        renderer: &Renderer,
        size: Size,
        theme: &Theme,
        progress: f64,
    ) -> Option<Geometry> {
        let x = self.playhead_x(size.width, progress)?;
        let palette = theme.extended_palette();
        let accent = palette.primary.base.color;
        let mut frame = Frame::new(renderer, size);
        let line = Path::line(
            Point::new(x, 0.0),
            Point::new(x, size.height),
        );
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
        Some(frame.into_geometry())
    }

    pub fn view_state(&self) -> WaveFormView {
        self.view
    }

    pub fn set_view(&mut self, view: WaveFormView) {
        if self.view != view {
            self.view = view;
            self.cache.clear();
        }
    }

    fn peak_amplitude(chunk: &[f32]) -> f32 {
        chunk
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0f32, |peak, sample| peak.max(sample))
            .clamp(0.0, 1.0)
    }

    fn columns(&self, size: Size) -> (f32, Vec<ColumnSample>) {
        let height = size.height;
        let width = size.width;
        let center = height / 2.0;

        if self.samples.is_empty() || width <= 0.0 || height <= 0.0 {
            return (center, Vec::new());
        }

        let (start, end) = self.view.sample_range(self.samples.len());
        let visible_count = end.saturating_sub(start);
        if visible_count == 0 {
            return (center, Vec::new());
        }

        let columns = self.view.column_count(width, visible_count);
        let bucket = visible_count.div_ceil(columns);
        let column_width = width / columns as f32;
        let slice = &self.samples[start..end];

        let mut out = Vec::with_capacity(columns);
        for col in 0..columns {
            let chunk_start = col * bucket;
            if chunk_start >= visible_count {
                break;
            }
            let chunk_end = (chunk_start + bucket).min(visible_count);
            let peak = Self::peak_amplitude(&slice[chunk_start..chunk_end]);
            let x = (col as f32 + 0.5) * column_width;
            out.push(ColumnSample {
                x,
                y_min: center + peak * center,
                y_max: center - peak * center,
            });
        }

        (center, out)
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

    fn draw_waveform(&self, frame: &mut Frame, theme: &Theme) {
        let palette = WaveformPalette::from_theme(theme);
        let size = frame.size();

        let background = Path::rounded_rectangle(Point::ORIGIN, size, Radius::new(8.0));
        frame.fill(&background, palette.background);

        let (center, columns) = self.columns(size);
        if columns.is_empty() {
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

        frame.fill(&Self::envelope_path(&columns), palette.fill);

        let stroke = Stroke::default()
            .with_color(palette.stroke)
            .with_width(1.0)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round);

        frame.stroke(&Self::envelope_line_path(&columns, true), stroke);
        frame.stroke(&Self::envelope_line_path(&columns, false), stroke);

        self.draw_time_markers(frame, &palette, size);
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
        nice * magnitude
    }

    fn format_time(secs: f64) -> String {
        let total = secs.max(0.0).round() as u64;
        let hours = total / 3600;
        let minutes = (total % 3600) / 60;
        let seconds = total % 60;
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    fn draw_time_markers(&self, frame: &mut Frame, palette: &WaveformPalette, size: Size) {
        if self.sample_rate == 0 || self.samples.is_empty() || size.width <= 0.0 {
            return;
        }

        let sample_rate = self.sample_rate as f64;
        let (start, end) = self.view.sample_range(self.samples.len());
        let visible_samples = end.saturating_sub(start);
        if visible_samples == 0 {
            return;
        }

        let start_secs = start as f64 / sample_rate;
        let visible_secs = visible_samples as f64 / sample_rate;
        let step = Self::nice_time_step(visible_secs);
        let mut tick = (start_secs / step).ceil() * step;
        let end_secs = start_secs + visible_secs;
        let label_y = size.height - 2.0;
        let line_bottom = size.height - TIME_MARKER_HEIGHT;

        while tick <= end_secs + step * 0.001 {
            let fraction = ((tick - start_secs) / visible_secs) as f32;
            if !(0.0..=1.0).contains(&fraction) {
                tick += step;
                continue;
            }

            let x = fraction * size.width;
            let line = Path::line(Point::new(x, 0.0), Point::new(x, line_bottom));
            frame.stroke(
                &line,
                Stroke::default()
                    .with_color(palette.marker)
                    .with_width(1.0),
            );

            frame.fill_text(Text {
                content: Self::format_time(tick),
                position: Point::new(x, label_y),
                color: palette.marker_label,
                size: Pixels(10.0),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: alignment::Vertical::Bottom,
                ..Default::default()
            });

            tick += step;
        }
    }

    fn cache_bounds(size: Size, view: WaveFormView, theme: &Theme) -> Rectangle {
        let primary = theme.extended_palette().primary.base.color;
        let theme_key = primary.r + primary.g * 0.01 + primary.b * 0.0001;
        Rectangle {
            x: view.offset + theme_key,
            y: view.zoom,
            width: size.width,
            height: size.height,
        }
    }

    fn handle_input(
        &self,
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
                    if view.zoom <= 1.0 {
                        return false;
                    }
                    let (x, y) = Self::scroll_lines(*delta);
                    let pan_delta = if x.abs() > y.abs() { -x } else { y };
                    if pan_delta == 0.0 {
                        return false;
                    }
                    view.pan(pan_delta * PAN_STEP);
                } else {
                    view.apply_wheel(*delta);
                }
                true
            }
            _ => false,
        }
    }
}

impl Program<Message> for WaveForm {
    type State = WaveFormState;

    fn draw(
        &self,
        _state: &WaveFormState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let cache_bounds = Self::cache_bounds(bounds.size(), self.view, theme);
        let mut layers = vec![self.cache.draw_with_bounds(renderer, cache_bounds, |frame| {
            self.draw_waveform(frame, theme);
        })];
        if let Some(progress) = self.playback_progress()
            && let Some(playhead) = self.draw_playhead(renderer, bounds.size(), theme, progress)
        {
            layers.push(playhead);
        }
        layers
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Message>> {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && cursor.is_over(bounds)
            && let Some(position) = cursor.position_in(bounds)
            && let Some(progress) = self.progress_at_x(bounds.width, position.x)
        {
            return Some(Action::publish(Message::WaveformSeek(progress)).and_capture());
        }

        let mut view = self.view;
        if !self.handle_input(&mut view, event, bounds, cursor) {
            return None;
        }

        Some(
            Action::publish(Message::WaveformViewChanged(view)).and_capture(),
        )
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        cursor: Cursor,
    ) -> iced::mouse::Interaction {
        if cursor.is_over(_bounds) {
            iced::mouse::Interaction::Pointer
        } else {
            iced::mouse::Interaction::default()
        }
    }
}
