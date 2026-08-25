# Tundra

Fast sample browser GUI built with [iced](https://github.com/iced-rs/iced).

## Requirements

- Rust 1.85+ (edition 2024)
- Git LFS (for SVG icons in `resources/`)
- Linux: GTK 3 (for the folder picker and X11 drag-out support via `rfd` / `x11rb`)

## Run

```bash
git lfs pull
cargo run --release
```

## Features

- Browse directories for audio samples (FLAC, WAV, MP3, OGG)
- Fuzzy search with directory caching, including audio tags and metadata
- Waveform preview with playback controls; zoom in to see individual sample points

Some icons from https://fontawesome.com/license

`resources/*` stored with git LFS.
