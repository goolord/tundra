//! Instrument classification for auto-tagging.
//!
//! Tier 1 (`tier1`) is a zero-crossing-rate heuristic in Rust that decides
//! clear kicks, basses, hi-hats, and cymbals. Everything else goes to tier 2,
//! YAMNet in long-lived Python workers (`classifier_pool`). Results are cached
//! per file version (`classify_cache`), and an instrument named by the file's
//! folder or name wins over any label the classifier is unsure of.

use serde::{Deserialize, Serialize};
use std::path::Path;

mod classifier_pool;
mod classify_cache;
mod tier1;

pub use classifier_pool::warm as warm_classifier_pool;
pub use classify_cache::{clear_cache as clear_classify_cache, flush_cache as flush_classify_cache};

pub const HIGH_CLASSIFIER_CONFIDENCE: f64 = 0.85;
/// Below this a suggestion is shown but not pre-selected for bulk apply.
pub const MEDIUM_CLASSIFIER_CONFIDENCE: f64 = 0.65;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub instrument: String,
    pub tier: u8,
    pub zcr: Option<f64>,
    pub confidence: Option<f64>,
    /// How the label was reached, for the details panel.
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct ClassifyError {
    /// Short, for the user.
    pub message: String,
    /// Technical, for the details panel and logs.
    pub details: String,
}

impl ClassifyError {
    pub fn new(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self { message: message.into(), details: details.into() }
    }

    fn analysis_failed(details: impl Into<String>) -> Self {
        Self::new("Couldn't analyze this file.", details)
    }
}

/// Classifies one file and saves the cache right away, so a manual Auto Tag
/// survives an app restart.
pub fn classify_file(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    let result = classify_file_bulk(path)?;
    classify_cache::flush_cache();
    Ok(result)
}

/// Like `classify_file`, but leaves saving the cache to the caller
/// (`flush_classify_cache`), so a bulk scan writes it once.
pub fn classify_file_bulk(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    if let Some(cached) = classify_cache::get_cached(path) {
        return Ok(with_path_hint(path, cached));
    }
    // Stamp before analysis: if the file changes while it is being analysed,
    // the result is stored against the old version and never matches.
    let stamp = crate::path_util::FileStamp::of(path);
    let zcr = tier1::file_zcr(path)?;
    let (result, cacheable) = if let Some((instrument, confidence)) = tier1::classify_zcr(zcr) {
        let summary = format!("Tier 1 · ZCR {zcr:.4} · {instrument}{}", format_confidence(Some(confidence)));
        let result = ClassificationResult {
            instrument: instrument.into(),
            tier: 1,
            zcr: Some(zcr),
            confidence: Some(confidence),
            summary,
        };
        (result, true)
    } else {
        let tier2 = classifier_pool::classify_tier2(path, zcr)?;
        let yamnet = tier2.engine.as_deref() == Some("yamnet");
        let summary = format!(
            "Tier 1 grey (ZCR {zcr:.4}) → Tier 2 ({engine}) · {instrument}{confidence}",
            engine = if yamnet { "YAMNet" } else { "Librosa spectral" },
            instrument = tier2.instrument,
            confidence = format_confidence(tier2.confidence),
        );
        let result = ClassificationResult {
            instrument: tier2.instrument,
            tier: 2,
            zcr: tier2.zcr.or(Some(zcr)),
            confidence: tier2.confidence,
            summary,
        };
        // Results from the librosa fallback (YAMNet missing or failing) are not
        // remembered, so the real model reclassifies once it is available.
        (result, yamnet)
    };
    if let Some(stamp) = stamp.filter(|_| cacheable) {
        classify_cache::store_cached(path, stamp, &result);
    }
    Ok(with_path_hint(path, result))
}

/// `87%`, or `—` when there is no confidence.
pub fn confidence_percent(confidence: Option<f64>) -> String {
    confidence.map_or_else(|| "—".into(), |value| format!("{:.0}%", value * 100.0))
}

fn format_confidence(confidence: Option<f64>) -> String {
    confidence.map_or_else(String::new, |value| format!(" ({:.0}%)", value * 100.0))
}

/// Replaces the classifier's label with an instrument the file's name or
/// folder names, unless the two are the same kind of instrument.
fn with_path_hint(path: &Path, mut result: ClassificationResult) -> ClassificationResult {
    let Some(hint) = crate::metadata::instrument_hint_from_path(path).filter(|hint| !hint.is_empty()) else {
        return result;
    };
    let classified = &result.instrument;
    if !classified.is_empty() && crate::metadata::instruments_related(&hint, classified) {
        return result;
    }
    result.summary = format!(
        "Path hint · {hint} · classifier {classified}{confidence} below high-confidence",
        confidence = format_confidence(result.confidence),
    );
    result.instrument = hint;
    result.confidence = None;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hint_replaces_only_unrelated_labels() {
        for (path, label, want, hinted) in [
            ("/Samples/Bongo/hit_01.wav", "Kick", "Percussion", true),
            ("/hats/tight_01.wav", "Closed Hat", "Closed Hat", false),
            ("/Samples/Snares/one.wav", "", "Snare", true),
            ("/Samples/untitled.wav", "Kick", "Kick", false),
        ] {
            let classified = ClassificationResult {
                instrument: label.into(),
                tier: 1,
                zcr: None,
                confidence: Some(0.9),
                summary: "classifier".into(),
            };
            let result = with_path_hint(Path::new(path), classified);
            assert_eq!(result.instrument, want, "{path}");
            assert_eq!(result.confidence.is_none(), hinted, "{path}");
            assert_eq!(result.summary.starts_with("Path hint"), hinted, "{path}");
        }
    }

    #[test]
    fn confidence_formats_as_a_percentage() {
        assert_eq!(confidence_percent(Some(0.874)), "87%");
        assert_eq!(confidence_percent(None), "—");
        assert_eq!(format_confidence(Some(0.5)), " (50%)");
        assert_eq!(format_confidence(None), "");
    }
}
