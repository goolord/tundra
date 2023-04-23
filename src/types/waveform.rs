pub use super::common::*;
pub use super::style::*;

use iced::keyboard::KeyCode;
use iced::pure::widget::canvas::*;
use iced::{Color, Point, Rectangle};
use rodio::Source;
use std::fs::File;

pub struct WaveFormState {
    cache: Cache,
    zoom: f32,
    scroll : f32,
}

impl Default for WaveFormState {
    fn default() -> WaveFormState {
        WaveFormState {
            cache: Cache::new(),
            zoom: 1.0,
            scroll: 0.0,
        }
    }
}

pub struct WaveForm {
    samples: Vec<i16>,
    bits_per_sample: u32,
}

impl WaveForm {
    pub fn to_path(&self, state: &WaveFormState, frame: &Frame) -> Path {
        let max = 2_i32.pow(self.bits_per_sample);
        let translate_y = (max / 2) as f32;
        let translate_x = state.scroll;
        let height = frame.height();
        let width = frame.width();
        let truncate = 1; // (self.samples.len() as usize).div(width as usize);
        let scale_height = height / max as f32;
        let scale_width = (width / self.samples.len() as f32) * (truncate as f32) * state.zoom;
        let mut builder = path::Builder::new();
        let mut old_y: f32 = translate_y * scale_height;
        self.samples
            .chunks(truncate)
            .map(|x| x.into_iter().max_by_key(|y| y.abs() ).unwrap_or(&0))
            .enumerate()
            .for_each(|(i, s)| {
                let sample = s.to_owned() as f32;
                let x = i as f32 * scale_width;
                let h_point = Point {
                    x,
                    y: old_y.clone(),
                };
                let y = (sample + translate_y) * scale_height;
                old_y = y;
                let v_point = Point {
                    x,
                    y,
                };
                builder.line_to(h_point);
                builder.move_to(h_point);
                builder.line_to(v_point);
                builder.move_to(v_point);
            });

        builder.close();
        builder.build()
    }
}

impl Program<Message> for WaveForm {
    type State = WaveFormState;
    fn draw(&self, state: &WaveFormState, bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let geometry = state.cache.draw(bounds.size(), |frame| {
            // frame.scale(0.01);
            // frame.translate(Vector {
            //     x: 0.0,
            //     y: (max / 2) as f32
            // });
            let path = self.to_path(state, &frame);
            let stroke = Stroke {
                color: Color::from_rgb8(0x50, 0x7a, 0xe0),
                width: 2.0,
                line_cap: Default::default(),
                line_join: Default::default(),
                line_dash: Default::default(),
            };
            frame.stroke(&path, stroke);
        });
        vec![geometry]
    }

    fn update(
            &self,
            state: &mut Self::State,
            event: Event,
            _bounds: Rectangle,
            _cursor: Cursor,
        ) -> (event::Status, Option<Message>) {
        match event {
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key_code: KeyCode::Plus | KeyCode::Equals, modifiers: _ }) => {
                state.zoom = state.zoom + 1.0;
                state.cache.clear();
                (event::Status::Captured, None)
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key_code: KeyCode::Minus, modifiers: _ }) => {
                state.zoom = state.zoom - 1.0;
                state.cache.clear();
                (event::Status::Captured, None)
            }
            _ => (event::Status::Ignored, None)
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> iced_native::mouse::Interaction {
        iced_native::mouse::Interaction::default()
    }
}

impl From<rodio::Decoder<std::io::BufReader<File>>> for WaveForm {
    fn from(decoder: rodio::Decoder<std::io::BufReader<File>>) -> WaveForm {
        let number_channels = decoder.channels();
        let all_samples: Vec<i16> = decoder.into_iter().collect();
        let mut samples: Vec<i16> = Vec::with_capacity(all_samples.len());
        for arr in all_samples.chunks_exact(number_channels as usize) {
            samples.push(arr.iter().sum::<i16>() / number_channels as i16);
        }
        WaveForm {
            samples,
            bits_per_sample: 16,
        }
    }
}


fn mean(list: &[i16]) -> f64 {
    let sum: i16 = Iterator::sum(list.iter());
    f64::from(sum) / (list.len() as f64)
}

