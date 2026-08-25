use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    fn new(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: details.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Tier1Response {
    decision: String,
    instrument: Option<String>,
    zcr: f64,
    confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct Tier2Response {
    instrument: String,
    confidence: Option<f64>,
    zcr: Option<f64>,
    engine: Option<String>,
}

const INSTALL_HINT: &str = "Install classifiers with: cargo xtask setup";

pub fn classify_file(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    if !path.is_file() {
        return Err(ClassifyError::new(
            "Couldn't find that audio file.",
            format!("File not found: {}", path.display()),
        ));
    }

    let tier1 = run_tier1(path)?;
    if tier1.decision == "definitive" {
        let instrument = tier1.instrument.ok_or_else(|| {
            ClassifyError::new(
                "Couldn't determine an instrument.",
                "Tier 1 returned no instrument label",
            )
        })?;
        let confidence = tier1.confidence;
        return Ok(ClassificationResult {
            instrument: instrument.clone(),
            tier: 1,
            zcr: Some(tier1.zcr),
            confidence,
            summary: format!(
                "Tier 1 · ZCR {zcr:.4} · {instrument}{confidence}",
                zcr = tier1.zcr,
                confidence = format_confidence(confidence),
            ),
        });
    }

    let tier2 = run_tier2(path)?;
    let engine = tier2.engine.as_deref().unwrap_or("essentia");
    Ok(ClassificationResult {
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
    })
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

fn scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

pub fn bundled_models_dir() -> Option<PathBuf> {
    const EFFNET: &str = "discogs-effnet-bs64-1.pb";
    const INSTRUMENT: &str = "mtg_jamendo_instrument-discogs-effnet-1.pb";
    const LABELS: &str = "mtg_jamendo_instrument-discogs-effnet-1.json";

    let has_models = |dir: &Path| {
        dir.join(EFFNET).is_file()
            && dir.join(INSTRUMENT).is_file()
            && dir.join(LABELS).is_file()
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for models in [parent.join("models"), parent.join("resources/models")] {
                if has_models(&models) {
                    return Some(models);
                }
            }
        }
    }

    let manifest_models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/models");
    if has_models(&manifest_models) {
        return Some(manifest_models);
    }

    None
}

fn configure_classifier_command(command: &mut Command) {
    if let Some(models) = bundled_models_dir() {
        command.env("ESSENTIA_MODELS", &models);
        command.env("TUNDRA_ESSENTIA_DL", "1");
    }
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

    if let Some(output) = try_uv_run(&scripts_dir, script, path) {
        match output {
            Ok(stdout) => return Ok(stdout),
            Err(err) => attempts.push(format!("uv run: {err}")),
        }
    } else {
        attempts.push("uv not found".to_string());
    }

    for python in ["python3", "python"] {
        let mut command = Command::new(python);
        configure_classifier_command(&mut command);
        match command.arg(&script_path).arg(path).output() {
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

fn run_tier1(path: &Path) -> Result<Tier1Response, ClassifyError> {
    let stdout = run_script("tier1_zcr.py", path)?;
    serde_json::from_str(&stdout).map_err(|err| {
        ClassifyError::new(
            "Analysis returned unexpected data.",
            format!("Invalid tier 1 output: {err}"),
        )
    })
}

fn run_tier2(path: &Path) -> Result<Tier2Response, ClassifyError> {
    let stdout = run_script("tier2_essentia.py", path)?;
    serde_json::from_str(&stdout).map_err(|err| {
        ClassifyError::new(
            "Analysis returned unexpected data.",
            format!("Invalid tier 2 output: {err}"),
        )
    })
}

pub fn classify_file_blocking(path: PathBuf) -> Result<ClassificationResult, ClassifyError> {
    classify_file(&path)
}
