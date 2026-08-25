pub use super::common::*;

use iced::keyboard::Key;
use iced::mouse::Cursor;
use iced::widget::canvas::Action;
use iced::widget::canvas::*;
use iced::{Color, Point, Rectangle, Renderer, Theme};

const MIN_ZOOM: f32 = 1.0;

pub struct WaveFormState {
    zoom: f32,
}

impl Default for WaveFormState {
    fn default() -> WaveFormState {
        WaveFormState { zoom: 1.0 }
    }
}

pub struct WaveForm {
    pub samples: Vec<f32>,
    cache: Cache,
}

impl WaveForm {
    pub fn new(samples: Vec<f32>) -> Self {
        Self {
            samples,
            cache: Cache::new(),
        }
    }

    pub fn to_path(&self, state: &WaveFormState, frame: &Frame) -> Path {
        let height = frame.height();
        let width = frame.width();
        let center = height / 2.0;

        if self.samples.is_empty() || width <= 0.0 || height <= 0.0 {
            return path::Builder::new().build();
        }

        let sample_count = self.samples.len();
        let columns = (width * state.zoom).ceil().max(1.0) as usize;
        let bucket = sample_count.div_ceil(columns);
        let scale_width = width / columns as f32;
        let mut builder = path::Builder::new();

        for col in 0..columns {
            let start = col * bucket;
            if start >= sample_count {
                break;
            }
            let end = (start + bucket).min(sample_count);
            let chunk = &self.samples[start..end];
            let min = chunk
                .iter()
                .copied()
                .fold(0.0_f32, |acc, sample| acc.min(sample))
                .clamp(-1.0, 1.0);
            let max = chunk
                .iter()
                .copied()
                .fold(0.0_f32, |acc, sample| acc.max(sample))
                .clamp(-1.0, 1.0);
            let x = col as f32 * scale_width;
            let y_top = center - max * center;
            let y_bottom = center - min * center;
            builder.move_to(Point { x, y: y_top });
            builder.line_to(Point { x, y: y_bottom });
        }

        builder.build()
    }
}

impl Program<Message> for WaveForm {
    type State = WaveFormState;

    fn draw(
        &self,
        state: &WaveFormState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let path = self.to_path(state, frame);
            let stroke = Stroke {
                width: 2.0,
                line_cap: Default::default(),
                line_join: Default::default(),
                line_dash: Default::default(),
                style: Style::Solid(Color::from_rgb8(0x50, 0x7a, 0xe0)),
            };
            frame.stroke(&path, stroke);
        });
        vec![geometry]
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> Option<Action<Message>> {
        match event {
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => match key.as_ref()
            {
                Key::Character("+") | Key::Character("=") => {
                    state.zoom += 1.0;
                    self.cache.clear();
                    Some(Action::request_redraw().and_capture())
                }
                Key::Character("-") if state.zoom > MIN_ZOOM => {
                    state.zoom = (state.zoom - 1.0).max(MIN_ZOOM);
                    self.cache.clear();
                    Some(Action::request_redraw().and_capture())
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> iced::mouse::Interaction {
        iced::mouse::Interaction::default()
    }
}
