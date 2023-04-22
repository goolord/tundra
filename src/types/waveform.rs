pub use super::common::*;
pub use super::style::*;

use iced::pure::widget::canvas::*;
use iced::{Color, Point, Rectangle};
use rodio::Source;
use std::fs::File;

pub struct WaveForm {
    samples: Vec<i16>,
    bits_per_sample: u32,
    cache: Cache,
}

impl WaveForm {
    pub fn to_path(&self, frame: &Frame) -> Path {
        let max = 2_i32.pow(self.bits_per_sample);
        let translate_y = (max / 2) as f32;
        let height = frame.height();
        let width = frame.width();
        let truncate = 1; // (self.samples.len() as usize).div(width as usize);
        let scale_height = height / max as f32;
        let scale_width = (width / self.samples.len() as f32) * (truncate as f32);
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
    type State = ();
    fn draw(&self, _state: &(), bounds: Rectangle, _cursor: Cursor) -> Vec<Geometry> {
        let geometry = self.cache.draw(bounds.size(), |frame| {
            // frame.scale(0.01);
            // frame.translate(Vector {
            //     x: 0.0,
            //     y: (max / 2) as f32
            // });
            let path = self.to_path(&frame);
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
            cache: Cache::new(),
        }
    }
}


fn mean(list: &[i16]) -> f64 {
    let sum: i16 = Iterator::sum(list.iter());
    f64::from(sum) / (list.len() as f64)
}

