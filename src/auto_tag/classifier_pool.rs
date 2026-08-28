use super::{bundled_python_exe, configure_classifier_command, scripts_dir, ClassifyError};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const WORKER_READ_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Deserialize)]
struct WorkerReady {
    ready: bool,
    error: Option<String>,
    #[serde(default)]
    onnx: bool,
}

#[derive(Debug, Deserialize)]
struct WorkerResponse {
    ok: bool,
    result: Option<Tier2Response>,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tier2Response {
    pub instrument: String,
    pub confidence: Option<f64>,
    pub zcr: Option<f64>,
    pub engine: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkerRequest<'a> {
    path: &'a str,
    tier1_zcr: f64,
}

enum ReaderMsg {
    Line(String),
    Closed,
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    line_rx: mpsc::Receiver<ReaderMsg>,
    _reader: std::thread::JoinHandle<()>,
}

impl Worker {
    fn spawn() -> Result<Self, ClassifyError> {
        let scripts_dir = scripts_dir();
        let mut last_error = String::from("no launch attempts");

        if let Some(python) = bundled_python_exe() {
            match try_spawn_python_worker(&scripts_dir, &python) {
                Ok(mut worker) => match worker.wait_for_ready() {
                    Ok(()) => return Ok(worker),
                    Err(err) => {
                        let _ = worker.child.kill();
                        last_error = err.details;
                    }
                },
                Err(err) => last_error = err.details,
            }
        }

        if let Some(mut worker) = try_spawn_uv_worker(&scripts_dir) {
            match worker.wait_for_ready() {
                Ok(()) => return Ok(worker),
                Err(err) => {
                    let _ = worker.child.kill();
                    last_error = err.details;
                }
            }
        }

        #[cfg(not(windows))]
        for python in ["python3", "python"] {
            match try_spawn_python_worker(&scripts_dir, Path::new(python)) {
                Ok(mut worker) => match worker.wait_for_ready() {
                    Ok(()) => return Ok(worker),
                    Err(err) => {
                        let _ = worker.child.kill();
                        last_error = err.details;
                    }
                },
                Err(err) => last_error = err.details,
            }
        }

        Err(ClassifyError::new(
            "Couldn't start classifier worker.",
            last_error,
        ))
    }

    fn wait_for_ready(&mut self) -> Result<(), ClassifyError> {
        let deadline = std::time::Instant::now() + WORKER_READ_TIMEOUT;
        loop {
            if std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                return Err(ClassifyError::new(
                    "Classifier worker failed to start.",
                    "Timed out waiting for worker ready line",
                ));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = match self.line_rx.recv_timeout(remaining) {
                Ok(ReaderMsg::Line(line)) => line,
                Ok(ReaderMsg::Closed) => {
                    return Err(ClassifyError::new(
                        "Classifier worker failed to start.",
                        "Worker closed stdout before ready",
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return Err(ClassifyError::new(
                        "Classifier worker failed to start.",
                        "Timed out waiting for worker ready line",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ClassifyError::new(
                        "Classifier worker failed to start.",
                        "Worker reader disconnected before ready",
                    ));
                }
            };
            if line.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<WorkerReady>(&line) {
                if !parsed.ready {
                    return Err(ClassifyError::new(
                        "Classifier worker failed to start.",
                        parsed.error.unwrap_or_else(|| "unknown worker error".into()),
                    ));
                }
                if !parsed.onnx {
                    eprintln!(
                        "classifier worker: ONNX unavailable; grey-zone files use librosa tier 2"
                    );
                }
                return Ok(());
            }
            eprintln!("classifier worker stdout (ignored before ready): {line}");
        }
    }

    fn classify(&mut self, path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
        if self
            .child
            .try_wait()
            .map_err(|err| {
                ClassifyError::new(
                    "Couldn't analyze this file.",
                    format!("Worker status check failed: {err}"),
                )
            })?
            .is_some()
        {
            *self = Self::spawn()?;
        }

        let path_str = crate::path_util::normalize_path(path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        if path_str.contains('\n') || path_str.contains('\r') {
            return Err(ClassifyError::new(
                "Couldn't analyze this file.",
                "Path contains unsupported control characters",
            ));
        }

        let request = WorkerRequest {
            path: &path_str,
            tier1_zcr,
        };
        let payload = serde_json::to_string(&request).map_err(|err| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                format!("Failed to encode worker request: {err}"),
            )
        })?;
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                "Classifier worker stdin closed",
            )
        })?;
        stdin
            .write_all(format!("{payload}\n").as_bytes())
            .map_err(|err| {
                ClassifyError::new(
                    "Couldn't analyze this file.",
                    format!("Failed to write to classifier worker: {err}"),
                )
            })?;
        stdin.flush().map_err(|err| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                format!("Failed to flush classifier worker stdin: {err}"),
            )
        })?;

        let line = self.read_line()?;
        let parsed: WorkerResponse = serde_json::from_str(&line).map_err(|err| {
            ClassifyError::new(
                "Analysis returned unexpected data.",
                format!("Invalid worker output: {err}; line={line:?}"),
            )
        })?;
        if parsed.ok {
            parsed.result.ok_or_else(|| {
                ClassifyError::new(
                    "Analysis returned unexpected data.",
                    "Worker success response missing result",
                )
            })
        } else {
            Err(ClassifyError::new(
                "Couldn't analyze this file.",
                parsed
                    .error
                    .unwrap_or_else(|| "Unknown worker error".into()),
            ))
        }
    }

    fn read_line(&mut self) -> Result<String, ClassifyError> {
        match self.line_rx.recv_timeout(WORKER_READ_TIMEOUT) {
            Ok(ReaderMsg::Line(line)) => Ok(line),
            Ok(ReaderMsg::Closed) => Err(ClassifyError::new(
                "Couldn't analyze this file.",
                "Classifier worker closed stdout",
            )),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
                // Dead worker; next `classify` respawns via `try_wait` at entry.
                Err(ClassifyError::new(
                    "Couldn't analyze this file.",
                    "Classifier worker timed out",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => Err(ClassifyError::new(
                "Couldn't analyze this file.",
                "Classifier worker reader disconnected",
            )),
        }
    }

    fn shutdown(&mut self) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(b"{\"quit\":true}\n");
            let _ = stdin.flush();
        }
        self.stdin = None;
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct WorkerSlot {
    worker: Mutex<Worker>,
}

struct ClassifierPool {
    workers: Mutex<Vec<Arc<WorkerSlot>>>,
    next: AtomicUsize,
}

impl ClassifierPool {
    fn new() -> Self {
        Self {
            workers: Mutex::new(Vec::new()),
            next: AtomicUsize::new(0),
        }
    }

    fn shutdown(&self) {
        if let Ok(mut workers) = self.workers.lock() {
            workers.clear();
        }
    }

    fn ensure_workers(&self, count: usize) -> Result<(), ClassifyError> {
        let mut workers = self.workers.lock().map_err(|_| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                "Classifier worker pool lock poisoned",
            )
        })?;
        while workers.len() < count {
            workers.push(Arc::new(WorkerSlot {
                worker: Mutex::new(Worker::spawn()?),
            }));
        }
        Ok(())
    }

    fn classify_tier2(&self, path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
        self.ensure_workers(worker_count())?;
        let worker_index = self.next.fetch_add(1, Ordering::Relaxed) % worker_count();
        let worker_slot = {
            let workers = self.workers.lock().map_err(|_| {
                ClassifyError::new(
                    "Couldn't analyze this file.",
                    "Classifier worker pool lock poisoned",
                )
            })?;
            Arc::clone(&workers[worker_index])
        };
        let mut worker = worker_slot.worker.lock().map_err(|_| {
            ClassifyError::new(
                "Couldn't analyze this file.",
                "Classifier worker lock poisoned",
            )
        })?;
        worker.classify(path, tier1_zcr)
    }
}

