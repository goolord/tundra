use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// Classifier Python, pinned for every platform. Keep in sync with
/// `scripts/.python-version` and `auto_tag::UV_PYTHON`.
const PYTHON_VERSION: &str = "3.12";
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Scripts the app runs; everything else under `scripts/` is development-only.
const RUNTIME_SCRIPTS: [&str; 2] = ["classifier_worker.py", "tier2_lib.py"];
const PACKAGE_DOCS: [&str; 3] = ["LICENSE", "EULA.md", "README.md"];

struct Model {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const MODELS: [Model; 3] = [
    Model {
        name: "discogs-effnet-bsdynamic-1.onnx",
        url: "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.onnx",
        sha256: "a280825b334797cf677939db8cd5762c0392aedd0ca6415dbc1cd083f045e43c",
    },
    Model {
        name: "mtg_jamendo_instrument-discogs-effnet-1.onnx",
        url: "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.onnx",
        sha256: "9ae2d9e763d66bd8eed654d1ac3aa171e6539cb8a0e11f3dcd53df1428980802",
    },
    Model {
        name: "mtg_jamendo_instrument-discogs-effnet-1.json",
        url: "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_instrument/mtg_jamendo_instrument-discogs-effnet-1.json",
        sha256: "7d02204c6451b5615e2968ec6364bbae3b915c886e608f05f00d3a38dc5177c4",
    },
];

#[derive(Parser)]
#[command(name = "xtask", about = "Build and release tasks for Tundra")]
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
        /// Skip the ONNX runtime (`--group dl`); tier 2 falls back to librosa
        #[arg(long)]
        skip_dl: bool,
    },
    /// Download and verify the bundled ONNX models in `resources/models/`.
    Models,
    /// Install Python classifier dependencies with uv.
    Classifiers {
        /// Skip the ONNX runtime (`--group dl`)
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
        /// Use `cross` instead of `cargo`
        #[arg(long)]
        cross: bool,
        /// Skip setup step
        #[arg(long)]
        no_setup: bool,
        /// Skip the ONNX runtime during setup
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
        /// Skip the ONNX runtime during setup
        #[arg(long)]
        skip_dl: bool,
        /// Audio paths to open (pass after `--`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Release build plus a portable archive: binary, models, scripts, and
    /// (when building for this host) a bundled Python.
    Package {
        /// Version used in the archive name; defaults to `v` + Cargo.toml version
        #[arg(long)]
        version: Option<String>,
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
    /// Package this host's build and attach it to a draft GitHub release for
    /// `v<Cargo.toml version>`. Published tags are never moved.
    Release {
        /// Also dispatch the CI workflow to build the other platforms
        #[arg(long)]
        ci: bool,
        /// Skip packaging and upload existing archives from `target/`
        #[arg(long)]
        skip_build: bool,
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
        /// Skip the ONNX runtime during setup
        #[arg(long)]
        skip_dl: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
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
        } => {
            let version = version.unwrap_or_else(release_tag);
            package_release(&version, target.as_deref(), cross, skip_build, skip_python).map(drop)
        }
        Commands::Release { ci, skip_build } => release(ci, skip_build),
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

/// `v` + the app's version from the root Cargo.toml.
fn release_tag() -> String {
    let manifest = std::fs::read_to_string(project_root().join("Cargo.toml")).unwrap_or_default();
    let version = manifest
        .split("[package]")
        .nth(1)
        .and_then(|package| {
            package.lines().find_map(|line| {
                let value = line.trim().strip_prefix("version")?.trim().strip_prefix('=')?;
                Some(value.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "0.0.0".into());
    format!("v{version}")
}

fn setup(skip_lfs: bool, skip_dl: bool) -> Result<()> {
    if !skip_lfs {
        git_lfs_pull()?;
    }
    download_models()?;
    setup_classifiers(skip_dl)
}

fn git_lfs_pull() -> Result<()> {
    let root = project_root();
    if !root.join(".git").exists() {
        return Ok(());
    }
    let available = Command::new("git")
        .args(["lfs", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        eprintln!("warning: git-lfs not installed; SVG resources and models may be missing");
        return Ok(());
    }
    run(Command::new("git").args(["lfs", "pull"]).current_dir(&root))
}

fn models_dir() -> PathBuf {
    project_root().join("resources/models")
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).with_context(|| format!("read {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Download any model that is missing, truncated, an LFS pointer, or otherwise
/// does not match its pinned hash. Downloads land in a `.part` file and are
/// only renamed into place once verified.
fn download_models() -> Result<()> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for model in &MODELS {
        let dest = dir.join(model.name);
        if dest.is_file() && sha256_file(&dest)? == model.sha256 {
            println!("models: {} verified", model.name);
            continue;
        }
        println!("models: downloading {}", model.name);
        let part = dir.join(format!("{}.part", model.name));
        let response = ureq::get(model.url)
            .timeout(MODEL_DOWNLOAD_TIMEOUT)
            .call()
            .with_context(|| format!("GET {}", model.url))?;
        let mut file = std::fs::File::create(&part)?;
        std::io::copy(&mut response.into_reader(), &mut file)
            .with_context(|| format!("write {}", part.display()))?;
        drop(file);
        let actual = sha256_file(&part)?;
        if actual != model.sha256 {
            let _ = std::fs::remove_file(&part);
            bail!(
                "{} has sha256 {actual}, expected {}; refusing to use it",
                model.name,
                model.sha256
            );
        }
        std::fs::rename(&part, &dest)?;
    }
    Ok(())
}

fn verify_models() -> Result<()> {
    for model in &MODELS {
        let path = models_dir().join(model.name);
        if !path.is_file() || sha256_file(&path)? != model.sha256 {
            bail!(
                "{} is missing or does not match its pinned hash; run `cargo xtask models`",
                path.display()
            );
        }
    }
    Ok(())
}

fn setup_classifiers(skip_dl: bool) -> Result<()> {
    let scripts = project_root().join("scripts");
    if !scripts.join("pyproject.toml").is_file() {
        bail!("missing scripts/pyproject.toml");
    }
    run(Command::new("uv")
        .args(["python", "install", PYTHON_VERSION])
        .current_dir(&scripts))?;
    let mut sync = Command::new("uv");
    sync.args(["sync", "--locked", "--python", PYTHON_VERSION])
        .current_dir(&scripts);
    if skip_dl {
        println!("classifiers: skipping the ONNX runtime (--skip-dl)");
    } else {
        sync.args(["--group", "dl"]);
    }
    run(&mut sync)
}

fn host_triple() -> Result<&'static str> {
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    HOST.get_or_init(|| {
        let output = Command::new("rustc").arg("-vV").output().ok()?;
        String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .map(str::to_string)
    })
    .as_deref()
    .context("could not detect the host triple; pass --target explicitly")
}

fn is_host(target: &str) -> bool {
    host_triple().is_ok_and(|host| host == target)
}

/// Whether building `target` from this host needs the `cross` tool.
fn needs_cross(target: &str) -> Result<bool> {
    let host = host_triple()?;
    if host == target || (target.contains("darwin") && host.contains("darwin")) {
        return Ok(false);
    }
    if target.contains("darwin") {
        bail!("{target} can only be built on a macOS host");
    }
    Ok(true)
}

fn install_release_targets() -> Result<()> {
    for target in cross_targets_for_host() {
        run(Command::new("rustup").args(["target", "add", target]))?;
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
        println!("cross: building {target}");
        if let Err(err) = cargo_build(true, Some(target), cross) {
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

fn cargo_build(release: bool, target: Option<&str>, force_cross: bool) -> Result<()> {
    let use_cross = match target {
        Some(target) => force_cross || needs_cross(target)?,
        None => false,
    };
    let mut cmd = Command::new(if use_cross { "cross" } else { "cargo" });
    cmd.args(["build", "--locked"]).current_dir(project_root());
    if release {
        cmd.arg("--release");
    }
    if let Some(target) = target {
        run(Command::new("rustup").args(["target", "add", target]))?;
        cmd.args(["--target", target]);
    }
    run(&mut cmd)
}

fn cargo_run(release: bool, extra_args: &[String]) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(project_root());
    if release {
        cmd.args(["--profile", "release-fast"]);
    }
    if !extra_args.is_empty() {
        cmd.arg("--").args(extra_args);
    }
    run(&mut cmd)
}

fn run(command: &mut Command) -> Result<()> {
    let label = format!(
        "{} {}",
        command.get_program().to_string_lossy(),
        command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let status = command.stdin(Stdio::inherit()).status().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("`{label}`: required tool is not installed or not on PATH")
        } else {
            anyhow::anyhow!("`{label}`: failed to start: {err}")
        }
    })?;
    if !status.success() {
        bail!("`{label}` failed with {status}");
    }
    Ok(())
}

fn output(command: &mut Command) -> Result<String> {
    let result = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("run {}", command.get_program().to_string_lossy()))?;
    if !result.status.success() {
        bail!(
            "{} failed with {}",
            command.get_program().to_string_lossy(),
            result.status
        );
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst)
        .with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

fn find_bundled_python(python_root: &Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(python_root)? {
        let dir = entry?.path();
        for candidate in [dir.join("python.exe"), dir.join("bin").join("python3")] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!("no python executable found under {}", python_root.display())
}

/// Install a standalone CPython plus the locked classifier dependencies into
/// `python/`. Packages go into `python/site-packages` (found via PYTHONPATH at
/// runtime) instead of a virtualenv, whose absolute interpreter path would
/// break as soon as the archive is unpacked anywhere else.
fn bundle_python(staging: &Path) -> Result<()> {
    let python_root = staging.join("python");
    std::fs::create_dir_all(&python_root)?;
    run(Command::new("uv")
        .args(["python", "install", PYTHON_VERSION])
        .env("UV_PYTHON_INSTALL_DIR", &python_root))?;
    let python = find_bundled_python(&python_root)?;

    let requirements = staging.join("requirements.txt");
    run(Command::new("uv")
        .args([
            "export",
            "--locked",
            "--group",
            "dl",
            "--no-hashes",
            "--no-emit-project",
            "--format",
            "requirements-txt",
            "--python",
            PYTHON_VERSION,
            "--output-file",
        ])
        .arg(&requirements)
        .current_dir(project_root().join("scripts")))?;
    run(Command::new("uv")
        .args(["pip", "install", "--python"])
        .arg(&python)
        .arg("--target")
        .arg(python_root.join("site-packages"))
        .arg("-r")
        .arg(&requirements))?;
    std::fs::remove_file(&requirements)?;
    Ok(())
}

/// Builds `target/package/tundra-<version>-<target>/` and archives it with that
/// folder at the top, plus a `.sha256` file. Returns the archive paths.
fn package_release(
    version: &str,
    target: Option<&str>,
    cross: bool,
    skip_build: bool,
    skip_python: bool,
) -> Result<Vec<PathBuf>> {
    let target = match target {
        Some(target) => target.to_string(),
        None if skip_build => bail!("--skip-build requires --target"),
        None => host_triple()?.to_string(),
    };
    verify_models()?;
    if !skip_build {
        cargo_build(true, Some(&target), cross)?;
    }

    let root = project_root();
    let windows = target.contains("windows");
    let bin_name = if windows { "tundra.exe" } else { "tundra" };
    let exe = root.join("target").join(&target).join("release").join(bin_name);
    if !exe.is_file() {
        bail!("missing release binary at {}", exe.display());
    }

    let name = format!("tundra-{version}-{target}");
    let package_dir = root.join("target").join("package");
    let staging = package_dir.join(&name);
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("clean {}", staging.display()))?;
    }
    std::fs::create_dir_all(staging.join("models"))?;
    std::fs::create_dir_all(staging.join("scripts"))?;

    copy_file(&exe, &staging.join(bin_name))?;
    for model in &MODELS {
        copy_file(&models_dir().join(model.name), &staging.join("models").join(model.name))?;
    }
    for script in RUNTIME_SCRIPTS {
        copy_file(&root.join("scripts").join(script), &staging.join("scripts").join(script))?;
    }
    for doc in PACKAGE_DOCS {
        copy_file(&root.join(doc), &staging.join(doc))?;
    }

    if skip_python {
        println!("package: skipping bundled Python (--skip-python)");
    } else if is_host(&target) {
        bundle_python(&staging)?;
    } else {
        eprintln!("package: not bundling Python for {target} (only the host's Python can be bundled)");
    }

    let archive = package_dir.join(if windows {
        format!("{name}.zip")
    } else {
        format!("{name}.tar.gz")
    });
    if archive.is_file() {
        std::fs::remove_file(&archive)?;
    }
    // bsdtar (bundled with Windows 10+ and macOS) picks the format from the
    // extension with -a; GNU tar handles .tar.gz with -z.
    let mut tar = Command::new("tar");
    if windows {
        tar.arg("-a");
    } else {
        tar.arg("-z");
    }
    run(tar
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&package_dir)
        .arg(&name))?;

    let checksum = PathBuf::from(format!("{}.sha256", archive.display()));
    let file_name = archive.file_name().expect("archive name").to_string_lossy();
    std::fs::write(&checksum, format!("{}  {file_name}\n", sha256_file(&archive)?))?;
    println!("package: {}", archive.display());
    Ok(vec![archive, checksum])
}

/// Attach this host's package to a draft release for the Cargo.toml version.
///
/// Refuses a dirty tree, an unpushed HEAD, or a tag that already points
/// somewhere else. The release stays a draft until published by hand, so a
/// failed CI job never leaves a half-populated public release.
fn release(ci: bool, skip_build: bool) -> Result<()> {
    let root = project_root();
    let tag = release_tag();
    let git = |args: &[&str]| output(Command::new("git").args(args).current_dir(&root));

    if !git(&["status", "--porcelain"])?.is_empty() {
        bail!("working tree has uncommitted changes");
    }
    let head = git(&["rev-parse", "HEAD"])?;
    run(Command::new("git").args(["fetch", "--tags", "origin"]).current_dir(&root))?;
    if git(&["branch", "-r", "--contains", &head])?.is_empty() {
        bail!("HEAD {head} is not on any remote branch; push it first");
    }
    match git(&["rev-parse", &format!("refs/tags/{tag}^{{commit}}")]) {
        Ok(tagged) if tagged != head => {
            bail!("{tag} already points at {tagged}; bump the version in Cargo.toml instead of moving it")
        }
        Ok(_) => {}
        Err(_) => {
            run(Command::new("git").args(["tag", "-a", &tag, "-m", &tag]).current_dir(&root))?;
            run(Command::new("git").args(["push", "origin", &tag]).current_dir(&root))?;
        }
    }

    let draft = output(
        Command::new("gh")
            .args(["release", "view", &tag, "--json", "isDraft", "--jq", ".isDraft"])
            .current_dir(&root),
    );
    match draft.as_deref() {
        Ok("true") => {}
        Ok(_) => bail!("release {tag} is already published; its assets are left untouched"),
        Err(_) => {
        run(Command::new("gh")
            .args(["release", "create", &tag, "--draft", "--verify-tag", "--generate-notes"])
            .current_dir(&root))?;
        }
    }

    let assets = if skip_build {
        let target = host_triple()?;
        let dir = root.join("target").join("package");
        std::fs::read_dir(&dir)
            .with_context(|| format!("read {}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!("tundra-{tag}-{target}.")))
            })
            .collect()
    } else {
        package_release(&tag, None, false, false, false)?
    };
    if assets.is_empty() {
        bail!("no packages for {tag} under target/package");
    }
    // Uploading to a draft may replace this host's own earlier upload, never a
    // published asset.
    run(Command::new("gh")
        .args(["release", "upload", &tag, "--clobber"])
        .args(&assets)
        .current_dir(&root))?;

    if ci {
        run(Command::new("gh")
            .args(["workflow", "run", "release.yml", "--ref", &tag, "-f"])
            .arg(format!("tag={tag}"))
            .current_dir(&root))?;
    }
    println!("release: draft {tag} updated; publish it on GitHub once every platform is attached");
    Ok(())
}
