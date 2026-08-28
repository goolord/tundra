use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};

use super::arc_samples::PlaybackPosition;

fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path)
        .map_err(|err| format!("Cannot open {}: {err}", path.display()))?;
    Decoder::try_from(file)
        .map_err(|err| format!("Cannot decode {}: {err}", path.display()))
}

pub fn probe_decoder(path: &Path) -> Result<StreamInfo, String> {
    stream_info_from_decoder(&open_decoder(path)?)
}

pub struct StreamInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub total_frames: u64,
}

fn stream_info_from_decoder(decoder: &Decoder<BufReader<File>>) -> Result<StreamInfo, String> {
    let channels = decoder.channels();
    let sample_rate = decoder.sample_rate();
    if channels == 0 {
        return Err("Audio file has no channels".into());
    }
    if sample_rate == 0 {
        return Err("Audio file has an invalid sample rate".into());
    }
    let total_frames = decoder
        .total_duration()
        .map(|duration| frames_from_duration(duration, sample_rate))
        .unwrap_or(0);
    Ok(StreamInfo {
        channels,
        sample_rate,
        total_frames,
    })
}

pub fn frames_from_duration(duration: Duration, sample_rate: u32) -> u64 {
    (duration.as_secs_f64() * f64::from(sample_rate)).round().max(0.0) as u64
}

pub struct StreamSource {
    decoder: Decoder<BufReader<File>>,
    channels: usize,
    sample_rate: u32,
    sample_index: usize,
    position: Option<Arc<PlaybackPosition>>,
}

impl StreamSource {
    pub fn open(
        path: &Path,
        progress: f64,
        total_frames: u64,
        position: Option<Arc<PlaybackPosition>>,
    ) -> Result<Self, String> {
        let mut decoder = open_decoder(path)?;
        let channels = decoder.channels() as usize;
        let sample_rate = decoder.sample_rate();
        if channels == 0 || sample_rate == 0 {
            return Err(format!("{} has invalid audio layout", path.display()));
        }

        let progress = progress.clamp(0.0, 1.0);
        let skip_frames = if progress > 0.0 {
            let target = if let Some(duration) = decoder.total_duration() {
                Duration::from_secs_f64(progress * duration.as_secs_f64())
            } else if total_frames > 0 {
                Duration::from_secs_f64(progress * total_frames as f64 / sample_rate as f64)
            } else {
                Duration::ZERO
            };
            if target > Duration::ZERO && decoder.try_seek(target).is_ok() {
                if total_frames > 0 {
                    (progress * total_frames as f64).round() as u64
                } else if let Some(duration) = decoder.total_duration() {
                    frames_from_duration(
                        Duration::from_secs_f64(progress * duration.as_secs_f64()),
                        sample_rate,
                    )
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };
        if let Some(position) = &position {
            position.set_frame(skip_frames);
        }

        Ok(Self {
            decoder,
            channels,
            sample_rate,
            sample_index: 0,
            position,
        })
    }
}

impl Iterator for StreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.decoder.next()?;
        if self.channels > 0 && self.sample_index.is_multiple_of(self.channels) {
            if let Some(position) = &self.position {
                position.set_frame((self.sample_index / self.channels) as u64);
            }
        }
        self.sample_index += 1;
        Some(sample)
    }
}

impl Source for StreamSource {
    fn current_span_len(&self) -> Option<usize> {
        self.decoder.current_span_len()
    }

    fn channels(&self) -> u16 {
        self.channels as u16
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.decoder.total_duration()
    }
}

pub fn append_stream(
    sink: &rodio::Sink,
    path: &Path,
    progress: f64,
    total_frames: u64,
    position: Option<Arc<PlaybackPosition>>,
    output_channels: u16,
    output_sample_rate: u32,
) -> Result<(), String> {
    let source = StreamSource::open(path, progress, total_frames, position)?;
    sink.append(UniformSourceIterator::new(
        source,
        output_channels,
        output_sample_rate,
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn probe_test_assets_have_frames() {
        for ext in ["wav", "flac", "mp3", "ogg", "aiff"] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(format!("tone.{ext}"));
            let info = probe_decoder(&path).unwrap_or_else(|err| panic!("{ext}: {err}"));
            assert!(info.total_frames > 0, "{ext} should report frame count");
        }
    }
}
