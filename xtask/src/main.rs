use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

const UV_PYTHON: &str = if cfg!(windows) { "3.12" } else { "3.14" };
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

const MODELS: [(&str, &str); 3] = [
    (
        "discogs-effnet-bs64-1.pb",
        "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bs64-1.pb",
    ),
    (
        "mtg_jamendo_instrument-discogs-effnet-1.pb",
        "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.pb",
    ),
    (
        "mtg_jamendo_instrument-discogs-effnet-1.json",
        "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json",
    ),
];

#[derive(Parser)]
#[command(name = "xtask", about = "Build and setup tasks for Tundra")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Full dev setup: LFS assets, classifier models, and Python envs.
    Setup {
        /// Skip `git lfs pull`
        #[arg(long)]
        skip_lfs: bool,
        /// Skip Essentia DL Python env (Python 3.14 + `--group dl`)
        #[arg(long)]
        skip_dl: bool,
    },
    /// Download bundled Essentia models into `resources/models/`.
    Models,
    /// Install Python classifier dependencies with uv.
    Classifiers {
        /// Skip Essentia DL Python env (Python 3.14 + `--group dl`)
        #[arg(long)]
        skip_dl: bool,
    },
    /// `cargo build` (runs setup first).
    Build {
        #[arg(long, short)]
        release: bool,
        /// Skip setup step
        #[arg(long)]
        no_setup: bool,
        /// Skip Essentia DL Python env during setup
        #[arg(long)]
        skip_dl: bool,
    },
    /// `cargo run` (runs setup first).
    Run {
        #[arg(long, short)]
        release: bool,
        /// Skip setup step
        #[arg(long)]
        no_setup: bool,
        /// Skip Essentia DL Python env during setup
        #[arg(long)]
        skip_dl: bool,
        /// Audio paths to open (pass after `--`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Setup { skip_lfs, skip_dl } => setup(skip_lfs, skip_dl),
        Commands::Models => download_models(),
        Commands::Classifiers { skip_dl } => setup_classifiers(skip_dl),
        Commands::Build {
            release,
            no_setup,
            skip_dl,
        } => {
            if !no_setup {
                setup(false, skip_dl)?;
            }
            cargo_build(release)
        }
        Commands::Run {
            release,
            no_setup,
            skip_dl,
            args,
        } => {
            if !no_setup {
                setup(false, skip_dl)?;
            }
            cargo_run(release, &args)
        }
    }
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate should live in project root")
        .to_path_buf()
}

fn setup(skip_lfs: bool, skip_dl: bool) -> Result<()> {
    if !skip_lfs {
        git_lfs_pull()?;
    }
    download_models()?;
    setup_classifiers(skip_dl)?;
    Ok(())
}

fn git_lfs_pull() -> Result<()> {
    let root = project_root();
    if !root.join(".git").exists() {
        return Ok(());
    }

    let check = Command::new("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match check {
        Ok(status) if status.success() => {
            run_command(
                Command::new("git")
                    .arg("lfs")
                    .arg("pull")
                    .current_dir(&root),
                "git lfs pull",
            )?;
        }
        _ => eprintln!("warning: git-lfs not installed; SVG resources may be missing"),
    }
    Ok(())
}

fn models_dir() -> PathBuf {
    project_root().join("resources/models")
}

fn download_models() -> Result<()> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    for (name, url) in MODELS {
        let dest = dir.join(name);
        if dest.is_file() {
            println!("models: {name} already present");
            continue;
        }
        println!("models: downloading {name}");
        download_file(url, &dest, &format!("download {name}"))?;
    }
    Ok(())
}

fn download_file(url: &str, dest: &std::path::Path, label: &str) -> Result<()> {
    let response = ureq::get(url)
        .timeout(MODEL_DOWNLOAD_TIMEOUT)
        .call()
        .with_context(|| format!("{label}: GET {url}"))?;
    let mut file = std::fs::File::create(dest)
        .with_context(|| format!("{label}: create {}", dest.display()))?;
    std::io::copy(&mut response.into_reader(), &mut file)
        .with_context(|| format!("{label}: write {}", dest.display()))?;
    Ok(())
}

fn setup_classifiers(skip_dl: bool) -> Result<()> {
    let scripts = project_root().join("scripts");
    if !scripts.join("pyproject.toml").is_file() {
        bail!("missing scripts/pyproject.toml");
    }

    ensure_tool("uv")?;

    run_command(
        Command::new("uv")
            .args(["python", "install", UV_PYTHON])
            .current_dir(&scripts),
        &format!("uv python install {UV_PYTHON}"),
    )?;

    if skip_dl || cfg!(windows) {
        run_command(
            Command::new("uv")
                .args(["sync", "--python", UV_PYTHON])
                .current_dir(&scripts),
            "uv sync (librosa tier)",
        )?;
        if cfg!(windows) {
            println!("classifiers: skipped Essentia DL env (TensorFlow tier unavailable on Windows)");
        } else {
            println!("classifiers: skipped Essentia DL env (--skip-dl)");
        }
        return Ok(());
    }

    run_command(
        Command::new("uv")
            .args(["sync", "--group", "dl", "--python", UV_PYTHON])
            .current_dir(&scripts),
        &format!("uv sync --group dl --python {UV_PYTHON}"),
    )?;

    Ok(())
}

fn apply_release_link_flags(cmd: &mut Command) {
    if cfg!(windows) {
        const FLAG: &str = "-C target-feature=+crt-static";
        let flags = match std::env::var("RUSTFLAGS") {
            Ok(existing) if !existing.trim().is_empty() => format!("{existing} {FLAG}"),
            _ => FLAG.to_string(),
        };
        cmd.env("RUSTFLAGS", flags);
    }
}

fn cargo_build_command(release: bool) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("build").current_dir(project_root());
    if release {
        cmd.arg("--release");
        apply_release_link_flags(&mut cmd);
    }
    cmd
}

fn cargo_run_command(release: bool) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(project_root());
    if release {
        cmd.args(["--profile", "release-fast"]);
    }
    cmd
}

fn cargo_build(release: bool) -> Result<()> {
    run_command(&mut cargo_build_command(release), "cargo build")
}

fn cargo_run(release: bool, extra_args: &[String]) -> Result<()> {
    let mut cmd = cargo_run_command(release);
    if !extra_args.is_empty() {
        cmd.arg("--");
        cmd.args(extra_args);
    }
    run_command(&mut cmd, "cargo run")
}

fn ensure_tool(name: &str) -> Result<()> {
    let status = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to execute {name}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("required tool `{name}` is not available on PATH")
    }
}

fn run_command(command: &mut Command, label: &str) -> Result<()> {
    command.stdin(Stdio::inherit());
    let status = command
        .status()
        .with_context(|| format!("failed to spawn {label}"))?;
    check_status(status, label)
}

fn check_status(status: ExitStatus, label: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("{label} failed with {status}");
    }
}
