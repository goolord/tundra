use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rodio::{Decoder, Source};

use super::position::PlaybackPosition;

/// Opens `path` for decoding, checking it has a usable channel count and sample rate.
fn open_decoder(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let display = crate::path_util::display_path(path);
    let file = File::open(path).map_err(|err| format!("Cannot open {display}: {err}"))?;
    let decoder = Decoder::try_from(file).map_err(|err| format!("Cannot decode {display}: {err}"))?;
    if decoder.channels() == 0 {
        return Err("Audio file has no channels".into());
    }
    if decoder.sample_rate() == 0 {
        return Err("Audio file has an invalid sample rate".into());
    }
    Ok(decoder)
}

pub struct StreamInfo {
    pub sample_rate: u32,
    pub total_frames: u64,
}

pub fn probe_decoder(path: &Path) -> Result<StreamInfo, String> {
    let decoder = open_decoder(path)?;
    let sample_rate = decoder.sample_rate();
    let total_frames = decoder.total_duration().map_or(0, |duration| frames_from_duration(duration, sample_rate));
    Ok(StreamInfo { sample_rate, total_frames })
}

fn frames_from_duration(duration: Duration, sample_rate: u32) -> u64 {
    (duration.as_secs_f64() * f64::from(sample_rate)).round().max(0.0) as u64
}

/// A decoded file that keeps the shared playhead at the frame it is playing, and calls
/// `on_end` when it runs out. Dropping it first (with its sink) means `on_end` never runs.
pub struct StreamSource {
    decoder: Decoder<BufReader<File>>,
    channels: usize,
    /// Where the decoder was seeked to. `sample_index` counts from this source's first sample,
    /// so reported frames have to be biased by it or a seeked stream reports itself as playing
    /// from the top of the file.
    start_frame: u64,
    sample_index: usize,
    position: Arc<PlaybackPosition>,
    on_end: Option<Box<dyn FnOnce() + Send>>,
}

impl StreamSource {
    /// Opens `path` at `progress` (0 to 1) of the track.
    pub fn open(
        path: &Path,
        progress: f64,
        position: Arc<PlaybackPosition>,
        on_end: Box<dyn FnOnce() + Send>,
    ) -> Result<Self, String> {
        let mut decoder = open_decoder(path)?;
        let (channels, sample_rate) = (decoder.channels() as usize, decoder.sample_rate());
        let progress = progress.clamp(0.0, 1.0);
        let total_frames = position.total_frames();
        let target = match decoder.total_duration() {
            Some(duration) => Duration::from_secs_f64(progress * duration.as_secs_f64()),
            None => Duration::from_secs_f64(progress * total_frames as f64 / f64::from(sample_rate)),
        };
        let start_frame = if target > Duration::ZERO && decoder.try_seek(target).is_ok() {
            match total_frames {
                0 => frames_from_duration(target, sample_rate),
                total => (progress * total as f64).round() as u64,
            }
        } else {
            0
        };
        position.set_frame(start_frame);

        Ok(Self { decoder, channels, start_frame, sample_index: 0, position, on_end: Some(on_end) })
    }
}

impl Iterator for StreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(sample) = self.decoder.next() else {
            if let Some(on_end) = self.on_end.take() {
                on_end();
            }
            return None;
        };
        if self.sample_index.is_multiple_of(self.channels) {
            let frame = self.start_frame + (self.sample_index / self.channels) as u64;
            self.position.set_frame(frame);
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
        self.decoder.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.decoder.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn asset(ext: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/assets/tone.{ext}"))
    }

    #[test]
    fn probe_test_assets_have_frames() {
        for ext in ["wav", "flac", "mp3", "ogg", "aiff"] {
            let info = probe_decoder(&asset(ext)).unwrap_or_else(|err| panic!("{ext}: {err}"));
            assert!(info.total_frames > 0, "{ext} should report frame count");
        }
    }

    #[test]
    fn seeked_stream_reports_frames_from_the_seek_point() {
        let info = probe_decoder(&asset("wav")).expect("probe tone.wav");
        let position = PlaybackPosition::new(info.total_frames);
        let mut source =
            StreamSource::open(&asset("wav"), 0.5, Arc::clone(&position), Box::new(|| ())).expect("open seeked stream");
        assert!(source.start_frame > 0, "test needs a decoder that can actually seek");

        // Pulling samples must advance from the seek point, not replay the file's frame
        // numbering from zero and drag the playhead back to the start.
        for _ in 0..source.channels * 64 {
            source.next();
        }
        assert!(position.progress() >= 0.5, "playhead fell back to {} after seeking to 0.5", position.progress());
    }
}
