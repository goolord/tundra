use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

const UV_PYTHON: &str = "3.12";
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

const MODELS: [(&str, &str); 3] = [
    (
        "discogs-effnet-bsdynamic-1.onnx",
        "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.onnx",
    ),
    (
        "mtg_jamendo_instrument-discogs-effnet-1.onnx",
        "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.onnx",
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
        /// Skip ONNX DL Python env (`--group dl`)
        #[arg(long)]
        skip_dl: bool,
    },
    /// Download bundled ONNX models into `resources/models/`.
    Models,
    /// Install Python classifier dependencies with uv.
    Classifiers {
        /// Skip ONNX DL Python env (`--group dl`)
        #[arg(long)]
        skip_dl: bool,
    },
    /// `cargo build` (runs setup first).
    Build {
        #[arg(long, short)]
        release: bool,
        /// Rust target triple (e.g. `x86_64-unknown-linux-gnu`)
        #[arg(long)]
        target: Option<String>,
        /// Use `cross` instead of `cargo` (recommended for non-native Linux/Windows-gnu targets)
        #[arg(long)]
        cross: bool,
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
    /// Build release binary and zip a portable package (exe/binary + models + python when native).
    Package {
        /// Release tag/version used in the zip file name (e.g. v0.1.0-pre-alpha)
        #[arg(long, default_value = "v0.1.0-pre-alpha")]
        version: String,
        /// Rust target triple (defaults to host)
        #[arg(long)]
        target: Option<String>,
        /// Use `cross` instead of `cargo` for the build step
        #[arg(long)]
        cross: bool,
        /// Skip `cargo build --release`
        #[arg(long)]
        skip_build: bool,
        /// Skip bundled Python even when host matches target
        #[arg(long)]
        skip_python: bool,
    },
    /// Cross-compilation helpers (install toolchains, build all host-supported targets).
    Cross {
        #[command(subcommand)]
        command: CrossCommands,
    },
}

#[derive(Subcommand)]
enum CrossCommands {
    /// `rustup target add` for all release triples
    InstallTargets,
    /// Build `--release` for every target feasible from this host
    BuildAll {
        /// Use `cross` where the target OS differs from the host
        #[arg(long)]
        cross: bool,
        /// Skip setup step
        #[arg(long)]
        no_setup: bool,
        /// Skip Essentia DL Python env during setup
        #[arg(long)]
        skip_dl: bool,
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
            target,
            cross,
            no_setup,
            skip_dl,
        } => {
            if !no_setup {
                setup(false, skip_dl)?;
            }
            cargo_build(release, target.as_deref(), cross)
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
        Commands::Package {
            version,
            target,
            cross,
            skip_build,
            skip_python,
        } => package_release(
            &version,
            target.as_deref(),
            cross,
            skip_build,
            skip_python,
        ),
        Commands::Cross { command } => match command {
            CrossCommands::InstallTargets => install_release_targets(),
            CrossCommands::BuildAll {
                cross,
                no_setup,
                skip_dl,
            } => cross_build_all(cross, no_setup, skip_dl),
        },
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

    if skip_dl {
        run_command(
            Command::new("uv")
                .args(["sync", "--python", UV_PYTHON])
                .current_dir(&scripts),
            "uv sync (librosa tier)",
        )?;
        println!("classifiers: skipped ONNX DL env (--skip-dl)");
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

fn windows_target(target: Option<&str>) -> bool {
    target.is_some_and(|triple| triple.contains("windows")) || (target.is_none() && cfg!(windows))
}

fn apply_release_link_flags(cmd: &mut Command, target: Option<&str>) {
    if !windows_target(target) {
        return;
    }
    const FLAG: &str = "-C target-feature=+crt-static";
    let flags = match std::env::var("RUSTFLAGS") {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} {FLAG}"),
        _ => FLAG.to_string(),
    };
    cmd.env("RUSTFLAGS", flags);
}

fn host_triple() -> Option<String> {
    std::env::var("HOST").ok().or_else(|| {
        Command::new("rustc")
            .arg("-vV")
            .output()
            .ok()
            .and_then(|output| {
                String::from_utf8(output.stdout).ok().and_then(|text| {
                    text.lines()
                        .find_map(|line| line.strip_prefix("host: "))
                        .map(str::to_string)
                })
            })
    })
}

fn host_matches_target(target: &str) -> bool {
    host_triple().as_deref() == Some(target)
}

fn resolve_package_target(target: Option<&str>, skip_build: bool) -> Result<String> {
    if let Some(target) = target {
        return Ok(target.to_string());
    }
    if skip_build {
        bail!("--skip-build requires --target");
    }
    host_triple().ok_or_else(|| {
        anyhow::anyhow!("could not detect host triple; pass --target explicitly")
    })
}

fn should_use_cross_tool(target: Option<&str>, force_cross: bool) -> Result<bool> {
    let Some(target) = target else {
        return Ok(false);
    };
    if force_cross {
        return Ok(true);
    }
    let host = host_triple().unwrap_or_default();
    if host == target {
        return Ok(false);
    }
    if target.contains("darwin") && host.contains("darwin") {
        return Ok(false);
    }
    if target.contains("linux") || target.contains("windows") {
        bail!(
            "cross-compiling {target} from {host} requires `--cross` (install: cargo install cross --locked)"
        );
    }
    if target.contains("darwin") {
        bail!(
            "cross-compiling {target} from {host} requires a macOS host; build on macOS CI instead"
        );
    }
    Ok(false)
}

fn ensure_rustup_target(target: &str) -> Result<()> {
    run_command(
        Command::new("rustup")
            .args(["target", "add", target])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
        &format!("rustup target add {target}"),
    )
}

fn install_release_targets() -> Result<()> {
    for target in cross_targets_for_host() {
        ensure_rustup_target(target)?;
    }
    Ok(())
}

fn cross_targets_for_host() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["x86_64-pc-windows-msvc"]
    } else if cfg!(target_os = "macos") {
        vec!["x86_64-apple-darwin", "aarch64-apple-darwin"]
    } else {
        vec![
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-gnu",
        ]
    }
}

fn cross_build_all(cross: bool, no_setup: bool, skip_dl: bool) -> Result<()> {
    if !no_setup {
        setup(false, skip_dl)?;
    }
    install_release_targets()?;
    let mut failures = Vec::new();
    for target in cross_targets_for_host() {
        let use_cross = cross || should_use_cross_for_target(target);
        println!("cross: building {target} (cross={use_cross})");
        if let Err(err) = cargo_build(true, Some(target), use_cross) {
            eprintln!("cross: {target} failed: {err:#}");
            failures.push(target);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("failed targets: {}", failures.join(", "))
    }
}

fn should_use_cross_for_target(target: &str) -> bool {
    !host_matches_target(target)
        && (target.contains("linux") || target.contains("windows"))
        && !cfg!(windows)
}

fn cargo_build_command(release: bool, target: Option<&str>, use_cross: bool) -> Result<Command> {
    let use_cross = if use_cross {
        true
    } else {
        should_use_cross_tool(target, false)?
    };
    if use_cross {
        ensure_tool("cross")?;
    }
    if let Some(target) = target {
        ensure_rustup_target(target)?;
    }

    let mut cmd = if use_cross {
        let mut command = Command::new("cross");
        command.arg("build");
        command
    } else {
        let mut command = Command::new("cargo");
        command.arg("build");
        command
    };
    cmd.current_dir(project_root());
    if release {
        cmd.arg("--release");
        apply_release_link_flags(&mut cmd, target);
    }
    if let Some(target) = target {
        cmd.args(["--target", target]);
    }
    Ok(cmd)
}

fn cargo_build(release: bool, target: Option<&str>, cross: bool) -> Result<()> {
    let mut cmd = cargo_build_command(release, target, cross)?;
    run_command(&mut cmd, "cargo build")
}

fn cargo_run_command(release: bool) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(project_root());
    if release {
        cmd.args(["--profile", "release-fast"]);
    }
    cmd
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

fn release_dir(target: Option<&str>) -> PathBuf {
    let root = project_root();
    if let Some(target) = target {
        return root.join("target").join(target).join("release");
    }
    let profile = root.join("target").join("release");
    if profile.join("tundra.exe").is_file() || profile.join("tundra").is_file() {
        return profile;
    }
    if let Ok(triple) = std::env::var("TARGET") {
        let triple_dir = root.join("target").join(triple).join("release");
        if triple_dir.join("tundra.exe").is_file() || triple_dir.join("tundra").is_file() {
            return triple_dir;
        }
    }
    profile
}

fn release_binary_name(target: Option<&str>) -> &'static str {
    if windows_target(target) {
        "tundra.exe"
    } else {
        "tundra"
    }
}

fn classifier_python_for_target(target: Option<&str>) -> &'static str {
    if windows_target(target) {
        "3.12"
    } else {
        UV_PYTHON
    }
}

