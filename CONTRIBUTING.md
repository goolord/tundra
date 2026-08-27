# Contributing

## Requirements

- Rust 1.85+ (edition 2024)
- [uv](https://docs.astral.sh/uv/) (Python classifier runtime)
- Git LFS (for SVG icons in `resources/`)
- **Linux:** GTK 3 (folder picker via `rfd`; X11 drag-out via `x11rb`)
- **Windows:** [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (MSVC linker for Rust)
- **macOS:** Xcode command-line tools

### Classifier tiers

| Tier | Engine | Platforms |
|------|--------|-----------|
| 1 | Rust ZCR | All |
| 2 (grey-zone) | Essentia TensorFlow | Linux, macOS (Python 3.14 + `cargo xtask setup`) |
| 2 (fallback) | Librosa spectral | All (default on Windows) |

On Windows, setup skips the TensorFlow env automatically; tier 2 uses librosa.

## Build & run

See [ARCHITECTURE.md](ARCHITECTURE.md) for module layout.

```bash
cargo xtask setup      # LFS assets, Essentia models, Python envs
cargo xtask build --release
cargo xtask run --release
```

Or one shot:

```bash
cargo xtask run --release
```

Open a specific file while developing:

```bash
cargo run --release -- ~/Desktop/snare.flac
cargo xtask run --release -- ~/Desktop/hat.ogg
```

### xtask commands

| Command | What it does |
|---------|----------------|
| `cargo xtask setup` | `git lfs pull`, download models, `uv sync` + DL env |
| `cargo xtask models` | Download bundled Essentia models to `resources/models/` |
| `cargo xtask classifiers` | Install Python deps (`uv sync`, optional DL on 3.14) |
| `cargo xtask build` | Setup + `cargo build` |
| `cargo xtask run` | Setup + `cargo run` |
| `cargo xtask package` | Release build + portable archive (exe/binary, models, bundled Python when native) |
| `cargo xtask cross install-targets` | `rustup target add` for targets buildable from this host |
| `cargo xtask cross build-all` | `--release` build for every target supported from this host |

Flags:

- `--skip-lfs`: skip `git lfs pull` during setup
- `--skip-dl`: skip Essentia TensorFlow env (tier 2 falls back to librosa)
- `--no-setup`: skip setup before build/run
- `--release`: optimized build (`build`: full release + static CRT on Windows; `run`: fast `release-fast` profile)
- `--target TRIPLE`: cross-compile (requires `--cross` when OS/arch differs; see Cross-compilation below)
- `--cross`: use `cross` instead of `cargo` for the build step

Models are copied next to the binary at build time. SVG icons are embedded in the executable.

`cargo xtask build --release` uses the full release profile: `opt-level = 3`, thin LTO, stripped symbols, and static CRT on Windows. Ship from `target/release/` (or `target/<triple>/release/`). `cargo xtask run --release` uses the `release-fast` profile (same speed opts, no LTO, dynamic CRT) and runs from `target/release-fast/`. Do not ship that binary. Plain `cargo build --release` on Windows also requires static CRT (`build.rs` enforces it). Debug builds use the dynamic CRT. GPU rendering uses `wgpu` with Vulkan on Windows/Linux or Metal on macOS, plus a `tiny-skia` software fallback when no GPU backend is available. DX12 is omitted to avoid a `windows` crate version conflict in the current dependency graph.

### Portable layout

For a copied build (not running from the source tree), place next to the executable:

- `models/`: Essentia model files (copied at build time)
- `scripts/`: classifier `.py` files plus a bundled `.venv` (release packages include `python/` as well)
- `python/`: portable CPython install (release packages only; dev builds use `cargo xtask setup`)

Release archive: `cargo xtask package --version v0.1.0-pre-alpha` (`.zip` on Windows, `.tar.gz` elsewhere)

macOS `.app` bundles also look in `Contents/Resources/` (`scripts/`, `models/`).

### Cross-compilation

Install toolchains and the [`cross`](https://github.com/cross-rs/cross) runner:

```bash
cargo install cross --locked
cargo xtask cross install-targets
```

Build for a specific triple (use `--cross` when the target OS/arch differs from the host):

```bash
# Linux → Windows (MinGW)
cargo xtask build --release --target x86_64-pc-windows-gnu --cross

# Linux → Linux (other arch, via cross container)
cargo xtask build --release --target aarch64-unknown-linux-gnu --cross

# Native Windows MSVC (no --cross)
cargo xtask build --release --target x86_64-pc-windows-msvc
```

Build every target feasible from the current host:

```bash
cargo xtask cross build-all --cross
```

Package a cross-built binary (Python bundling only when host triple matches `--target`):

```bash
cargo xtask package --target x86_64-pc-windows-gnu --cross --version v0.1.0-pre-alpha
```

Pass `--skip-build --target TRIPLE` when the binary is already built under `target/<triple>/release/`.

### Overwrite the latest GitHub release

From repo root (requires `gh` auth):

```powershell
# Windows: rebuild host package and replace assets on the latest release tag
.\scripts\release-latest.ps1

# Also rebuild Linux + macOS via GitHub Actions
.\scripts\release-latest.ps1 -Ci
```

```bash
./scripts/release-latest.sh
./scripts/release-latest.sh --ci
```

The script moves the latest release tag to `HEAD`, runs `cargo xtask package`, uploads with `gh release upload --clobber`, and optionally dispatches [`.github/workflows/release.yml`](.github/workflows/release.yml). Pass `-Tag v0.1.0-pre-alpha` (or `--tag`) to target a specific release instead of the most recent one.

| Host | Typical `--cross` targets | Notes |
|------|---------------------------|-------|
| Windows | `x86_64-pc-windows-msvc` | Native MSVC; use `--cross` for GNU triple |
| Linux | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-gnu` | GTK/Vulkan deps via `Cross.toml` |
| macOS | `x86_64-apple-darwin`, `aarch64-apple-darwin` | Native only; no Linux→macOS cross in CI |

Target-specific linker flags live in [`.cargo/config.toml`](.cargo/config.toml). Windows-gnu linkers and container apt packages for `cross` are in [`Cross.toml`](Cross.toml). `x86_64-unknown-linux-musl` is configured but not supported for this GUI (GTK/rfd need glibc).

## Notes

Files opened from outside configured search directories still play; search and auto-tag stay limited to configured folders.

`resources/*` is stored with git LFS.
