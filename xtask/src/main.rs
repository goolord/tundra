use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Classifier Python, pinned for every platform by `scripts/.python-version`.
fn python_version() -> &'static str {
    include_str!("../../scripts/.python-version").trim()
}
const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Scripts the app runs; everything else under `scripts/` is development-only.
const RUNTIME_SCRIPTS: [&str; 2] = ["classifier_worker.py", "tier2_lib.py"];
const PACKAGE_DOCS: [&str; 3] = ["LICENSE", "EULA.md", "README.md"];

/// Bundled models: file name, SHA-256, and download URL. `yamnet.onnx` has no
/// URL: it is built from Google's YAMNet Keras weights by `tools/yamnet/convert.py`.
const MODELS: [(&str, &str, Option<&str>); 2] = [
    ("yamnet.onnx", "ca1d489ec98848d73e8e7816003c72c960148f1e985fdb2b0cab0d2b10e250a4", None),
    (
        "yamnet_class_map.csv",
        "cdf24d193e196d9e95912a2667051ae203e92a2ba09449218ccb40ef787c6df2",
        Some(
            "https://raw.githubusercontent.com/tensorflow/models/c14bf9ad91962cf189f9f58db2132c06247fcd53/research/audioset/yamnet/yamnet_class_map.csv",
        ),
    ),
];
/// Licence and attribution notices shipped next to the models.
const MODEL_NOTICE: &str = "NOTICE.md";
const YAMNET_WEIGHTS_URL: &str = "https://storage.googleapis.com/audioset/yamnet.h5";
const YAMNET_WEIGHTS_SHA256: &str = "13c3308955bbfaef262f175ac9c40e47b134573a93984f009220dd7cc12a1744";
/// Pinned so the converted model is byte-identical to `MODELS`' hash.
const YAMNET_CONVERT_DEPS: [&str; 3] = ["onnx==1.17.0", "h5py==3.12.1", "numpy==2.2.6"];

#[derive(Parser)]
#[command(name = "xtask", about = "Build and release tasks for Tundra")]
enum Commands {
    /// Full dev setup: LFS assets, classifier models, and Python envs.
    Setup {
        /// Skip `git lfs pull`
        #[arg(long)]
        skip_lfs: bool,
        /// Skip the ONNX runtime (`--group dl`); tier 2 falls back to the spectral heuristic
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
        #[command(flatten)]
        setup: SetupBeforeBuild,
    },
    /// `cargo run` (runs setup first).
    Run {
        #[arg(long, short)]
        release: bool,
        #[command(flatten)]
        setup: SetupBeforeBuild,
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
        #[command(flatten)]
        options: PackageOptions,
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
        #[command(flatten)]
        setup: SetupBeforeBuild,
    },
}

/// Flags for commands that run `setup` before building.
#[derive(Args)]
struct SetupBeforeBuild {
    /// Skip setup step
    #[arg(long)]
    no_setup: bool,
    /// Skip the ONNX runtime during setup
    #[arg(long)]
    skip_dl: bool,
}

impl SetupBeforeBuild {
    fn run(&self) -> Result<()> {
        if self.no_setup { Ok(()) } else { setup(false, self.skip_dl) }
    }
}

#[derive(Args, Default)]
struct PackageOptions {
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
}

fn main() -> Result<()> {
    match Commands::parse() {
        Commands::Setup { skip_lfs, skip_dl } => setup(skip_lfs, skip_dl),
        Commands::Models => download_models(),
        Commands::Classifiers { skip_dl } => setup_classifiers(skip_dl),
        Commands::Build { release, target, cross, setup } => {
            setup.run()?;
            cargo_build(release, target.as_deref(), cross)
        }
        Commands::Run { release, setup, args } => {
            setup.run()?;
            cargo_run(release, &args)
        }
        Commands::Package { version, options } => {
            package_release(&version.unwrap_or_else(release_tag), &options).map(drop)
        }
        Commands::Release { ci, skip_build } => release(ci, skip_build),
        Commands::Cross { command } => match command {
            CrossCommands::InstallTargets => install_release_targets(),
            CrossCommands::BuildAll { cross, setup } => {
                setup.run()?;
                cross_build_all(cross)
            }
        },
    }
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask crate should live in project root").to_path_buf()
}

