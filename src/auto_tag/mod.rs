use std::path::{Path, PathBuf};
use std::process::Command;

mod classify_cache;
mod classifier_pool;
mod tier1;

pub use classify_cache::{clear_cache as clear_classify_cache, flush_cache as flush_classify_cache};
pub use classifier_pool::warm as warm_classifier_pool;

#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub instrument: String,
    pub tier: u8,
    pub zcr: Option<f64>,
    pub confidence: Option<f64>,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct ClassifyError {
    pub message: String,
    pub details: String,
}

impl ClassifyError {
    pub fn new(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: details.into(),
        }
    }
}

pub(crate) const INSTALL_HINT: &str =
    "Install classifiers with `cargo xtask setup`, or use a release package that includes python/";

/// A Python that ships with Tundra, if any: the standalone interpreter plus
/// `site-packages` from a release package, or the `scripts/.venv` that
/// `cargo xtask setup` creates for development.
pub struct BundledPython {
    pub exe: PathBuf,
    /// Set as PYTHONPATH so the interpreter finds the packaged dependencies.
    pub site_packages: Option<PathBuf>,
}

pub fn bundled_python() -> Option<BundledPython> {
    let packaged = crate::path_util::find_beside(&["python"], |dir| {
        dir.join("site-packages").is_dir()
    })
    .and_then(|python| {
        let exe = std::fs::read_dir(&python)
            .ok()?
            .flatten()
            .flat_map(|entry| {
                let dir = entry.path();
                [dir.join("python.exe"), dir.join("bin").join("python3")]
            })
            .find(|candidate| candidate.is_file())?;
        Some(BundledPython {
            exe,
            site_packages: Some(python.join("site-packages")),
        })
    });
    packaged.or_else(|| {
        #[cfg(windows)]
        const VENV_REL: &str = "scripts/.venv/Scripts/python.exe";
        #[cfg(not(windows))]
        const VENV_REL: &str = "scripts/.venv/bin/python3";
        crate::path_util::find_beside(&[VENV_REL], |candidate| candidate.is_file()).map(|exe| {
            BundledPython {
                exe,
                site_packages: None,
            }
        })
    })
}

/// Must match `scripts/.python-version` and xtask's `PYTHON_VERSION`.
pub const UV_PYTHON: &str = "3.12";
pub const HIGH_CLASSIFIER_CONFIDENCE: f64 = 0.85;
/// Below this a suggestion is shown but not pre-selected for bulk apply.
pub const MEDIUM_CLASSIFIER_CONFIDENCE: f64 = 0.65;

/// Single-file path persists cache immediately so manual Auto Tag survives app restarts.
pub fn classify_file(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    let result = classify_file_inner(path)?;
    classify_cache::flush_cache();
    Ok(result)
}

pub fn classify_file_bulk(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    classify_file_inner(path)
}

fn cached_with_path_hint(path: &Path) -> Option<ClassificationResult> {
    classify_cache::get_cached(path).map(|result| with_path_hint(path, result))
}

fn classify_file_inner(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    if let Some(cached) = cached_with_path_hint(path) {
        return Ok(cached);
    }

    // Stamp before analysis: if the file changes while it is being analysed,
    // the result is stored against the old version and never matches.
    let stamp = classify_cache::FileStamp::of(path);
    let tier1 = tier1::classify(path)?;
    if tier1.instrument.is_some() {
        let instrument = tier1.instrument.ok_or_else(|| {
            ClassifyError::new(
                "Couldn't determine an instrument.",
                "Tier 1 returned no instrument label",
            )
        })?;
        let confidence = tier1.confidence;
        let result = ClassificationResult {
            instrument: instrument.clone(),
            tier: 1,
            zcr: Some(tier1.zcr),
            confidence,
            summary: format!(
                "Tier 1 · ZCR {zcr:.4} · {instrument}{confidence}",
                zcr = tier1.zcr,
                confidence = format_confidence(confidence),
            ),
        };
        if let Some(stamp) = stamp {
            classify_cache::store_cached(path, stamp, &result);
        }
        return Ok(with_path_hint(path, result));
    }

    let tier2 = classifier_pool::classify_tier2(path, tier1.zcr)?;
    let engine = tier2.engine.as_deref().unwrap_or_default();
    let result = ClassificationResult {
        instrument: tier2.instrument.clone(),
        tier: 2,
        zcr: tier2.zcr.or(Some(tier1.zcr)),
        confidence: tier2.confidence,
        summary: format!(
            "Tier 1 grey (ZCR {zcr:.4}) → Tier 2 ({engine}) · {instrument}{confidence}",
            zcr = tier1.zcr,
            engine = engine_label(engine),
            instrument = tier2.instrument,
            confidence = format_confidence(tier2.confidence),
        ),
    };
    // Results from the librosa fallback (YAMNet missing or failing) are not
    // remembered, so the real model reclassifies once it is available.
    if let Some(stamp) = stamp.filter(|_| engine == "yamnet") {
        classify_cache::store_cached(path, stamp, &result);
    }
    Ok(with_path_hint(path, result))
}

