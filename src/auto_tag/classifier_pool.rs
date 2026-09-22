//! Long-lived Python tier-2 workers speaking JSON lines over stdio.
//!
//! Every request carries an id that the reply must echo. Anything else on the
//! protocol stream (a stray print, a reply to an earlier request) retires the
//! worker, so one file's label can never be attributed to another file.

use super::ClassifyError;
use crate::locks::lock;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// For the start-up greeting (model load) and for each reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);
/// Each worker holds the model in memory; release it when unused.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// A recent start-up failure is reused this long, so a broken Python setup
/// fails each file at once instead of timing out on every grey-zone file.
const SPAWN_RETRY_AFTER: Duration = Duration::from_secs(60);
const INSTALL_HINT: &str =
    "Install classifiers with `cargo xtask setup`, or use a release package that includes python/";

#[derive(Deserialize)]
struct WorkerReady {
    ready: bool,
    error: Option<String>,
    #[serde(default)]
    onnx: bool,
}

#[derive(Deserialize)]
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

fn failed(details: impl Into<String>) -> ClassifyError {
    ClassifyError::analysis_failed(details)
}

fn parse<T: DeserializeOwned>(line: &str, what: &str) -> Result<T, ClassifyError> {
    serde_json::from_str(line).map_err(|err| failed(format!("{what} {line:?}: {err}")))
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    last_used: Instant,
}

impl Worker {
    /// The first launch command that starts and reports ready.
    fn spawn() -> Result<Self, ClassifyError> {
        let mut attempts = Vec::new();
        for (label, command) in launch_commands() {
            match Self::start(command) {
                Ok(worker) => return Ok(worker),
                Err(err) => attempts.push(format!("{label}: {}", err.details)),
            }
        }
        Err(ClassifyError::new("Couldn't start classifier worker.", format!("{INSTALL_HINT}. {}", attempts.join("; "))))
    }