fn package_archive_name(version: &str, target: &str) -> String {
    if target.contains("windows") {
        format!("tundra-{version}-{target}.zip")
    } else {
        format!("tundra-{version}-{target}.tar.gz")
    }
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .with_context(|| format!("copy {} -> {}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn copy_glob(src_dir: &std::path::Path, pattern: &str, dst_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst_dir)?;
    for entry in std::fs::read_dir(src_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(pattern.trim_start_matches('*')) {
            continue;
        }
        std::fs::copy(entry.path(), dst_dir.join(name))?;
    }
    Ok(())
}

fn find_bundled_python(python_root: &std::path::Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(python_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        #[cfg(windows)]
        let candidate = entry.path().join("python.exe");
        #[cfg(not(windows))]
        let candidate = entry.path().join("bin").join("python3");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no python executable found under {}",
        python_root.display()
    );
}

fn package_release(
    version: &str,
    target: Option<&str>,
    cross: bool,
    skip_build: bool,
    skip_python: bool,
) -> Result<()> {
    let package_target = resolve_package_target(target, skip_build)?;
    let target_ref = package_target.as_str();

    if !skip_build {
        cargo_build(true, Some(target_ref), cross)?;
    }

    let root = project_root();
    let release = release_dir(Some(target_ref));
    let bin_name = release_binary_name(Some(target_ref));
    let exe = release.join(bin_name);
    if !exe.is_file() {
        bail!("missing release binary at {}", exe.display());
    }

    let bundle_python = !skip_python && host_matches_target(target_ref);
    if !skip_python && !host_matches_target(target_ref) {
        eprintln!(
            "package: skipping bundled Python (host triple != {target_ref}); ship scripts/ + models/ only"
        );
    }

    let staging = root.join("target").join("release-package");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clean {}", staging.display()))?;
    }
    std::fs::create_dir_all(&staging)?;

    std::fs::copy(&exe, staging.join(bin_name))
        .with_context(|| format!("copy {}", exe.display()))?;

    let models_src = release.join("models");
    if models_src.is_dir() {
        copy_dir_all(&models_src, &staging.join("models"))?;
    } else {
        bail!("missing bundled models at {}", models_src.display());
    }

    let scripts_src = root.join("scripts");
    let scripts_dst = staging.join("scripts");
    std::fs::create_dir_all(&scripts_dst)?;
    copy_glob(&scripts_src, "*.py", &scripts_dst)?;
    for name in ["pyproject.toml", "uv.lock", ".python-version"] {
        let src = scripts_src.join(name);
        if src.is_file() {
            std::fs::copy(&src, scripts_dst.join(name))?;
        }
    }

    if bundle_python {
        ensure_tool("uv")?;
        let python_version = classifier_python_for_target(Some(target_ref));
        let python_root = staging.join("python");
        std::fs::create_dir_all(&python_root)?;

        let mut python_install = Command::new("uv");
        python_install
            .args(["python", "install", python_version])
            .env("UV_PYTHON_INSTALL_DIR", &python_root);
        run_command(
            &mut python_install,
            &format!("uv python install {python_version}"),
        )?;

        let python_exe = find_bundled_python(&python_root)?;
        let venv_dir = scripts_dst.join(".venv");
        run_command(
            Command::new("uv")
                .args(["venv", "--python"])
                .arg(&python_exe)
                .arg(&venv_dir)
                .current_dir(&scripts_dst),
            "uv venv",
        )?;
        run_command(
            Command::new("uv")
                .arg("sync")
                .current_dir(&scripts_dst),
            "uv sync",
        )?;
    }

    let archive_name = package_archive_name(version, target_ref);
    let archive_path = root.join("target").join(&archive_name);
    if archive_path.is_file() {
        std::fs::remove_file(&archive_path)?;
    }

    if archive_name.ends_with(".zip") {
        #[cfg(windows)]
        {
            run_command(
                Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-Command",
                        &format!(
                            "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                            staging.join("*").display(),
                            archive_path.display()
                        ),
                    ]),
                "Compress-Archive",
            )?;
        }
        #[cfg(not(windows))]
        {
            run_command(
                Command::new("zip")
                    .arg("-r")
                    .arg(&archive_path)
                    .arg(".")
                    .current_dir(&staging),
                "zip",
            )?;
        }
    } else {
        run_command(
            Command::new("tar")
                .args(["-czf"])
                .arg(&archive_path)
                .arg("-C")
                .arg(&staging)
                .arg("."),
            "tar",
        )?;
    }

    println!("package: {}", archive_path.display());
    Ok(())
}
