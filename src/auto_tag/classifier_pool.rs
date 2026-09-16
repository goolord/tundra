//! Long-lived Python tier-2 workers speaking JSON lines over stdio.
//!
//! Every request carries an id that the reply must echo. Anything else on the
//! protocol stream (a stray print, a reply to an earlier request) retires the
//! worker, so one file's label can never be attributed to another file.

use super::{bundled_python, configure_classifier_command, scripts_dir, ClassifyError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Each worker holds the ONNX models in memory; release them when unused.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
struct WorkerReady {
    ready: bool,
    error: Option<String>,
    #[serde(default)]
    onnx: bool,
}

#[derive(Debug, Deserialize)]
struct WorkerResponse {
    id: Option<u64>,
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
    id: u64,
    path: &'a str,
    tier1_zcr: f64,
}

fn failure(details: impl Into<String>) -> ClassifyError {
    ClassifyError::new("Couldn't analyze this file.", details)
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    last_used: Instant,
}

impl Worker {
    fn spawn() -> Result<Self, ClassifyError> {
        let mut attempts = Vec::new();
        for (label, command) in launch_commands() {
            match Self::start(command).and_then(|mut worker| {
                worker.wait_for_ready()?;
                Ok(worker)
            }) {
                Ok(worker) => return Ok(worker),
                Err(err) => attempts.push(format!("{label}: {}", err.details)),
            }
        }
        Err(ClassifyError::new(
            "Couldn't start classifier worker.",
            format!("{}. {}", super::INSTALL_HINT, attempts.join("; ")),
        ))
    }

    fn start(mut command: Command) -> Result<Self, ClassifyError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|err| failure(format!("Failed to spawn worker: {err}")))?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        // Drain stderr so a chatty worker can never fill the pipe and stall.
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("classifier worker: {line}");
            }
        });
        let (line_tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            lines,
            last_used: Instant::now(),
        })
    }

    fn next_line(&self, deadline: Instant) -> Result<String, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(remaining) {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err("Classifier worker timed out".into()),
            Err(RecvTimeoutError::Disconnected) => Err("Classifier worker exited".into()),
        }
    }

    fn wait_for_ready(&mut self) -> Result<(), ClassifyError> {
        let line = self
            .next_line(Instant::now() + READY_TIMEOUT)
            .map_err(failure)?;
        let ready: WorkerReady = serde_json::from_str(&line)
            .map_err(|err| failure(format!("Unexpected worker greeting {line:?}: {err}")))?;
        if !ready.ready {
            return Err(failure(
                ready.error.unwrap_or_else(|| "unknown worker error".into()),
            ));
        }
        if !ready.onnx {
            eprintln!("classifier worker: ONNX unavailable; grey-zone files use librosa tier 2");
        }
        Ok(())
    }

    /// The outer `Err` means the worker can no longer be trusted and must be
    /// replaced; an inner `Err` is an ordinary per-file failure.
    fn classify(
        &mut self,
        path: &Path,
        tier1_zcr: f64,
    ) -> Result<Result<Tier2Response, ClassifyError>, ClassifyError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.last_used = Instant::now();

        let path_str = crate::path_util::normalize_path(path.to_path_buf())
            .to_string_lossy()
            .into_owned();
        let request = WorkerRequest {
            id,
            path: &path_str,
            tier1_zcr,
        };
        let mut payload = serde_json::to_vec(&request)
            .map_err(|err| failure(format!("Failed to encode worker request: {err}")))?;
        payload.push(b'\n');
        self.stdin
            .write_all(&payload)
            .and_then(|()| self.stdin.flush())
            .map_err(|err| failure(format!("Failed to write to classifier worker: {err}")))?;

        let line = self
            .next_line(Instant::now() + REQUEST_TIMEOUT)
            .map_err(failure)?;
        let response: WorkerResponse = serde_json::from_str(&line)
            .map_err(|err| failure(format!("Invalid worker output {line:?}: {err}")))?;
        if response.id != Some(id) {
            return Err(failure(format!(
                "Worker replied to request {:?} while {id} was pending",
                response.id
            )));
        }
        self.last_used = Instant::now();

        Ok(if response.ok {
            response
                .result
                .ok_or_else(|| failure("Worker success response missing result"))
        } else {
            Err(failure(
                response.error.unwrap_or_else(|| "Unknown worker error".into()),
            ))
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing stdin ends the worker's read loop; kill covers a hung one.
        let _ = self.stdin.write_all(b"{\"quit\":true}\n");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Interpreters to try, in order: the bundled Python, `uv run`, then a system
/// Python on Unix.
fn launch_commands() -> Vec<(String, Command)> {
    let scripts_dir = scripts_dir();
    let script = scripts_dir.join("classifier_worker.py");
    let mut commands = Vec::new();

    let python = |program: &Path| {
        let mut command = Command::new(program);
        command.arg(&script).current_dir(&scripts_dir);
        command
    };
    if let Some(bundled) = bundled_python() {
        let mut command = python(&bundled.exe);
        if let Some(site_packages) = &bundled.site_packages {
            command.env("PYTHONPATH", site_packages);
        }
        commands.push((bundled.exe.display().to_string(), command));
    }
    let mut uv = Command::new("uv");
    uv.current_dir(&scripts_dir)
        .args(["run", "--python", super::UV_PYTHON, "classifier_worker.py"]);
    commands.push(("uv run".to_string(), uv));
    #[cfg(not(windows))]
    for system in ["python3", "python"] {
        commands.push((system.to_string(), python(Path::new(system))));
    }

    for (_, command) in &mut commands {
        configure_classifier_command(command);
    }
    commands
}

struct ClassifierPool {
    slots: Vec<Mutex<Option<Worker>>>,
    next: AtomicUsize,
}

static POOL: LazyLock<ClassifierPool> = LazyLock::new(|| {
    std::thread::spawn(reap_idle_workers);
    ClassifierPool {
        slots: (0..worker_count()).map(|_| Mutex::new(None)).collect(),
        next: AtomicUsize::new(0),
    }
});

fn reap_idle_workers() {
    loop {
        std::thread::sleep(IDLE_TIMEOUT / 4);
        for slot in &POOL.slots {
            if let Ok(mut worker) = slot.try_lock() {
                if worker
                    .as_ref()
                    .is_some_and(|worker| worker.last_used.elapsed() >= IDLE_TIMEOUT)
                {
                    *worker = None;
                }
            }
        }
    }
}

impl ClassifierPool {
    fn lock(slot: &Mutex<Option<Worker>>) -> std::sync::MutexGuard<'_, Option<Worker>> {
        // A panic mid-request leaves the worker in an unknown state; drop it.
        slot.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        })
    }

    /// An idle slot if one is free, otherwise round-robin.
    fn pick(&self) -> std::sync::MutexGuard<'_, Option<Worker>> {
        for slot in &self.slots {
            if let Ok(guard) = slot.try_lock() {
                return guard;
            }
        }
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.slots.len();
        Self::lock(&self.slots[index])
    }

    fn classify(&self, path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
        let mut slot = self.pick();
        let mut last_error = None;
        // A worker that crashed or desynced gets one fresh replacement.
        for _ in 0..2 {
            let exited = slot
                .as_mut()
                .is_none_or(|worker| !matches!(worker.child.try_wait(), Ok(None)));
            if exited {
                *slot = Some(Worker::spawn()?);
            }
            let worker = slot.as_mut().expect("worker present");
            match worker.classify(path, tier1_zcr) {
                Ok(result) => return result,
                Err(err) => {
                    *slot = None;
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.expect("loop ran"))
    }

    fn warm(&self) -> Result<(), ClassifyError> {
        for slot in &self.slots {
            let mut worker = Self::lock(slot);
            if worker.is_none() {
                *worker = Some(Worker::spawn()?);
            }
        }
        Ok(())
    }
}

/// Cap at two workers: each loads the ONNX models and is memory-heavy.
pub fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|count| (count.get() / 2).clamp(1, 2))
        .unwrap_or(1)
}

pub fn warm() -> Result<(), ClassifyError> {
    POOL.warm()
}

pub fn classify_tier2(path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
    POOL.classify(path, tier1_zcr)
}