/// `v` + the app's version from the root Cargo.toml.
fn release_tag() -> String {
    let manifest = std::fs::read_to_string(project_root().join("Cargo.toml")).unwrap_or_default();
    let version = manifest.split("[package]").nth(1).and_then(|package| {
        package.lines().find_map(|line| {
            let value = line.trim().strip_prefix("version")?.trim().strip_prefix('=')?;
            Some(value.trim().trim_matches('"').to_string())
        })
    });
    format!("v{}", version.as_deref().unwrap_or("0.0.0"))
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
    if !Command::new("git").args(["lfs", "version"]).output().is_ok_and(|out| out.status.success()) {
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

fn matches_hash(path: &Path, sha256: &str) -> Result<bool> {
    Ok(path.is_file() && sha256_file(path)? == sha256)
}

/// Download `url` to `dest` through a `.part` file, keeping it only if its
/// SHA-256 matches.
fn fetch_verified(url: &str, sha256: &str, dest: &Path) -> Result<()> {
    let part = dest.with_added_extension("part");
    let response = ureq::get(url).timeout(MODEL_DOWNLOAD_TIMEOUT).call().with_context(|| format!("GET {url}"))?;
    let mut file = std::fs::File::create(&part)?;
    std::io::copy(&mut response.into_reader(), &mut file).with_context(|| format!("write {}", part.display()))?;
    drop(file);
    let actual = sha256_file(&part)?;
    if actual != sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{url} has sha256 {actual}, expected {sha256}; refusing to use it");
    }
    Ok(std::fs::rename(&part, dest)?)
}

/// Rebuild `yamnet.onnx` from the official weights.
fn convert_yamnet(dest: &Path) -> Result<()> {
    let root = project_root();
    let weights = root.join("target/yamnet/yamnet.h5");
    std::fs::create_dir_all(weights.parent().expect("parent"))?;
    if !matches_hash(&weights, YAMNET_WEIGHTS_SHA256)? {
        println!("models: downloading YAMNet weights");
        fetch_verified(YAMNET_WEIGHTS_URL, YAMNET_WEIGHTS_SHA256, &weights)?;
    }
    // Staged like a download, so an interrupted build never looks complete.
    let part = dest.with_added_extension("part");
    let mut convert = Command::new("uv");
    convert.args(["run", "--no-project", "--python", python_version()]);
    for dependency in YAMNET_CONVERT_DEPS {
        convert.args(["--with", dependency]);
    }
    convert.arg("python").arg(root.join("tools/yamnet/convert.py"));
    run(convert.arg(&weights).arg(&part).current_dir(&root))?;
    Ok(std::fs::rename(&part, dest)?)
}

/// Fetch or rebuild any model that is missing, truncated, a Git LFS pointer,
/// or otherwise does not match its pinned hash.
fn download_models() -> Result<()> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for (name, sha256, url) in MODELS {
        let dest = dir.join(name);
        if matches_hash(&dest, sha256)? {
            println!("models: {name} verified");
            continue;
        }
        println!("models: {} {name}", if url.is_some() { "downloading" } else { "building" });
        match url {
            Some(url) => fetch_verified(url, sha256, &dest)?,
            None => convert_yamnet(&dest)?,
        }
    }
    verify_models()
}

fn verify_models() -> Result<()> {
    for (name, sha256, _) in MODELS {
        let path = models_dir().join(name);
        ensure!(
            matches_hash(&path, sha256)?,
            "{} is missing or does not match its pinned hash; run `cargo xtask models`",
            path.display()
        );
    }
    Ok(())
}

fn setup_classifiers(skip_dl: bool) -> Result<()> {
    let scripts = project_root().join("scripts");
    ensure!(scripts.join("pyproject.toml").is_file(), "missing scripts/pyproject.toml");
    run(Command::new("uv").args(["python", "install", python_version()]).current_dir(&scripts))?;
    let mut sync = Command::new("uv");
    sync.args(["sync", "--locked", "--python", python_version()]).current_dir(&scripts);
    if skip_dl {
        println!("classifiers: skipping the ONNX runtime (--skip-dl)");
    } else {
        sync.args(["--group", "dl"]);
    }
    run(&mut sync)
}

fn host_triple() -> Result<String> {
    let version = output(Command::new("rustc").arg("-vV")).unwrap_or_default();
    let host = version.lines().find_map(|line| line.strip_prefix("host: "));
    host.map(str::to_string).context("could not detect the host triple; pass --target explicitly")
}