static CLASSIFIER_POOL: LazyLock<ClassifierPool> = LazyLock::new(ClassifierPool::new);

/// Cap at two workers: each loads TensorFlow/librosa and is memory-heavy.
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| (count.get() / 2).clamp(1, 2))
        .unwrap_or(1)
}

pub fn warm() -> Result<(), ClassifyError> {
    CLASSIFIER_POOL.ensure_workers(worker_count())
}

pub fn classify_tier2(path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
    CLASSIFIER_POOL.classify_tier2(path, tier1_zcr)
}

pub fn shutdown() {
    CLASSIFIER_POOL.shutdown();
}

fn spawn_worker(mut command: Command) -> Result<Worker, ClassifyError> {
    let mut child = command.spawn().map_err(|err| {
        ClassifyError::new(
            "Couldn't start classifier worker.",
            format!("Failed to spawn worker: {err}"),
        )
    })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        ClassifyError::new(
            "Couldn't start classifier worker.",
            "Worker stdin unavailable",
        )
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ClassifyError::new(
            "Couldn't start classifier worker.",
            "Worker stdout unavailable",
        )
    })?;

    let (line_tx, line_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = line_tx.send(ReaderMsg::Closed);
                    break;
                }
                Ok(_) => {
                    if line_tx
                        .send(ReaderMsg::Line(line.trim().to_string()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => {
                    let _ = line_tx.send(ReaderMsg::Closed);
                    break;
                }
            }
        }
    });

    Ok(Worker {
        child,
        stdin: Some(stdin),
        line_rx,
        _reader: reader,
    })
}

fn try_spawn_uv_worker(scripts_dir: &Path) -> Option<Worker> {
    let mut command = Command::new("uv");
    command
        .current_dir(scripts_dir)
        .arg("run")
        .arg("--python")
        .arg(super::UV_PYTHON)
        .arg("classifier_worker.py")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    configure_classifier_command(&mut command);
    spawn_worker(command).ok()
}

fn try_spawn_python_worker(scripts_dir: &Path, python: &Path) -> Result<Worker, ClassifyError> {
    let script = scripts_dir.join("classifier_worker.py");
    let mut command = Command::new(python);
    command
        .arg(script)
        .current_dir(scripts_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    configure_classifier_command(&mut command);
    spawn_worker(command)
}
