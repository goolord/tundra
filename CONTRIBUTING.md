# Contributing

## Requirements

- Rust 1.89+ (edition 2024)
- [uv](https://docs.astral.sh/uv/) (Python classifier runtime)
- Git LFS (SVG icons and ONNX models in `resources/`)
- **Linux:** GTK 3 (folder picker via `rfd`; X11 drag-out via `x11rb`)
- **Windows:** [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (MSVC linker)
- **macOS:** Xcode command-line tools

### Classifier tiers

| Tier | Engine | Notes |
|------|--------|-------|
| 1 | Rust zero-crossing rate | Always available |
| 2 | ONNX (MTG Jamendo instrument head) | Needs `onnxruntime` (`cargo xtask setup`) |
| 2 (fallback) | Librosa spectral heuristic | Used when ONNX is missing; results are not cached |

Classifier Python is pinned to **3.12** on every platform (`scripts/.python-version`, xtask, and `auto_tag::UV_PYTHON` must agree).

The ONNX weights live in `resources/models/` (Git LFS). `cargo xtask models` verifies them against pinned SHA-256 hashes and re-downloads any that are missing or do not match.

## Build & run

See [ARCHITECTURE.md](ARCHITECTURE.md) for module layout.

```bash
cargo xtask setup      # LFS assets, verified models, Python env
cargo xtask run --release
```

Open a specific file while developing:

```bash
cargo xtask run --release -- ~/Desktop/hat.ogg
```

Dev builds find `scripts/` and `resources/models/` in the source tree; nothing is copied into `target/`.

```bash
cargo test --workspace
```

### xtask commands

| Command | What it does |
|---------|----------------|
| `cargo xtask setup` | `git lfs pull`, verify/download models, `uv sync --locked` with the ONNX group |
| `cargo xtask models` | Verify models against pinned hashes; download missing or mismatched ones |
| `cargo xtask classifiers` | `uv sync --locked` for the classifier environment |
| `cargo xtask build` | Setup + `cargo build --locked` |
| `cargo xtask run` | Setup + `cargo run` |
| `cargo xtask package` | Release build + portable archive under `target/package/` |
| `cargo xtask release` | Package this host and attach it to a draft GitHub release |
| `cargo xtask cross install-targets` | `rustup target add` for targets buildable from this host |
| `cargo xtask cross build-all` | `--release` build for every target supported from this host |

Flags:

- `--skip-lfs`: skip `git lfs pull` during setup
- `--skip-dl`: skip the ONNX runtime (tier 2 falls back to librosa)
- `--no-setup`: skip setup before build/run
- `--release`: `build` uses the full release profile; `run` uses `release-fast` (no LTO) for quicker iteration. Do not ship `release-fast` binaries.
- `--target TRIPLE`: cross-compile (see below)
- `--cross`: use `cross` instead of `cargo` for the build step

Windows MSVC builds link the C runtime statically in every profile (`.cargo/config.toml`), so release binaries do not need the Visual C++ redistributable. GPU rendering uses `wgpu` with Vulkan on Windows/Linux or Metal on macOS, plus a `tiny-skia` software fallback. DX12 is omitted to avoid a `windows` crate version conflict.

### Package layout

`cargo xtask package [--version vX.Y.Z]` produces `target/package/tundra-<version>-<target>.zip` (Windows) or `.tar.gz`, plus a `.sha256` file. The archive holds one top-level folder:

- `tundra` / `tundra.exe`
- `models/`: the verified ONNX models
- `scripts/`: `classifier_worker.py`, `tier2_lib.py`
- `python/`: a standalone CPython 3.12 and `python/site-packages` with the locked dependencies (only when the target matches the build host)
- `LICENSE`, `EULA.md`, `README.md`

The bundled Python is not a virtualenv: a venv records its interpreter's absolute path and breaks once the archive is unpacked elsewhere. The app runs `python/<cpython>/python` with `PYTHONPATH=python/site-packages`.

### Releasing

1. Bump `version` in `Cargo.toml` and commit. Push to `master`.
2. `cargo xtask release --ci` on one machine. It refuses a dirty tree or unpushed HEAD, tags `v<version>` (never moving an existing tag), creates a **draft** release, uploads this host's package, and dispatches the workflow to build the other platforms from that tag.
3. When every platform's archive is attached, publish the draft on GitHub.

Published releases are never modified by the tooling; ship a new version instead.

### Cross-compilation

Install toolchains and the [`cross`](https://github.com/cross-rs/cross) runner:

```bash
cargo install cross --locked
cargo xtask cross install-targets
```

```bash
# Linux → Windows (MinGW)
cargo xtask build --release --target x86_64-pc-windows-gnu --cross

# Linux → Linux (other arch, via cross container)
cargo xtask build --release --target aarch64-unknown-linux-gnu --cross
```

| Host | Typical targets | Notes |
|------|-----------------|-------|
| Windows | `x86_64-pc-windows-msvc` | Native MSVC |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-gnu` | GTK/Vulkan deps via `Cross.toml` |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` | Native only |

Target-specific linker flags live in [`.cargo/config.toml`](.cargo/config.toml); container packages for `cross` are in [`Cross.toml`](Cross.toml). Cross-built packages ship without a bundled Python.

## Notes

Files opened from outside configured search directories still play; search and auto-tag stay limited to configured folders.
