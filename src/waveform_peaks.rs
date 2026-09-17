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
        let bucket = (sample_index.min(self.sample_count.saturating_sub(1)) * PEAK_BUCKET_COUNT) / self.sample_count;
        self.bucket_midpoint(bucket)
    }

    pub fn bucket_midpoint(&self, bucket: usize) -> f32 {
        let bucket = bucket.min(PEAK_BUCKET_COUNT - 1);
        (self.min[bucket] + self.max[bucket]) * 0.5
    }
}

/// Builds a peak envelope. `sample_count_hint` should come from stream metadata
/// when available; when zero, or when the decode finds a different length
/// (MP3 without an accurate header), frames are counted and peaks rebuilt so
/// the envelope lines up with the playhead. Returns `None` once `cancelled`.
pub fn build_peaks(path: &Path, sample_count_hint: usize, cancelled: &dyn Fn() -> bool) -> Option<WaveformPeaks> {
    if file_too_large_for_peaks(path) {
        return Some(skipped_peaks(sample_count_hint));
    }
    let mut sample_count = sample_count_hint;
    if sample_count == 0 {
        sample_count = decode_peaks(path, None, cancelled)?.sample_count;
    }
    if sample_count == 0 {
        return Some(WaveformPeaks::new(0));
    }
    let peaks = decode_peaks(path, Some(sample_count), cancelled)?;
    // About 0.05%: a coarser tolerance let long files drift seconds from the playhead.
    let tolerance = (sample_count / 2000).max(1);
    if peaks.sample_count > 0 && peaks.sample_count.abs_diff(sample_count) > tolerance {
        return decode_peaks(path, Some(peaks.sample_count), cancelled);
    }
    Some(peaks)
}

/// One decode pass. With `sample_count` of `None` it only counts frames.
fn decode_peaks(path: &Path, sample_count: Option<usize>, cancelled: &dyn Fn() -> bool) -> Option<WaveformPeaks> {
    const CANCEL_CHECK_FRAMES: usize = 1 << 14;

    let Some(decoder) = open_decoder(path) else {
        return Some(WaveformPeaks::new(0));
    };
    let channels = decoder.channels().max(1) as usize;
    let buckets = if sample_count.is_some() { PEAK_BUCKET_COUNT } else { 0 };
    let mut min = vec![f32::INFINITY; buckets];
    let mut max = vec![f32::NEG_INFINITY; buckets];
    let mut frame = 0usize;
    let mut sum = 0.0f32;
    let mut in_frame = 0usize;

    for sample in decoder {
        sum += sample;
        in_frame += 1;
        if in_frame < channels {
            continue;
        }
        if let Some(count) = sample_count {
            let mono = sum / channels as f32;
            let bucket = (frame * PEAK_BUCKET_COUNT / count).min(PEAK_BUCKET_COUNT - 1);
            min[bucket] = min[bucket].min(mono);
            max[bucket] = max[bucket].max(mono);
        }
        sum = 0.0;
        in_frame = 0;
        frame += 1;
        if frame.is_multiple_of(CANCEL_CHECK_FRAMES) && cancelled() {
            return None;
        }
    }

    for value in min.iter_mut().chain(max.iter_mut()) {
        if !value.is_finite() {
            *value = 0.0;
        }
    }
    Some(WaveformPeaks {
        min,
        max,
        sample_count: if frame > 0 { frame } else { sample_count.unwrap_or(0) },
        complete: sample_count.is_some(),
    })
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

/// Build peaks on a background thread. `cancelled` is polled while decoding so
/// skipping through files does not stack up full decodes of each one.
pub fn spawn_peak_build(
    path: PathBuf,
    sample_count_hint: usize,
    peaks: Arc<Mutex<WaveformPeaks>>,
    cancelled: impl Fn() -> bool + Send + 'static,
    on_complete: impl FnOnce() + Send + 'static,
) {
    thread::spawn(move || {
        let Some(built) = build_peaks(&path, sample_count_hint, &cancelled) else {
            return;
        };
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
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/tone.wav");
        let peaks = build_peaks(&path, 999_999, &|| false).expect("not cancelled");
        assert!(peaks.complete);
        assert!(peaks.sample_count > 0);
        assert!(peaks.sample_count < 999_999);
    }

    #[test]
    fn build_peaks_rebuckets_to_the_decoded_length_and_honours_cancel() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/tone.wav");
        let exact = build_peaks(&path, 0, &|| false).expect("not cancelled");
        let overstated = build_peaks(&path, exact.sample_count * 3, &|| false).expect("not cancelled");
        assert_eq!(overstated.sample_count, exact.sample_count);
        assert_eq!(overstated.max, exact.max, "envelope must span the real length");
        assert!(build_peaks(&path, exact.sample_count, &|| true).is_none() || exact.sample_count < 1 << 14);
    }
}