fn engine_label(engine: &str) -> &'static str {
    match engine {
        "yamnet" => "YAMNet",
        _ => "Librosa spectral",
    }
}

fn format_percent(confidence: Option<f64>) -> Option<String> {
    confidence.map(|value| format!("{:.0}%", value * 100.0))
}

pub fn confidence_percent(confidence: Option<f64>) -> String {
    format_percent(confidence).unwrap_or_else(|| "—".into())
}

fn format_confidence(confidence: Option<f64>) -> String {
    format_percent(confidence)
        .map(|percent| format!(" ({percent})"))
        .unwrap_or_default()
}

fn choose_instrument(
    hint: Option<&str>,
    classified: &str,
    confidence: Option<f64>,
    tier: u8,
    source: crate::metadata::HintSource,
) -> (String, bool) {
    let Some(hint) = hint.filter(|value| !value.is_empty()) else {
        return (classified.to_string(), false);
    };
    if classified.is_empty() {
        return (hint.to_string(), true);
    }
    if crate::metadata::instruments_related(hint, classified) {
        return (classified.to_string(), false);
    }
    if source == crate::metadata::HintSource::Path {
        return (hint.to_string(), true);
    }
    if confidence.is_some_and(|value| value >= HIGH_CLASSIFIER_CONFIDENCE) {
        return (classified.to_string(), false);
    }
    if tier <= 1 {
        return (classified.to_string(), false);
    }
    (hint.to_string(), true)
}

fn with_path_hint(path: &Path, result: ClassificationResult) -> ClassificationResult {
    apply_hint(
        crate::metadata::instrument_hint_from_path(path),
        crate::metadata::HintSource::Path,
        result,
    )
}

fn apply_hint(
    hint: Option<String>,
    source: crate::metadata::HintSource,
    mut result: ClassificationResult,
) -> ClassificationResult {
    let classified = result.instrument.clone();
    let (instrument, used_hint) =
        choose_instrument(hint.as_deref(), &classified, result.confidence, result.tier, source);
    if used_hint {
        result.summary = format!(
            "{source} · {instrument} · classifier {classified}{confidence} below high-confidence",
            source = source.label(),
            confidence = format_confidence(result.confidence),
        );
        result.instrument = instrument;
        result.confidence = None;
    } else if hint.as_deref().is_some_and(|hint| {
        hint != classified && !crate::metadata::instruments_related(hint, &classified)
    }) {
        result.summary = format!(
            "{summary} · {source} {hint} overridden",
            summary = result.summary,
            source = source.label().to_ascii_lowercase(),
            hint = hint.unwrap_or_default(),
        );
    }
    result
}

pub fn scripts_dir() -> PathBuf {
    const WORKER: &str = "classifier_worker.py";
    crate::path_util::find_beside(&["scripts"], |dir| dir.join(WORKER).is_file())
        .or_else(|| {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
            dir.is_dir().then_some(dir)
        })
        .unwrap_or_else(|| PathBuf::from("scripts"))
}

/// Directory holding the YAMNet model and its class map (see `tools/yamnet`).
pub fn bundled_models_dir() -> Option<PathBuf> {
    crate::path_util::find_beside(&["models", "resources/models"], |dir| {
        dir.join("yamnet.onnx").is_file() && dir.join("yamnet_class_map.csv").is_file()
    })
}