    fn start(mut command: Command) -> Result<Self, ClassifyError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| failed(format!("Failed to spawn worker: {err}")))?;
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
            BufReader::new(stdout).lines().map_while(Result::ok).try_for_each(|line| line_tx.send(line))
        });
        // Built before the greeting so a failed start still kills the child.
        let worker = Self { stdin: child.stdin.take().expect("stdin piped"), child, lines, last_used: Instant::now() };
        let ready: WorkerReady = parse(&worker.next_line()?, "Unexpected worker greeting")?;
        if !ready.ready {
            return Err(failed(ready.error.unwrap_or_else(|| "unknown worker error".into())));
        }
        if !ready.onnx {
            eprintln!("classifier worker: YAMNet unavailable; grey-zone files use the spectral fallback");
        }
        Ok(worker)
    }

    fn next_line(&self) -> Result<String, ClassifyError> {
        self.lines.recv_timeout(REPLY_TIMEOUT).map_err(|err| {
            failed(match err {
                RecvTimeoutError::Timeout => "Classifier worker timed out",
                RecvTimeoutError::Disconnected => "Classifier worker exited",
            })
        })
    }

    /// The outer `Err` means the worker can no longer be trusted and must be
    /// replaced; an inner `Err` is an ordinary per-file failure.
    fn classify(&mut self, path: &Path, tier1_zcr: f64) -> Result<Result<Tier2Response, ClassifyError>, ClassifyError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.last_used = Instant::now();

        let path = crate::path_util::normalize_path(path.to_path_buf());
        let request = serde_json::json!({ "id": id, "path": path.to_string_lossy(), "tier1_zcr": tier1_zcr });
        writeln!(self.stdin, "{request}")
            .and_then(|()| self.stdin.flush())
            .map_err(|err| failed(format!("Failed to write to classifier worker: {err}")))?;

        let response: WorkerResponse = parse(&self.next_line()?, "Invalid worker output")?;
        if response.id != Some(id) {
            return Err(failed(format!("Worker replied to request {:?} while {id} was pending", response.id)));
        }
        self.last_used = Instant::now();
        Ok(match (response.ok, response.result) {
            (true, Some(result)) => Ok(result),
            (true, None) => Err(failed("Worker success response missing result")),
            (false, _) => Err(failed(response.error.unwrap_or_else(|| "Unknown worker error".into()))),
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

/// A Python that ships with Tundra, if any: the standalone interpreter plus
/// the `site-packages` to put on PYTHONPATH from a release package, or the
/// `scripts/.venv` that `cargo xtask setup` creates for development.
fn bundled_python() -> Option<(PathBuf, Option<PathBuf>)> {
    #[cfg(windows)]
    const VENV_PYTHON: &str = "scripts/.venv/Scripts/python.exe";
    #[cfg(not(windows))]
    const VENV_PYTHON: &str = "scripts/.venv/bin/python3";
    crate::platform::find_beside(&["python"], |dir| dir.join("site-packages").is_dir())
        .and_then(|python| {
            let exe = std::fs::read_dir(&python)
                .ok()?
                .flatten()
                .flat_map(|entry| {
                    let dir = entry.path();
                    [dir.join("python.exe"), dir.join("bin").join("python3")]
                })
                .find(|candidate| candidate.is_file())?;
            Some((exe, Some(python.join("site-packages"))))
        })
        .or_else(|| {
            crate::platform::find_beside(&[VENV_PYTHON], |candidate| candidate.is_file()).map(|exe| (exe, None))
        })
}

/// Interpreters to try, in order: the bundled Python, `uv run` with the
/// version pinned by `scripts/.python-version`, then a system Python on Unix.
fn launch_commands() -> Vec<(String, Command)> {
    let scripts_dir = crate::platform::find_beside(&["scripts"], |dir| dir.join("classifier_worker.py").is_file())
        .unwrap_or_else(|| PathBuf::from("scripts"));
    let script = scripts_dir.join("classifier_worker.py");
    let python = |program: &Path| {
        let mut command = Command::new(program);
        command.arg(&script).current_dir(&scripts_dir);
        command
    };
    let mut commands = Vec::new();
    if let Some((exe, site_packages)) = bundled_python() {
        let mut command = python(&exe);
        if let Some(site_packages) = site_packages {
            command.env("PYTHONPATH", site_packages);
        }
        commands.push((exe.display().to_string(), command));
    }
    let mut uv = Command::new("uv");
    let pinned = include_str!("../../scripts/.python-version").trim();
    uv.current_dir(&scripts_dir).args(["run", "--python", pinned, "classifier_worker.py"]);
    commands.push(("uv run".to_string(), uv));
    #[cfg(not(windows))]
    commands.extend(["python3", "python"].map(|system| (system.to_string(), python(Path::new(system)))));

    // The YAMNet model and its class map (see `tools/yamnet`).
    let models = crate::platform::find_beside(&["models", "resources/models"], |dir| {
        dir.join("yamnet.onnx").is_file() && dir.join("yamnet_class_map.csv").is_file()
    });
    for (_, command) in &mut commands {
        // One BLAS/OpenMP thread per worker keeps bulk runs polite; no GPU.
        for key in [
            "OMP_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "MKL_NUM_THREADS",
            "VECLIB_MAXIMUM_THREADS",
            "NUMEXPR_NUM_THREADS",
        ] {
            command.env(key, "1");
        }
        command.env("CUDA_VISIBLE_DEVICES", "-1");
        // Paths cross the pipe as UTF-8 whatever the system code page is.
        command.env("PYTHONUTF8", "1").env("PYTHONIOENCODING", "utf-8");
        if let Some(models) = &models {
            command.env("TUNDRA_MODELS", models).env("TUNDRA_ONNX_DL", "1");
        }
        crate::platform::hide_console(command);
    }
    commands
}

struct ClassifierPool {
    slots: Vec<Mutex<Option<Worker>>>,
    next: AtomicUsize,
    last_spawn_failure: Mutex<Option<(Instant, ClassifyError)>>,
}

static POOL: LazyLock<ClassifierPool> = LazyLock::new(|| {
    std::thread::spawn(reap_idle_workers);
    // At most two workers: each loads the model and onnxruntime.
    let workers = std::thread::available_parallelism().map_or(1, |count| (count.get() / 2).clamp(1, 2));
    ClassifierPool {
        slots: (0..workers).map(|_| Mutex::new(None)).collect(),
        next: AtomicUsize::new(0),
        last_spawn_failure: Mutex::new(None),
    }
});

fn reap_idle_workers() {
    loop {
        std::thread::sleep(IDLE_TIMEOUT / 4);
        for slot in &POOL.slots {
            if let Ok(mut worker) = slot.try_lock()
                && worker.as_ref().is_some_and(|worker| worker.last_used.elapsed() >= IDLE_TIMEOUT)
            {
                *worker = None;
            }
        }
    }
}

impl ClassifierPool {
    fn lock(slot: &Mutex<Option<Worker>>) -> MutexGuard<'_, Option<Worker>> {
        // A panic mid-request leaves the worker in an unknown state; drop it.
        slot.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = None;
            guard
        })
    }

    fn spawn(&self) -> Result<Worker, ClassifyError> {
        if let Some((at, err)) = &*lock(&self.last_spawn_failure)
            && at.elapsed() < SPAWN_RETRY_AFTER
        {
            return Err(err.clone());
        }
        Worker::spawn().inspect_err(|err| *lock(&self.last_spawn_failure) = Some((Instant::now(), err.clone())))
    }

    /// An idle slot if one is free, otherwise round-robin.
    fn pick(&self) -> MutexGuard<'_, Option<Worker>> {
        self.slots.iter().find_map(|slot| slot.try_lock().ok()).unwrap_or_else(|| {
            let index = self.next.fetch_add(1, Ordering::Relaxed) % self.slots.len();
            Self::lock(&self.slots[index])
        })
    }

    fn classify(&self, path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
        let mut slot = self.pick();
        let mut last_error = None;
        // A worker that crashed or desynced gets one fresh replacement.
        for _ in 0..2 {
            let exited = slot.as_mut().is_none_or(|worker| !matches!(worker.child.try_wait(), Ok(None)));
            if exited {
                *slot = Some(self.spawn()?);
            }
            match slot.as_mut().expect("worker present").classify(path, tier1_zcr) {
                Ok(result) => return result,
                Err(err) => {
                    *slot = None;
                    last_error = Some(err);
                }
            }
        }
        Err(last_error.expect("loop ran"))
    }
}

pub fn warm() -> Result<(), ClassifyError> {
    for slot in &POOL.slots {
        let mut worker = ClassifierPool::lock(slot);
        if worker.is_none() {
            *worker = Some(POOL.spawn()?);
        }
    }
    Ok(())
}

pub fn classify_tier2(path: &Path, tier1_zcr: f64) -> Result<Tier2Response, ClassifyError> {
    POOL.classify(path, tier1_zcr)
}
