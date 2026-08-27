# Architecture

Tundra is one Rust binary. `main.rs` calls `types::app()`, the iced application.

## Source layout

| Path | Contents |
|------|----------|
| `src/*.rs` | Metadata, tagging, path I/O, audio decode, peaks |
| `src/types/` | UI: file list, player, waveform, modals |
| `src/metadata/` | Tag read/write, search, caches, path hints |
| `src/auto_tag/` | Instrument classifier |
| `xtask/` | Setup, cross-build, packaging (not in the binary) |

## `main.rs` modules

| Module | Role |
|--------|------|
| `types/` | iced app shell |
| `metadata/` | Tags and search |
| `auto_tag/` | Classifier (Rust tier 1, Python tier 2) |
| `bulk_auto_tag.rs` | Folder scan/apply for auto-tags |
| `tag_store.rs` | SQLite sidecar when native tags fail |
| `path_util.rs` | Atomic writes, cache keys, tmp reclaim |
| `source/` | Playback backends |
| `waveform_peaks.rs` | Peak data for the player |
| `drag_out.rs` | Drag file out of the waveform |
| `launch.rs` | Open-with / CLI startup paths |

## `types/` modules

| Module | Role |
|--------|------|
| `app/` | State, `update`, layout (~3k lines; see below) |
| `app/cache.rs` | Dir + metadata disk caches |
| `app/prefs.rs` | Sidebar width, volume, loop, always-on-top |
| `app/helpers.rs` | Background search, walks, drag state |
| `common.rs` | `Message` enum, shared widgets |
| `file_selector.rs` | File list and search UI |
| `waveform.rs` | Waveform view |
| `player.rs` | Transport and worker thread |
| `settings.rs` | Allowed directories, favorites |
| `bulk_auto_tag.rs`, `auto_tag.rs`, `tag_editor.rs`, `menu.rs` | Feature UI |

Read `types/common.rs` first for `Message`, then `types/app/mod.rs` for routing, then `metadata/mod.rs` for tag I/O.

## `metadata/` modules

| File | Role |
|------|------|
| `fields.rs` | `TagField`, `TagFields`, filters, manual edits |
| `read.rs` | Read tags per container (WAV, FLAC, MP3, …) |
| `write.rs` | Manual and auto tag writes |
| `search.rs` | Filename and tag search |
| `cache.rs` | `CachedMetadata`, index refresh |
| `hints.rs` | Instrument/artist hints from paths |
| `auto_tag.rs` | Whether auto-tags need upgrade |
| `tests.rs` | Tag/search integration tests |

Use `crate::metadata::*`; submodules are internal.

## `auto_tag/` modules

| File | Role |
|------|------|
| `tier1.rs` | ZCR heuristic |
| `classifier_pool.rs` | Python workers (librosa / Essentia) |
| `classify_cache.rs` | Persisted classification results |
| `mod.rs` | Orchestration |

Tier 2 backend is picked at setup (see README).

## File I/O

Shared rules in `path_util.rs`, covered by `data_safety_tests.rs`:

- Write via `.tmp`, sync, rename
- Reclaim stale `.tmp` / `.bak` from dead processes
- Fall back to `tag_store` when the container cannot be written
- Bump `metadata_cache_v10` / `TUNDRA_TAG_VERSION` when tag layout changes

## Tests

Unit tests sit next to the code. Also: `metadata/tests.rs`, `data_safety_tests.rs`, `test_fixtures.rs`.

```bash
cargo test
```

## Large files

Still single files: `types/waveform.rs`, `types/file_selector.rs`, `types/player.rs`, most of `types/app/mod.rs`.
