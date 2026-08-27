use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

mod classify_cache;
mod classifier_pool;
mod tier1;

pub use classify_cache::{clear_cache as clear_classify_cache, flush_cache as flush_classify_cache};
pub use classifier_pool::{
    shutdown as shutdown_classifier_pool, warm as warm_classifier_pool,
    worker_count as classifier_worker_count,
};

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

#[derive(Debug, Deserialize)]
struct Tier2CliResponse {
    instrument: String,
    confidence: Option<f64>,
    zcr: Option<f64>,
    engine: Option<String>,
}

const INSTALL_HINT: &str =
    "Install classifiers with: cargo xtask setup (or use a release package with scripts/.venv)";

pub fn bundled_python_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    const VENV_REL: &str = "scripts/.venv/Scripts/python.exe";
    #[cfg(not(windows))]
    const VENV_REL: &str = "scripts/.venv/bin/python3";

    crate::path_util::find_beside(&[VENV_REL], |candidate| candidate.is_file())
}

fn run_python_script(
    python: &Path,
    scripts_dir: &Path,
    script: &str,
    path: &Path,
) -> Result<std::process::Output, std::io::Error> {
    let script_path = scripts_dir.join(script);
    let mut command = Command::new(python);
    configure_classifier_command(&mut command);
    command
        .arg(&script_path)
        .current_dir(scripts_dir)
        .arg(path)
        .output()
}
pub const UV_PYTHON: &str = if cfg!(windows) { "3.12" } else { "3.14" };
/// Matches the bulk-tagger high-confidence badge (>= 85%).
const HIGH_CLASSIFIER_CONFIDENCE: f64 = 0.85;

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

    let tier1 = tier1::classify(path)?;
    if tier1.decision == "definitive" {
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
        classify_cache::store_cached(path, &result);
        return Ok(with_path_hint(path, result));
    }

    let tier2 = classify_tier2(path, tier1.zcr)?;
    let engine = tier2.engine.as_deref().unwrap_or("essentia");
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
    classify_cache::store_cached(path, &result);
    Ok(with_path_hint(path, result))
}

fn classify_tier2(
    path: &Path,
    tier1_zcr: f64,
) -> Result<classifier_pool::Tier2Response, ClassifyError> {
    match classifier_pool::classify_tier2(path, tier1_zcr) {
        Ok(response) => Ok(response),
        Err(worker_err) => {
            eprintln!(
                "classifier worker failed ({}); falling back to subprocess tier 2",
                worker_err.details
            );
            run_tier2_subprocess(path)
        }
    }
}

fn engine_label(engine: &str) -> &'static str {
    match engine {
        "tensorflow" => "Essentia TensorFlow",
        "essentia-spectral" => "Essentia spectral",
        "librosa-spectral" => "Librosa spectral",
        "librosa-fallback" => "Librosa spectral",
        _ => "Essentia",
    }
}

