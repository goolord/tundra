//! Rust tier 1 zero-crossing-rate classifier.
//!
//! Frame layout follows librosa's `zero_crossing_rate` (22050 Hz, 2048-sample
//! frames, 512 hop, centered with reflect padding). Tier 2 reuses this ZCR for
//! grey-zone files.

use super::ClassifyError;
use rodio::Source;
use std::fs::File;
use std::path::Path;

const SAMPLE_RATE: u32 = 22050;
const ANALYSIS_SECONDS: f32 = 30.0;
const FRAME_LENGTH: usize = 2048;
const HOP_LENGTH: usize = 512;
const MAX_AUDIO_BYTES: u64 = 100 * 1024 * 1024;

const VERY_LOW_THRESHOLD: f64 = 0.03;
const LOW_THRESHOLD: f64 = 0.04;
const HIGH_THRESHOLD: f64 = 0.20;
const VERY_HIGH_THRESHOLD: f64 = 0.28;

#[derive(Debug, Clone)]
pub struct Tier1Result {
    /// `Some` when the ZCR alone is decisive; `None` for the grey zone.
    pub instrument: Option<String>,
    pub zcr: f64,
    pub confidence: Option<f64>,
}

pub fn classify(path: &Path) -> Result<Tier1Result, ClassifyError> {
    if !path.is_file() {
        return Err(ClassifyError::new(
            "Couldn't find that audio file.",
            format!("File not found: {}", path.display()),
        ));
    }
    let audio = load_mono_audio(path)?;
    Ok(classify_zcr(zero_crossing_rate(&audio)))
}

fn classify_zcr(zcr: f64) -> Tier1Result {
    let (instrument, confidence) = if zcr <= VERY_LOW_THRESHOLD {
        ("Kick", (0.74 + (VERY_LOW_THRESHOLD - zcr) * 5.0).min(0.95))
    } else if zcr <= LOW_THRESHOLD {
        ("Bass", (0.70 + (LOW_THRESHOLD - zcr) * 4.0).min(0.92))
    } else if zcr >= VERY_HIGH_THRESHOLD {
        ("Cymbal", (0.74 + (zcr - VERY_HIGH_THRESHOLD) * 2.0).min(0.95))
    } else if zcr >= HIGH_THRESHOLD {
        ("Hi-Hat", (0.70 + (zcr - HIGH_THRESHOLD) * 2.5).min(0.92))
    } else {
        return Tier1Result {
            instrument: None,
            zcr,
            confidence: None,
        };
    };
    Tier1Result {
        instrument: Some(instrument.into()),
        zcr,
        confidence: Some(confidence),
    }
}

/// Up to `ANALYSIS_SECONDS` of the file, downmixed while decoding and
/// resampled to `SAMPLE_RATE`.
fn load_mono_audio(path: &Path) -> Result<Vec<f32>, ClassifyError> {
    let file_len = std::fs::metadata(path)
        .map_err(|err| ClassifyError::analysis_failed(format!("Failed to stat {}: {err}", path.display())))?
        .len();
    if file_len > MAX_AUDIO_BYTES {
        return Err(ClassifyError::analysis_failed(format!(
            "{} is too large ({} MB; limit is {} MB)",
            path.display(),
            file_len / (1024 * 1024),
            MAX_AUDIO_BYTES / (1024 * 1024)
        )));
    }

    let file = File::open(path)
        .map_err(|err| ClassifyError::analysis_failed(format!("Failed to open {}: {err}", path.display())))?;
    let mut decoder = rodio::Decoder::try_from(file)
        .map_err(|err| ClassifyError::analysis_failed(format!("Cannot decode {}: {err}", path.display())))?;

    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels().max(1) as usize;
    let max_frames = (sample_rate as f32 * ANALYSIS_SECONDS).ceil() as usize;
    let mut mono = Vec::with_capacity(max_frames.min(file_len as usize / channels));
    'frames: while mono.len() < max_frames {
        let mut sum = 0.0f32;
        for _ in 0..channels {
            match decoder.next() {
                Some(sample) => sum += sample,
                None => break 'frames,
            }
        }
        mono.push(sum / channels as f32);
    }
    if mono.is_empty() {
        return Err(ClassifyError::analysis_failed(format!(
            "{} contains no audio samples",
            path.display()
        )));
    }
    Ok(resample_linear(&mono, sample_rate, SAMPLE_RATE))
}

fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((input.len() as f64) / ratio).floor() as usize;
    (0..output_len)
        .map(|out_idx| {
            let src_pos = out_idx as f64 * ratio;
            let base = src_pos.floor() as usize;
            let frac = src_pos - base as f64;
            let a = input[base];
            let b = input.get(base + 1).copied().unwrap_or(a);
            a + ((b - a) as f64 * frac) as f32
        })
        .collect()
}

/// Mean over frames of (sign changes in the frame) / (frame length - 1).
/// Frame sums come from a prefix sum, so the cost is linear in the signal.
fn zero_crossing_rate(audio: &[f32]) -> f64 {
    if audio.len() < 2 {
        return 0.0;
    }
    // Crossing into sample `i` from the previous one (zero before the start).
    let crossings: Vec<u32> = std::iter::once(0.0)
        .chain(audio.iter().copied())
        .zip(audio.iter().copied())
        .map(|(previous, current)| u32::from(previous * current < 0.0))
        .collect();
    let padded = reflect_pad(&crossings, FRAME_LENGTH / 2);
    if padded.len() < FRAME_LENGTH {
        return 0.0;
    }

    let mut prefix = Vec::with_capacity(padded.len() + 1);
    prefix.push(0u32);
    for value in &padded {
        prefix.push(prefix.last().copied().unwrap_or(0) + value);
    }

    let frames = (padded.len() - FRAME_LENGTH) / HOP_LENGTH + 1;
    let total: f64 = (0..=padded.len() - FRAME_LENGTH)
        .step_by(HOP_LENGTH)
        .map(|start| f64::from(prefix[start + FRAME_LENGTH] - prefix[start]) / (FRAME_LENGTH - 1) as f64)
        .sum();
    total / frames as f64
}

/// numpy-style reflect padding (edge sample not repeated) on both sides.
fn reflect_pad<T: Copy>(signal: &[T], pad: usize) -> Vec<T> {
    let last = signal.len().saturating_sub(1);
    let mut out = Vec::with_capacity(signal.len() + 2 * pad);
    out.extend((0..pad).rev().map(|idx| signal[(idx + 1).min(last)]));
    out.extend_from_slice(signal);
    out.extend((0..pad).map(|idx| signal[last.saturating_sub(1).saturating_sub(idx)]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity() {
        let input = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let output = resample_linear(&input, 22050, 22050);
        assert_eq!(input, output);
    }

    #[test]
    fn grey_band_between_thresholds() {
        let result = classify_zcr(0.10);
        assert!(result.instrument.is_none());
    }

    /// Frame-by-frame reference: copy each frame, count its crossings.
    fn reference_zcr(audio: &[f32]) -> f64 {
        let mut padded_signal = vec![0.0];
        padded_signal.extend_from_slice(audio);
        let crossings: Vec<f32> = padded_signal
            .windows(2)
            .map(|pair| if pair[0] * pair[1] < 0.0 { 1.0 } else { 0.0 })
            .collect();
        let padded = reflect_pad(&crossings, FRAME_LENGTH / 2);
        let rates: Vec<f64> = padded
            .windows(FRAME_LENGTH)
            .step_by(HOP_LENGTH)
            .map(|frame| f64::from(frame.iter().sum::<f32>()) / (FRAME_LENGTH - 1) as f64)
            .collect();
        rates.iter().sum::<f64>() / rates.len() as f64
    }

    #[test]
    fn prefix_sum_zcr_matches_frame_by_frame_reference() {
        for (len, freq) in [(3_000, 440.0), (22_050, 90.0), (50_000, 3_000.0), (4_096, 7.0)] {
            let audio: Vec<f32> = (0..len)
                .map(|n| (n as f32 * freq * std::f32::consts::TAU / SAMPLE_RATE as f32).sin())
                .collect();
            let fast = zero_crossing_rate(&audio);
            let slow = reference_zcr(&audio);
            assert!((fast - slow).abs() < 1e-12, "len {len}: {fast} vs {slow}");
        }
    }
}
