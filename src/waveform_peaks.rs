use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use rodio::{Decoder, Source};

pub const PEAK_BUCKET_COUNT: usize = 65_536;
const MAX_PEAK_DECODE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug)]
pub struct WaveformPeaks {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
    pub sample_count: usize,
    pub complete: bool,
}

impl WaveformPeaks {
    pub fn empty() -> Self {
        Self::new(0)
    }

    pub fn new(sample_count: usize) -> Self {
        Self {
            min: vec![0.0; PEAK_BUCKET_COUNT],
            max: vec![0.0; PEAK_BUCKET_COUNT],
            sample_count,
            complete: false,
        }
    }

    pub fn extents_for_range(&self, start: usize, end: usize) -> (f32, f32) {
        if self.sample_count == 0 || start >= end {
            return (0.0, 0.0);
        }
        let start = start.min(self.sample_count);
        let end = end.min(self.sample_count);
        if start >= end {
            return (0.0, 0.0);
        }

        let first_bucket = start * PEAK_BUCKET_COUNT / self.sample_count;
        let last_bucket = end.saturating_sub(1) * PEAK_BUCKET_COUNT / self.sample_count;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for bucket in first_bucket..=last_bucket.min(PEAK_BUCKET_COUNT - 1) {
            min = min.min(self.min[bucket]);
            max = max.max(self.max[bucket]);
        }
        if !min.is_finite() || !max.is_finite() {
            (0.0, 0.0)
        } else {
            (min, max)
        }
    }

    pub fn midpoint_at(&self, sample_index: usize) -> f32 {
        if self.sample_count == 0 {
            return 0.0;
        }
        let bucket = (sample_index.min(self.sample_count.saturating_sub(1)) * PEAK_BUCKET_COUNT)
            / self.sample_count;
        let bucket = bucket.min(PEAK_BUCKET_COUNT - 1);
        (self.min[bucket] + self.max[bucket]) * 0.5
    }
}

/// Builds a peak envelope in one decode pass. `sample_count_hint` should come from
/// stream metadata when available; when zero the file is scanned once to count frames first.
pub fn build_peaks(path: &Path, sample_count_hint: usize) -> WaveformPeaks {
    if file_too_large_for_peaks(path) {
        return skipped_peaks(sample_count_hint);
    }

    let sample_count = if sample_count_hint > 0 {
        sample_count_hint
    } else {
        count_frames(path)
    };
    if sample_count == 0 {
        return WaveformPeaks::new(0);
    }

    let Some(decoder) = open_decoder(path) else {
        return WaveformPeaks::new(0);
    };

    let channels = decoder.channels().max(1) as usize;
    let mut frame = 0usize;
    let mut channel = Vec::with_capacity(channels);
    let mut min = vec![f32::INFINITY; PEAK_BUCKET_COUNT];
    let mut max = vec![f32::NEG_INFINITY; PEAK_BUCKET_COUNT];

    for sample in decoder {
        channel.push(sample);
        if channel.len() < channels {
            continue;
        }
        let mono = channel.iter().sum::<f32>() / channels as f32;
        channel.clear();
        let bucket = frame * PEAK_BUCKET_COUNT / sample_count;
        let bucket = bucket.min(PEAK_BUCKET_COUNT - 1);
        min[bucket] = min[bucket].min(mono);
        max[bucket] = max[bucket].max(mono);
        frame += 1;
    }

    let sample_count = if frame > 0 {
        frame
    } else {
        sample_count_hint
    };

    for bucket in 0..PEAK_BUCKET_COUNT {
        if !min[bucket].is_finite() {
            min[bucket] = 0.0;
        }
        if !max[bucket].is_finite() {
            max[bucket] = 0.0;
        }
    }

    WaveformPeaks {
        min,
        max,
        sample_count,
        complete: true,
    }
}

fn file_too_large_for_peaks(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() > MAX_PEAK_DECODE_BYTES)
        .unwrap_or(false)
}

fn skipped_peaks(sample_count_hint: usize) -> WaveformPeaks {
    WaveformPeaks {
        min: vec![0.0; PEAK_BUCKET_COUNT],
        max: vec![0.0; PEAK_BUCKET_COUNT],
        sample_count: sample_count_hint,
        complete: sample_count_hint > 0,
    }
}

fn open_decoder(path: &Path) -> Option<Decoder<std::io::BufReader<File>>> {
    Decoder::try_from(File::open(path).ok()?).ok()
}

fn count_frames(path: &Path) -> usize {
    if file_too_large_for_peaks(path) {
        return 0;
    }
    let Some(decoder) = open_decoder(path) else {
        return 0;
    };
    let channels = decoder.channels().max(1) as usize;
    let mut frame = 0usize;
    let mut channel = Vec::with_capacity(channels);
    for sample in decoder {
        channel.push(sample);
        if channel.len() == channels {
            channel.clear();
            frame += 1;
        }
    }
    frame
}

pub fn spawn_peak_build(
    path: PathBuf,
    sample_count_hint: usize,
    peaks: Arc<Mutex<WaveformPeaks>>,
    on_complete: impl FnOnce() + Send + 'static,
) {
    thread::spawn(move || {
        let built = build_peaks(&path, sample_count_hint);
        if let Ok(mut shared) = peaks.lock() {
            *shared = built;
        }
        on_complete();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_peaks_uses_decoded_frame_count_when_hint_overstated() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets/tone.wav");
        let peaks = build_peaks(&path, 999_999);
        assert!(peaks.complete);
        assert!(peaks.sample_count > 0);
        assert!(peaks.sample_count < 999_999);
    }
}
