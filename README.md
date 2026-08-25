# Tundra

Fast sample browser GUI built with [iced](https://github.com/iced-rs/iced).

## Requirements

- Rust 1.85+ (edition 2024)
- [uv](https://docs.astral.sh/uv/) (Python classifier runtime; **Python 3.14** for Essentia TensorFlow tier 2)
- Git LFS (for SVG icons in `resources/`)
- Linux: GTK 3 (for the folder picker and X11 drag-out support via `rfd` / `x11rb`)

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

Models are copied next to the binary at build time (`target/{profile}/models/`).

## Features

- Browse directories for audio samples (FLAC, WAV, MP3, OGG)
- Fuzzy file search with directory caching
- Separate tag filters (`title:value`, `artist:value`, etc.) with autocomplete
- Auto-tag untagged files (Rust ZCR tier 1 + optional Essentia TensorFlow tier 2 on Python 3.14)
- Bulk auto-tag: scan a folder, review suggestions, apply in batch
- Waveform preview with playback controls; zoom in to see individual sample points

Some icons from https://fontawesome.com/license

`resources/*` stored with git LFS.
