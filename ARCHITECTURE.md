# Architecture

Tundra is one Rust binary. `main.rs` calls `types::app()`, the iced application.

## Source layout

| Path | Contents |
|------|----------|
| `src/*.rs` | Sidecar tag store, path I/O, peaks, drag-out, launch args |
| `src/types/` | UI: file list, player, waveform, modals |
| `src/metadata/` | Tag read/write, search, path hints |
| `src/auto_tag/` | Instrument classifier |
| `src/source/` | Streaming playback source |
| `scripts/` | Python tier-2 classifier worker (YAMNet) |
| `tools/yamnet/` | Builds and verifies `resources/models/yamnet.onnx` (not shipped) |
| `xtask/` | Setup, packaging, releases (not in the binary) |

Read `types/common.rs` first for `Message`, then `types/app/mod.rs` for routing, then `metadata/mod.rs` for tag I/O.

## `types/` modules

| Module | Role |
|--------|------|
| `app/mod.rs` | State, `update`, `view` |
| `app/cache.rs` | Directory listings and tag index (`PersistedMap`) |
| `app/prefs.rs` | Sidebar width, volume, loop, always-on-top |
| `app/helpers.rs` | Background search, directory walks, drag state |
| `common.rs` | `Message` enum, shared widgets |
| `file_selector.rs` | File list and search UI |
| `waveform.rs` | Waveform canvas |
| `player.rs` | Transport and the audio worker thread |
| `settings.rs` | Allowed directories, favorites |
| `bulk_auto_tag.rs`, `auto_tag.rs`, `tag_editor.rs`, `menu.rs` | Feature UI |

## `metadata/` modules

| File | Role |
|------|------|
| `fields.rs` | `TagField`, `TagFields`, filters, manual edits |
| `read.rs` | Read tags per container, Tundra marker comments |
| `write.rs` | The single tag write path (see File I/O) |
| `riff.rs` | IFF chunk parsing/encoding; WAV tag writer |
| `verify.rs` | Audio fingerprints and read-back checks for staged writes |
| `search.rs` | Filename and tag search |
| `cache.rs` | `CachedMetadata`, `MetadataLookup` |
| `hints.rs` | Instrument/artist hints from paths |
| `auto_tag.rs` | Which fields auto-tag may fill or replace |

Use `crate::metadata::*`; submodules are internal.

## `auto_tag/` modules

| File | Role |
|------|------|
| `tier1.rs` | Zero-crossing-rate heuristic |
| `classifier_pool.rs` | Long-lived Python workers (JSON lines, request ids) |
| `classify_cache.rs` | Results keyed by path, size, and mtime |
| `mod.rs` | Orchestration, path hints, bundled Python lookup |

## Threads

- **UI:** iced `update`/`view`. `view` must not touch the filesystem; anything it shows is computed in `update` or read from in-memory caches.
- **Audio worker:** owns the output stream; one `rodio::Sink` per playback segment. Events carry a track id so stale ones are ignored.
- **Peak builder:** one thread per loaded file, cancelled when another file loads.
- **Searches, walks, indexing, tag writes in bulk:** background tasks (`run_blocking` or iced's executor). Cache saves are coalesced onto a background thread.
- **Classifier workers:** up to two Python processes, reaped after five idle minutes.

## File I/O and data safety

Tundra writes to users' audio files only through `metadata::write::write_tags`:

1. Copy the file to a same-directory temp (`<name>.tundra-tag-<pid>-<n>.tmp`).
2. Edit the copy with the container's own tag type. WAV goes through `riff.rs` so sampler chunks survive; nothing uses lofty's lossy generic `Tag`.
3. Verify the copy: the non-tag audio payload must hash identically, and every field written must read back.
4. Refuse if the original's size or mtime changed meanwhile, or it vanished.
5. Swap the copy in atomically (`rename`, or `ReplaceFileW` on Windows), then carry any sidecar row onto the new stamp.

If the container cannot take the write, fields go to the SQLite sidecar store (`tag_store.rs`) instead, stamped with the file's size and mtime and marked as user-owned when they came from the tag editor.

Other rules, covered by `data_safety_tests.rs` and module tests:

- App data (settings, favorites, caches) is written with `path_util::write_atomic`: temp, fsync, rename.
- A settings or favorites file that cannot be read is moved aside (`.unreadable-<ts>`), never overwritten by defaults.
- Stale temps are reclaimed during directory walks (`SidecarSweep`) and in the cache/config dirs at startup. A live process's temp is never touched, and a deleted audio file is never resurrected from a temp.
- Favorites are never pruned automatically.
- Auto-tag only marks files whose instrument it wrote; hand-set instruments are never replaced.
- Bump `metadata_cache_v10` / `classify_cache_v6` / `TUNDRA_TAG_VERSION` when their layouts or meanings change.
