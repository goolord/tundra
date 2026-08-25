//! Rust tier 1 ZCR classifier. Threshold parity target: `scripts/tier1_zcr.py` (librosa).
//!
//! Sample rate matches tier 1 reference (22050 Hz). Tier 2 reuses this ZCR for grey-zone
//! files; Essentia/librosa inside tier 2 decode at 16 kHz independently.

use super::ClassifyError;
use rodio::Source;
use std::fs::File;
use std::path::Path;

pub const SAMPLE_RATE: u32 = 22050;
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
    pub decision: String,
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
    let zcr = zero_crossing_rate(&audio);
    Ok(classify_zcr(zcr))
}

fn classify_zcr(zcr: f64) -> Tier1Result {
    if zcr <= VERY_LOW_THRESHOLD {
        let confidence = (0.74 + (VERY_LOW_THRESHOLD - zcr) * 5.0).min(0.95);
        return Tier1Result {
            decision: "definitive".into(),
            instrument: Some("Kick".into()),
            zcr,
            confidence: Some(confidence),
        };
    }
    if zcr <= LOW_THRESHOLD {
        let confidence = (0.70 + (LOW_THRESHOLD - zcr) * 4.0).min(0.92);
        return Tier1Result {
            decision: "definitive".into(),
            instrument: Some("Bass".into()),
            zcr,
            confidence: Some(confidence),
        };
    }
    if zcr >= VERY_HIGH_THRESHOLD {
        let confidence = (0.74 + (zcr - VERY_HIGH_THRESHOLD) * 2.0).min(0.95);
        return Tier1Result {
            decision: "definitive".into(),
            instrument: Some("Cymbal".into()),
            zcr,
            confidence: Some(confidence),
        };
    }
    if zcr >= HIGH_THRESHOLD {
        let confidence = (0.70 + (zcr - HIGH_THRESHOLD) * 2.5).min(0.92);
        return Tier1Result {
            decision: "definitive".into(),
            instrument: Some("Hi-Hat".into()),
            zcr,
            confidence: Some(confidence),
        };
    }

    Tier1Result {
        decision: "grey".into(),
        instrument: None,
        zcr,
        confidence: None,
    }
}

fn load_mono_audio(path: &Path) -> Result<Vec<f32>, ClassifyError> {
    let file_len = std::fs::metadata(path)
        .map_err(|err| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                format!("Failed to stat {}: {err}", path.display()),
            )
        })?
        .len();
    if file_len > MAX_AUDIO_BYTES {
        return Err(ClassifyError::new(
            "Couldn't analyze this file.",
            format!(
                "{} is too large ({} MB; limit is {} MB)",
                path.display(),
                file_len / (1024 * 1024),
                MAX_AUDIO_BYTES / (1024 * 1024)
            ),
        ));
    }

    let file = File::open(path).map_err(|err| {
        ClassifyError::new(
            "Couldn't analyze this file.",
            format!("Failed to open {}: {err}", path.display()),
        )
    })?;
    let decoder = rodio::Decoder::try_from(file).map_err(|err| {
        ClassifyError::new(
            "Couldn't analyze this file.",
            format!("Cannot decode {}: {err}", path.display()),
        )
    })?;

    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels().max(1) as usize;
    let max_samples = (sample_rate as f32 * ANALYSIS_SECONDS).ceil() as usize * channels;

    let interleaved: Vec<f32> = decoder.take(max_samples).collect();
    if interleaved.is_empty() {
        return Err(ClassifyError::new(
            "Couldn't analyze this file.",
            format!("{} contains no audio samples", path.display()),
        ));
    }

    let frame_count = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frame_count);
    for chunk in interleaved.chunks(channels) {
        mono.push(chunk.iter().sum::<f32>() / channels as f32);
    }

    Ok(resample_linear(&mono, sample_rate, SAMPLE_RATE))
}

fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((input.len() as f64) / ratio).floor() as usize;
    if output_len == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(output_len);
    for out_idx in 0..output_len {
        let src_pos = out_idx as f64 * ratio;
        let base = src_pos.floor() as usize;
        let frac = src_pos - base as f64;
        let a = input[base];
        let b = input.get(base + 1).copied().unwrap_or(a);
        output.push(a + ((b - a) as f64 * frac) as f32);
    }
    output
}

fn zero_crossing_rate(audio: &[f32]) -> f64 {
    if audio.len() < 2 {
        return 0.0;
    }

    let mut padded = Vec::with_capacity(audio.len() + 1);
    padded.push(0.0);
    padded.extend_from_slice(audio);

    let crossings: Vec<f32> = padded
        .windows(2)
        .map(|window| {
            if window[0] * window[1] < 0.0 {
                1.0
            } else {
                0.0
            }
        })
        .collect();

    let frames = frame_signal(&crossings, FRAME_LENGTH, HOP_LENGTH, true);
    if frames.is_empty() {
        return 0.0;
    }

    let mut rates = Vec::with_capacity(frames.len());
    for frame in frames {
        let count: f32 = frame.iter().sum();
        rates.push(f64::from(count) / (FRAME_LENGTH - 1) as f64);
    }

    rates.iter().sum::<f64>() / rates.len() as f64
}

fn frame_signal(signal: &[f32], frame_length: usize, hop_length: usize, center: bool) -> Vec<Vec<f32>> {
    if signal.is_empty() || frame_length == 0 || hop_length == 0 {
        return Vec::new();
    }

    let padded = if center {
        reflect_pad(signal, frame_length / 2, frame_length / 2)
    } else {
        signal.to_vec()
    };

    if padded.len() < frame_length {
        return Vec::new();
    }

    let mut frames = Vec::new();
    let mut start = 0usize;
    while start + frame_length <= padded.len() {
        frames.push(padded[start..start + frame_length].to_vec());
        start += hop_length;
    }
    frames
}

fn reflect_pad(signal: &[f32], pad_left: usize, pad_right: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(pad_left + signal.len() + pad_right);

    for idx in (0..pad_left).rev() {
        let src = (idx + 1).min(signal.len().saturating_sub(1));
        out.push(signal[src]);
    }

    out.extend_from_slice(signal);

    for idx in 0..pad_right {
        let src = signal
            .len()
            .saturating_sub(2)
            .saturating_sub(idx);
        out.push(signal[src]);
    }

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
        assert_eq!(result.decision, "grey");
        assert!(result.instrument.is_none());
    }
}
