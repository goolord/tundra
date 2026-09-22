//! Rust tier 1 zero-crossing-rate classifier.
//!
//! Frame layout follows librosa's `zero_crossing_rate` (22050 Hz, 2048-sample
//! frames, 512 hop, centered with reflect padding). Tier 2 reuses this ZCR for
//! grey-zone files.

use super::ClassifyError;
use rodio::Source;
use std::path::Path;

const SAMPLE_RATE: u32 = 22050;
const ANALYSIS_SECONDS: f32 = 30.0;
const FRAME_LENGTH: usize = 2048;
const HOP_LENGTH: usize = 512;
const MAX_AUDIO_BYTES: u64 = 100 * 1024 * 1024;

/// The ZCR of the file's first `ANALYSIS_SECONDS`.
pub fn file_zcr(path: &Path) -> Result<f64, ClassifyError> {
    if !path.is_file() {
        return Err(ClassifyError::new(
            "Couldn't find that audio file.",
            format!("File not found: {}", crate::path_util::display_path(path)),
        ));
    }
    Ok(zero_crossing_rate(&load_mono_audio(path)?))
}

/// Instrument and confidence when the ZCR alone is decisive; `None` for the
/// grey zone in between, which goes to tier 2.
pub fn classify_zcr(zcr: f64) -> Option<(&'static str, f64)> {
    Some(match zcr {
        ..=0.03 => ("Kick", (0.74 + (0.03 - zcr) * 5.0).min(0.95)),
        ..=0.04 => ("Bass", (0.70 + (0.04 - zcr) * 4.0).min(0.92)),
        0.28.. => ("Cymbal", (0.74 + (zcr - 0.28) * 2.0).min(0.95)),
        0.20.. => ("Hi-Hat", (0.70 + (zcr - 0.20) * 2.5).min(0.92)),
        _ => return None,
    })
}

/// Up to `ANALYSIS_SECONDS` of the file, downmixed while decoding and
/// resampled to `SAMPLE_RATE`.
fn load_mono_audio(path: &Path) -> Result<Vec<f32>, ClassifyError> {
    let fail = |what: &str, err: &dyn std::fmt::Display| {
        ClassifyError::analysis_failed(format!("{what} {}: {err}", crate::path_util::display_path(path)))
    };
    let file_len = std::fs::metadata(path).map_err(|err| fail("Failed to stat", &err))?.len();
    if file_len > MAX_AUDIO_BYTES {
        let mb = |bytes: u64| bytes / (1024 * 1024);
        return Err(ClassifyError::analysis_failed(format!(
            "{} is too large ({} MB; limit is {} MB)",
            crate::path_util::display_path(path),
            mb(file_len),
            mb(MAX_AUDIO_BYTES)
        )));
    }
    let file = std::fs::File::open(path).map_err(|err| fail("Failed to open", &err))?;
    let mut decoder = rodio::Decoder::try_from(file).map_err(|err| fail("Cannot decode", &err))?;

    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels().max(1) as usize;
    let max_frames = (sample_rate as f32 * ANALYSIS_SECONDS).ceil() as usize;
    let mut mono = Vec::with_capacity(max_frames.min(file_len as usize / channels));
    'frames: while mono.len() < max_frames {
        let mut sum = 0.0f32;
        for _ in 0..channels {
            let Some(sample) = decoder.next() else { break 'frames };
            sum += sample;
        }
        mono.push(sum / channels as f32);
    }
    if mono.is_empty() {
        return Err(ClassifyError::analysis_failed(format!(
            "{} contains no audio samples",
            crate::path_util::display_path(path)
        )));
    }
    Ok(resample_linear(&mono, sample_rate, SAMPLE_RATE))
}

fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (input.len() as f64 / ratio).floor() as usize;
    (0..output_len)
        .map(|out_idx| {
            let src_pos = out_idx as f64 * ratio;
            let base = src_pos.floor() as usize;
            let a = input[base];
            let b = input.get(base + 1).copied().unwrap_or(a);
            a + ((b - a) as f64 * (src_pos - base as f64)) as f32
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
    let prefix: Vec<u32> = std::iter::once(0)
        .chain(padded.iter().scan(0, |sum, value| {
            *sum += value;
            Some(*sum)
        }))
        .collect();
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
    fn thresholds_and_resampling() {
        assert_eq!(classify_zcr(0.01).map(|(label, _)| label), Some("Kick"));
        assert_eq!(classify_zcr(0.10), None, "grey band goes to tier 2");
        assert_eq!(classify_zcr(0.30).map(|(label, _)| label), Some("Cymbal"));
        let input = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        assert_eq!(resample_linear(&input, 22050, 22050), input);
    }

    /// Frame-by-frame reference: copy each frame, count its crossings.
    fn reference_zcr(audio: &[f32]) -> f64 {
        let padded_signal: Vec<f32> = std::iter::once(0.0).chain(audio.iter().copied()).collect();
        let crossings: Vec<f32> =
            padded_signal.windows(2).map(|pair| if pair[0] * pair[1] < 0.0 { 1.0 } else { 0.0 }).collect();
        let rates: Vec<f64> = reflect_pad(&crossings, FRAME_LENGTH / 2)
            .windows(FRAME_LENGTH)
            .step_by(HOP_LENGTH)
            .map(|frame| f64::from(frame.iter().sum::<f32>()) / (FRAME_LENGTH - 1) as f64)
            .collect();
        rates.iter().sum::<f64>() / rates.len() as f64
    }

    #[test]
    fn prefix_sum_zcr_matches_frame_by_frame_reference() {
        for (len, freq) in [(3_000, 440.0), (22_050, 90.0), (50_000, 3_000.0), (4_096, 7.0)] {
            let audio: Vec<f32> =
                (0..len).map(|n| (n as f32 * freq * std::f32::consts::TAU / SAMPLE_RATE as f32).sin()).collect();
            let fast = zero_crossing_rate(&audio);
            let slow = reference_zcr(&audio);
            assert!((fast - slow).abs() < 1e-12, "len {len}: {fast} vs {slow}");
        }
    }
}