pub fn configure_classifier_command(command: &mut Command) {
    // Cap BLAS/OpenMP threads per subprocess so bulk parallel runs stay polite.
    // CUDA_VISIBLE_DEVICES only affects this child env (classifier may still ignore it).
    for (key, value) in [
        ("OMP_NUM_THREADS", "1"),
        ("OPENBLAS_NUM_THREADS", "1"),
        ("MKL_NUM_THREADS", "1"),
        ("VECLIB_MAXIMUM_THREADS", "1"),
        ("NUMEXPR_NUM_THREADS", "1"),
        ("CUDA_VISIBLE_DEVICES", "-1"),
    ] {
        command.env(key, value);
    }
    // Paths cross the pipe as UTF-8 whatever the system code page is.
    command.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
    if let Some(models) = bundled_models_dir() {
        command.env("TUNDRA_MODELS", &models);
        command.env("TUNDRA_ONNX_DL", "1");
    }
    crate::path_util::hide_console(command);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hint_wins_when_classifier_is_not_high_confidence() {
        let (instrument, used_hint) =
            choose_instrument(Some("Snare"), "Cymbal", Some(0.74), 2, crate::metadata::HintSource::Tags);
        assert_eq!(instrument, "Snare");
        assert!(used_hint);
    }

    #[test]
    fn classifier_wins_when_confidence_is_high() {
        let (instrument, used_hint) =
            choose_instrument(Some("Snare"), "Cymbal", Some(0.90), 2, crate::metadata::HintSource::Tags);
        assert_eq!(instrument, "Cymbal");
        assert!(!used_hint);
    }

    #[test]
    fn related_hint_keeps_classifier_label() {
        let (instrument, used_hint) =
            choose_instrument(Some("Hat"), "Hi-Hat", Some(0.72), 2, crate::metadata::HintSource::Tags);
        assert_eq!(instrument, "Hi-Hat");
        assert!(!used_hint);
    }

    #[test]
    fn missing_confidence_uses_path_hint_for_tier2() {
        let (instrument, used_hint) =
            choose_instrument(Some("Snare"), "Cymbal", None, 2, crate::metadata::HintSource::Tags);
        assert_eq!(instrument, "Snare");
        assert!(used_hint);
    }

    #[test]
    fn tier1_without_confidence_keeps_classifier_label_for_tag_hints() {
        let (instrument, used_hint) =
            choose_instrument(Some("Snare"), "Cymbal", None, 1, crate::metadata::HintSource::Tags);
        assert_eq!(instrument, "Cymbal");
        assert!(!used_hint);
    }

    #[test]
    fn path_hint_overrides_tier1_when_folder_names_instrument() {
        let (instrument, used_hint) =
            choose_instrument(Some("Percussion"), "Kick", Some(0.90), 1, crate::metadata::HintSource::Path);
        assert_eq!(instrument, "Percussion");
        assert!(used_hint);
    }

    #[test]
    fn empty_classifier_uses_path_hint() {
        let (instrument, used_hint) =
            choose_instrument(Some("Snare"), "", None, 2, crate::metadata::HintSource::Path);
        assert_eq!(instrument, "Snare");
        assert!(used_hint);
    }

    #[test]
    fn path_hint_overrides_tier1_kick_for_bongo_folder() {
        let result = ClassificationResult {
            instrument: "Kick".into(),
            tier: 1,
            zcr: Some(0.01),
            confidence: Some(0.90),
            summary: "Tier 1 · Kick (90%)".into(),
        };
        let result = with_path_hint(Path::new("/Samples/Bongo/hit_01.wav"), result);
        assert_eq!(result.instrument, "Percussion");
        assert!(result.summary.contains("Path hint"));
    }

    #[test]
    fn related_hint_does_not_mark_summary_overridden() {
        let result = ClassificationResult {
            instrument: "Closed Hat".into(),
            tier: 1,
            zcr: Some(0.1),
            confidence: Some(0.9),
            summary: "Tier 1 · Closed Hat (90%)".into(),
        };
        let result = with_path_hint(Path::new("/hats/tight_01.wav"), result);
        assert_eq!(result.instrument, "Closed Hat");
        assert!(!result.summary.contains("overridden"));
    }
}