/// Whether building `target` from this host needs the `cross` tool.
fn needs_cross(target: &str) -> Result<bool> {
    let host = host_triple()?;
    let darwin = target.contains("darwin");
    ensure!(!darwin || host.contains("darwin"), "{target} can only be built on a macOS host");
    Ok(host != target && !darwin)
}

fn install_release_targets() -> Result<()> {
    for &target in cross_targets_for_host() {
        run(Command::new("rustup").args(["target", "add", target]))?;
    }
    Ok(())
}

fn cross_targets_for_host() -> &'static [&'static str] {
    if cfg!(windows) {
        &["x86_64-pc-windows-msvc"]
    } else if cfg!(target_os = "macos") {
        &["x86_64-apple-darwin", "aarch64-apple-darwin"]
    } else {
        &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-gnu"]
    }
}

fn cross_build_all(cross: bool) -> Result<()> {
    let mut failures = Vec::new();
    for &target in cross_targets_for_host() {
        println!("cross: building {target}");
        if let Err(err) = cargo_build(true, Some(target), cross) {
            eprintln!("cross: {target} failed: {err:#}");
            failures.push(target);
        }
    }
    ensure!(failures.is_empty(), "failed targets: {}", failures.join(", "));
    Ok(())
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

fn label(command: &Command) -> String {
    let words = std::iter::once(command.get_program()).chain(command.get_args());
    words.map(|word| word.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

fn run(command: &mut Command) -> Result<()> {
    let label = label(command);
    let status = command.stdin(Stdio::inherit()).status().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => anyhow!("`{label}`: required tool is not installed or not on PATH"),
        _ => anyhow!("`{label}`: failed to start: {err}"),
    })?;
    ensure!(status.success(), "`{label}` failed with {status}");
    Ok(())
}

fn output(command: &mut Command) -> Result<String> {
    let label = label(command);
    let result = command.stderr(Stdio::inherit()).output().with_context(|| format!("run {label}"))?;
    ensure!(result.status.success(), "`{label}` failed with {}", result.status);
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

fn copy_file(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst).with_context(|| format!("copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

/// Install a standalone CPython plus the locked classifier dependencies into
/// `python/`. Packages go into `python/site-packages` (found via PYTHONPATH at
/// runtime) instead of a virtualenv, whose absolute interpreter path would
/// break as soon as the archive is unpacked anywhere else.
fn bundle_python(staging: &Path) -> Result<()> {
    let python_root = staging.join("python");
    std::fs::create_dir_all(&python_root)?;
    run(Command::new("uv").args(["python", "install", python_version()]).env("UV_PYTHON_INSTALL_DIR", &python_root))?;
    let python = std::fs::read_dir(&python_root)?
        .flatten()
        .flat_map(|entry| [entry.path().join("python.exe"), entry.path().join("bin/python3")])
        .find(|candidate| candidate.is_file())
        .with_context(|| format!("no python executable found under {}", python_root.display()))?;

    let requirements = staging.join("requirements.txt");
    let export = "export --locked --group dl --no-hashes --no-emit-project --format requirements-txt";
    run(Command::new("uv")
        .args(export.split(' '))
        .args(["--python", python_version(), "--output-file"])
        .arg(&requirements)
        .current_dir(project_root().join("scripts")))?;
    run(Command::new("uv")
        .args(["pip", "install", "--python"])
        .arg(&python)
        .arg("--target")
        .arg(python_root.join("site-packages"))
        .arg("-r")
        .arg(&requirements))?;
    Ok(std::fs::remove_file(&requirements)?)
}

/// Builds `target/package/tundra-<version>-<target>/` and archives it with that
/// folder at the top, plus a `.sha256` file. Returns the archive paths.
fn package_release(version: &str, options: &PackageOptions) -> Result<Vec<PathBuf>> {
    let target = match &options.target {
        Some(target) => target.clone(),
        None if options.skip_build => bail!("--skip-build requires --target"),
        None => host_triple()?,
    };
    verify_models()?;
    if !options.skip_build {
        cargo_build(true, Some(&target), options.cross)?;
    }

    let root = project_root();
    let windows = target.contains("windows");
    let bin_name = if windows { "tundra.exe" } else { "tundra" };
    let exe = root.join("target").join(&target).join("release").join(bin_name);
    ensure!(exe.is_file(), "missing release binary at {}", exe.display());

    let name = format!("tundra-{version}-{target}");
    let package_dir = root.join("target/package");
    let staging = package_dir.join(&name);
    if staging.exists() {
        std::fs::remove_dir_all(&staging).with_context(|| format!("clean {}", staging.display()))?;
    }
    std::fs::create_dir_all(staging.join("models"))?;
    std::fs::create_dir_all(staging.join("scripts"))?;

    copy_file(&exe, &staging.join(bin_name))?;
    for name in MODELS.map(|(name, ..)| name).into_iter().chain([MODEL_NOTICE]) {
        copy_file(&models_dir().join(name), &staging.join("models").join(name))?;
    }
    for script in RUNTIME_SCRIPTS.map(|script| Path::new("scripts").join(script)) {
        copy_file(&root.join(&script), &staging.join(&script))?;
    }
    for doc in PACKAGE_DOCS {
        copy_file(&root.join(doc), &staging.join(doc))?;
    }

    if options.skip_python {
        println!("package: skipping bundled Python (--skip-python)");
    } else if host_triple().is_ok_and(|host| host == target) {
        bundle_python(&staging)?;
    } else {
        eprintln!("package: not bundling Python for {target} (only the host's Python can be bundled)");
    }

    let archive = package_dir.join(format!("{name}.{}", if windows { "zip" } else { "tar.gz" }));
    if archive.is_file() {
        std::fs::remove_file(&archive)?;
    }
    // bsdtar (bundled with Windows 10+ and macOS) picks the format from the
    // extension with -a; GNU tar handles .tar.gz with -z.
    let mut tar = Command::new("tar");
    tar.arg(if windows { "-a" } else { "-z" });
    run(tar.arg("-cf").arg(&archive).arg("-C").arg(&package_dir).arg(&name))?;

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
    let tool = |program: &str, args: &[&str]| {
        let mut command = Command::new(program);
        command.args(args).current_dir(&root);
        command
    };
    let git = |args: &[&str]| output(&mut tool("git", args));

    ensure!(git(&["status", "--porcelain"])?.is_empty(), "working tree has uncommitted changes");
    let head = git(&["rev-parse", "HEAD"])?;
    run(&mut tool("git", &["fetch", "--tags", "origin"]))?;
    ensure!(
        !git(&["branch", "-r", "--contains", &head])?.is_empty(),
        "HEAD {head} is not on any remote branch; push it first"
    );
    match git(&["rev-parse", &format!("refs/tags/{tag}^{{commit}}")]) {
        Ok(tagged) if tagged != head => {
            bail!("{tag} already points at {tagged}; bump the version in Cargo.toml instead of moving it")
        }
        Ok(_) => {}
        Err(_) => run(&mut tool("git", &["tag", "-a", &tag, "-m", &tag]))?,
    }
    // Also covers a rerun after the push failed but the local tag was created.
    if git(&["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")])?.is_empty() {
        run(&mut tool("git", &["push", "origin", &tag]))?;
    }

    match output(&mut tool("gh", &["release", "view", &tag, "--json", "isDraft", "--jq", ".isDraft"])).as_deref() {
        Ok("true") => {}
        Ok(_) => bail!("release {tag} is already published; its assets are left untouched"),
        Err(_) => run(&mut tool("gh", &["release", "create", &tag, "--draft", "--verify-tag", "--generate-notes"]))?,
    }

    let assets = if skip_build {
        let prefix = format!("tundra-{tag}-{}.", host_triple()?);
        let dir = root.join("target/package");
        std::fs::read_dir(&dir)
            .with_context(|| format!("read {}", dir.display()))?
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect()
    } else {
        package_release(&tag, &PackageOptions::default())?
    };
    ensure!(!assets.is_empty(), "no packages for {tag} under target/package");
    // Uploading to a draft may replace this host's own earlier upload, never a
    // published asset.
    run(tool("gh", &["release", "upload", &tag, "--clobber"]).args(&assets))?;

    if ci {
        run(&mut tool("gh", &["workflow", "run", "release.yml", "--ref", &tag, "-f", &format!("tag={tag}")]))?;
    }
    println!("release: draft {tag} updated; publish it on GitHub once every platform is attached");
    Ok(())
}