fn format_confidence(confidence: Option<f64>) -> String {
    confidence
        .map(|value| format!(" ({value:.0}%)", value = value * 100.0))
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

pub fn bundled_models_dir() -> Option<PathBuf> {
    const EFFNET: &str = "discogs-effnet-bs64-1.pb";
    const INSTRUMENT: &str = "mtg_jamendo_instrument-discogs-effnet-1.pb";
    const LABELS: &str = "mtg_jamendo_instrument-discogs-effnet-1.json";

    crate::path_util::find_beside(&["models", "resources/models"], |dir| {
        dir.join(EFFNET).is_file()
            && dir.join(INSTRUMENT).is_file()
            && dir.join(LABELS).is_file()
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
        ("TF_CPP_MIN_LOG_LEVEL", "3"),
        ("CUDA_VISIBLE_DEVICES", "-1"),
    ] {
        command.env(key, value);
    }
    if let Some(models) = bundled_models_dir() {
        command.env("ESSENTIA_MODELS", &models);
        command.env("TUNDRA_ESSENTIA_DL", "1");
    }
    crate::path_util::hide_console(command);
}

fn run_script(script: &str, path: &Path) -> Result<String, ClassifyError> {
    let scripts_dir = scripts_dir();
    let script_path = scripts_dir.join(script);
    if !script_path.is_file() {
        return Err(ClassifyError::new(
            "Couldn't analyze this file.",
            format!("Missing classifier script: {}", script_path.display()),
        ));
    }

    let mut attempts: Vec<String> = Vec::new();

    if let Some(python) = bundled_python_exe() {
        match run_python_script(&python, &scripts_dir, script, path) {
            Ok(output) if output.status.success() => {
                return String::from_utf8(output.stdout)
                    .map_err(|err| {
                        ClassifyError::new(
                            "Couldn't analyze this file.",
                            format!("Invalid UTF-8 from {script}: {err}"),
                        )
                    })
                    .map(|text| text.trim().to_string());
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                attempts.push(if stderr.is_empty() {
                    format!(
                        "{} {script} exited with status {}",
                        python.display(),
                        output.status
                    )
                } else {
                    format!("{} {script}: {stderr}", python.display())
                });
            }
            Err(err) => {
                attempts.push(format!("{}: {err}", python.display()));
            }
        }
    }

    if let Some(output) = try_uv_run(&scripts_dir, script, path) {
        match output {
            Ok(stdout) => return Ok(stdout),
            Err(err) => attempts.push(format!("uv run: {err}")),
        }
    } else {
        attempts.push("uv not found".to_string());
    }

    #[cfg(not(windows))]
    for python in ["python3", "python"] {
        let mut command = Command::new(python);
        configure_classifier_command(&mut command);
        match command
            .arg(&script_path)
            .current_dir(&scripts_dir)
            .arg(path)
            .output()
        {
            Ok(output) if output.status.success() => {
                return String::from_utf8(output.stdout)
                    .map_err(|err| {
                        ClassifyError::new(
                            "Couldn't analyze this file.",
                            format!("Invalid UTF-8 from {script}: {err}"),
                        )
                    })
                    .map(|text| text.trim().to_string());
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                attempts.push(if stderr.is_empty() {
                    format!("{python} {script} exited with status {}", output.status)
                } else {
                    format!("{python} {script}: {stderr}")
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                attempts.push(format!("{python} not found"));
            }
            Err(err) => {
                return Err(ClassifyError::new(
                    "Couldn't analyze this file.",
                    format!("Failed to launch {python} for {script}: {err}"),
                ));
            }
        }
    }

    Err(ClassifyError::new(
        "Couldn't analyze this file.",
        format!(
            "Could not run {script}. {INSTALL_HINT}. {}",
            attempts.join("; ")
        ),
    ))
}

fn try_uv_run(scripts_dir: &Path, script: &str, path: &Path) -> Option<Result<String, String>> {
    let mut command = Command::new("uv");
    command
        .current_dir(scripts_dir)
        .arg("run")
        .arg("--python")
        .arg(UV_PYTHON)
        .arg(script)
        .arg(path);
    configure_classifier_command(&mut command);
    let output = command.output().ok()?;

    if output.status.success() {
        Some(
            String::from_utf8(output.stdout)
                .map_err(|err| format!("Invalid UTF-8 from uv run {script}: {err}"))
                .map(|text| text.trim().to_string()),
        )
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Some(Err(if stderr.is_empty() {
            format!("exited with status {}", output.status)
        } else {
            stderr
        }))
    }
}

fn run_tier2_subprocess(path: &Path) -> Result<classifier_pool::Tier2Response, ClassifyError> {
    let stdout = run_script("tier2_essentia.py", path)?;
    let parsed: Tier2CliResponse = serde_json::from_str(&stdout).map_err(|err| {
        ClassifyError::new(
            "Analysis returned unexpected data.",
            format!("Invalid tier 2 output: {err}"),
        )
    })?;
    Ok(classifier_pool::Tier2Response {
        instrument: parsed.instrument,
        confidence: parsed.confidence,
        zcr: parsed.zcr,
        engine: parsed.engine,
    })
}

pub fn classify_file_blocking(path: PathBuf) -> Result<ClassificationResult, ClassifyError> {
    classify_file(&path)
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
        let result = with_path_hint(Path::new(r"F:\Samples\Bongo\hit_01.wav"), result);
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
        let result = with_path_hint(Path::new(r"C:\hats\tight_01.wav"), result);
        assert_eq!(result.instrument, "Closed Hat");
        assert!(!result.summary.contains("overridden"));
    }
}
