# Tundra

Fast sample browser GUI built with [iced](https://github.com/iced-rs/iced).

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

Use the project xtask (Rust-native task runner):

```bash
cargo xtask setup      # LFS assets, Essentia models, Python envs
cargo xtask build --release
cargo xtask run --release
```

Or one shot:

```bash
cargo xtask run --release
```

### xtask commands

| Command | What it does |
|---------|----------------|
| `cargo xtask setup` | `git lfs pull`, download models, `uv sync` + DL env |
| `cargo xtask models` | Download bundled Essentia models to `resources/models/` |
| `cargo xtask classifiers` | Install Python deps (`uv sync`, optional DL on 3.14) |
| `cargo xtask build` | Setup + `cargo build` |
| `cargo xtask run` | Setup + `cargo run` |

Flags:

- `--skip-lfs` — skip `git lfs pull` during setup
- `--skip-dl` — skip Essentia TensorFlow env (tier 2 falls back to librosa)
- `--no-setup` — skip setup before build/run
- `--release` — release profile

Models and SVG icons are copied next to the binary at build time.

Release builds use a size-optimized profile (`opt-level = "z"`, LTO, stripped symbols) and static CRT on Windows (`+crt-static`), so the executable does not depend on the MSVC redistributable. GPU rendering uses `wgpu` with Vulkan on Windows/Linux or Metal on macOS, plus a `tiny-skia` software fallback when no GPU backend is available. DX12 is omitted to avoid a `windows` crate version conflict in the current dependency graph.

### Portable layout

For a copied build (not running from the source tree), place next to the executable:

- `resources/` — SVG icons (`play.svg`, etc.; copied at build time)
- `models/` — Essentia model files (copied at build time; optional on Windows)
- `scripts/` — classifier `.py` files (copied at build time) plus a `uv`-synced env (`cargo xtask classifiers`)

macOS `.app` bundles also look in `Contents/Resources/` (`scripts/`, `models/`, icons).

## Features

- Browse directories for audio samples (FLAC, WAV, MP3, OGG)
- Fuzzy file search with directory caching
- Separate tag filters (`title:value`, `artist:value`, etc.) with autocomplete
- Auto-tag untagged files (Rust ZCR tier 1 + Essentia TensorFlow tier 2 on Linux/macOS, librosa on Windows)
- Bulk auto-tag: scan a folder, review suggestions, apply in batch
- Waveform preview with playback controls; zoom in to see individual sample points

Some icons from https://fontawesome.com/license

`resources/*` stored with git LFS.
