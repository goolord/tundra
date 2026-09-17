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
        Self {
            message: message.into(),
            details: details.into(),
        }
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
    let tier1 = tier1::classify(path)?;
    if let Some(instrument) = tier1.instrument {
        let result = ClassificationResult {
            summary: format!(
                "Tier 1 · ZCR {zcr:.4} · {instrument}{confidence}",
                zcr = tier1.zcr,
                confidence = format_confidence(tier1.confidence),
            ),
            instrument,
            tier: 1,
            zcr: Some(tier1.zcr),
            confidence: tier1.confidence,
        };
        if let Some(stamp) = stamp {
            classify_cache::store_cached(path, stamp, &result);
        }
        return Ok(with_path_hint(path, result));
    }

    let tier2 = classifier_pool::classify_tier2(path, tier1.zcr)?;
    let yamnet = tier2.engine.as_deref() == Some("yamnet");
    let result = ClassificationResult {
        summary: format!(
            "Tier 1 grey (ZCR {zcr:.4}) → Tier 2 ({engine}) · {instrument}{confidence}",
            zcr = tier1.zcr,
            engine = if yamnet { "YAMNet" } else { "Librosa spectral" },
            instrument = tier2.instrument,
            confidence = format_confidence(tier2.confidence),
        ),
        instrument: tier2.instrument,
        tier: 2,
        zcr: tier2.zcr.or(Some(tier1.zcr)),
        confidence: tier2.confidence,
    };
    // Results from the librosa fallback (YAMNet missing or failing) are not
    // remembered, so the real model reclassifies once it is available.
    if let Some(stamp) = stamp.filter(|_| yamnet) {
        classify_cache::store_cached(path, stamp, &result);
    }
    Ok(with_path_hint(path, result))
}

/// `87%`, or `—` when there is no confidence.
pub fn confidence_percent(confidence: Option<f64>) -> String {
    confidence.map_or_else(|| "—".into(), |value| format!("{:.0}%", value * 100.0))
}

fn format_confidence(confidence: Option<f64>) -> String {
    confidence.map(|_| format!(" ({})", confidence_percent(confidence))).unwrap_or_default()
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

    fn classified(instrument: &str, tier: u8, confidence: f64) -> ClassificationResult {
        ClassificationResult {
            instrument: instrument.into(),
            tier,
            zcr: Some(0.01),
            confidence: Some(confidence),
            summary: format!("Tier {tier} · {instrument}"),
        }
    }

    #[test]
    fn folder_hint_overrides_even_a_confident_classifier() {
        let result = with_path_hint(Path::new("/Samples/Bongo/hit_01.wav"), classified("Kick", 1, 0.90));
        assert_eq!(result.instrument, "Percussion");
        assert_eq!(result.confidence, None);
        assert!(result.summary.contains("Path hint"));
    }

    #[test]
    fn related_hint_keeps_the_classifier_label() {
        let result = with_path_hint(Path::new("/hats/tight_01.wav"), classified("Closed Hat", 1, 0.9));
        assert_eq!(result.instrument, "Closed Hat");
        assert_eq!(result.summary, "Tier 1 · Closed Hat");
    }

    #[test]
    fn empty_classifier_label_takes_the_hint() {
        let result = with_path_hint(Path::new("/Samples/Snares/one.wav"), classified("", 2, 0.5));
        assert_eq!(result.instrument, "Snare");
    }

    #[test]
    fn no_hint_leaves_the_result_alone() {
        let result = with_path_hint(Path::new("/Samples/untitled.wav"), classified("Kick", 1, 0.9));
        assert_eq!((result.instrument.as_str(), result.confidence), ("Kick", Some(0.9)));
    }

    #[test]
    fn confidence_formats_as_a_percentage() {
        assert_eq!(confidence_percent(Some(0.874)), "87%");
        assert_eq!(confidence_percent(None), "—");
        assert_eq!(format_confidence(Some(0.5)), " (50%)");
        assert_eq!(format_confidence(None), "");
    }
}
