# Architecture

Tundra is one Rust binary. `main.rs` calls `ui::run()`, which starts the iced application.

## Source layout

Only `ui/` depends on iced. Everything else is plain Rust that the UI drives, usually from background threads.

| Path | Contents |
|------|----------|
| `ui/` | The iced app: state, message routing, and views |
| `library/` | Allowed folders and favorites, directory walks, the persisted listing and tag caches, running a search |
| `metadata/` | Reading and writing tags, tag search, path hints |
| `auto_tag/` | Instrument classifier (tier 1 in Rust, tier 2 in Python workers) |
| `bulk_auto_tag.rs` | Scanning a folder, classifying it, and applying accepted tags |
| `playback/` | The audio thread, the decoding source it plays, and the shared playhead |
| `waveform_peaks.rs` | Min/max peaks for drawing, built on a background thread |
| `tag_store.rs` | SQLite fallback for tags a file cannot hold |
| `safe_write.rs` | Atomic writes, and recovery from temp files a crash leaves behind |
| `app_data.rs` | Tundra's cache/config/data directories and the files saved there |
| `path_util.rs` | Path spellings (`cache_key`), labels, `FileStamp` |
| `platform.rs` | File manager, child processes, finding bundled assets |
| `drag_out.rs` | Dragging files out to other apps (OLE, NSDraggingSession, XDND) |
| `launch.rs` | Files and folders passed on the command line |
| `locks.rs` | Lock guards that ignore poisoning |
| `scripts/` | Python tier-2 classifier worker (YAMNet) |
| `tools/yamnet/` | Builds and verifies `resources/models/yamnet.onnx` (not shipped) |
| `xtask/` | Setup, packaging, releases (not in the binary) |

To find your way around, read `ui/message.rs` (every event the UI handles), then `ui/app/mod.rs` (state and routing), then `metadata/mod.rs`.

## `ui/`

The app follows iced's Elm-style loop: `update` changes state in response to a `Message`, `view` draws the state, and `subscription` turns outside events and timers into messages.

| File | Role |
|------|------|
| `message.rs` | `Message`, with nested enums per feature (`FilterMsg`, `WaveformMsg`, `BulkAutoTagMsg`, …) |
| `app/mod.rs` | `App` state, start-up, and `update`, which only dispatches |
| `app/input.rs` | Pointer and keyboard: drag-out, sidebar resizing, the list scrollbar, title bar, shortcuts |
| `app/library.rs` | Opening files and folders, walks, search and tag filters, favorites, caches |
| `app/modals.rs` | Settings, Auto Tag, and Edit Tags handlers |
| `app/bulk_auto_tag.rs` | Bulk Auto Tag handlers |
| `app/view.rs`, `app/subscription.rs` | Window layout; event listeners and timers |
| `app/prefs.rs` | Sidebar width, volume, loop, always-on-top |
| `file_selector.rs` | The sidebar: file list, scrollbar, filter dock |
| `player.rs` | Waveform toolbar and transport controls, and the UI side of playback |
| `waveform/` | The waveform canvas: `view.rs` (zoom/pan math), `draw.rs` (rendering), `mod.rs` (input) |
| `auto_tag.rs`, `bulk_auto_tag.rs`, `tag_editor.rs`, `settings.rs`, `dialog.rs` | Modals: their state and views |
| `menu.rs` | The custom title bar and menus |
| `selection.rs` | Click, Ctrl+click, Shift+click selection, shared by both lists |
| `style.rs`, `widgets.rs` | Shared colors and style functions; small widget builders |

### Adding a UI feature

1. Add a variant to the right enum in `message.rs` (or a new nested enum plus a line in the `nested!` macro).
2. Handle it in the matching `app/*.rs` file. Long-running work goes through `background` so `update` stays fast.
3. Draw it in the feature's view file, using `style::*` and `widgets::*` rather than inline `Style { .. }` literals.

## `metadata/`

| File | Role |
|------|------|
| `fields.rs` | `TagField`, `TagFields`, `ManualTagEdits`, tag filter parsing |
| `read.rs` | Read tags per container, Tundra marker comments, `is_audio` |
| `write.rs` | The single tag write path (see File I/O) |
| `riff.rs` | IFF chunk parsing/encoding; WAV tag writer |
| `verify.rs` | Audio fingerprints and read-back checks for staged writes |
| `search.rs` | Filename and tag search (`search`, `SearchQuery`) |
| `cache.rs` | `CachedMetadata`, `MetadataLookup` |
| `hints.rs` | Instrument/artist hints from paths |
| `auto_tag.rs` | Which fields auto-tag may fill or replace |

`metadata/mod.rs` lists the public API; submodules are internal.

## `auto_tag/`

| File | Role |
|------|------|
| `mod.rs` | `classify_file`: cache, then tier 1, then tier 2, then path hints |
| `tier1.rs` | Zero-crossing-rate heuristic |
| `classifier_pool.rs` | Finding Python; long-lived workers speaking JSON lines with request ids |
| `classify_cache.rs` | Results keyed by path and `FileStamp` |

## Threads

- **UI:** iced `update`/`view`. `view` must not touch the filesystem; anything it shows is computed in `update` or read from in-memory caches.
- **Audio worker (`playback/worker.rs`):** owns the output stream; one `rodio::Sink` per playback segment. Events carry a track id so stale ones are ignored.
- **Peak builder:** one thread per loaded file, cancelled when another file loads.
- **Searches, walks, indexing, tag writes, bulk jobs:** background threads via `background`, or iced's executor. Results come back as messages; searches and bulk jobs carry a generation number so a stale result is dropped. Cache saves are coalesced onto a background thread.
- **Classifier workers:** up to two Python processes, reaped after five idle minutes.

## File I/O and data safety

Tundra writes to users' audio files only through `metadata::write::write_tags`:

1. Copy the file to a same-directory temp (`<name>.tundra-tag-<pid>-<n>.tmp`).
2. Edit the copy with the container's own tag type. WAV goes through `riff.rs` so sampler chunks survive; nothing uses lofty's lossy generic `Tag`.
3. Verify the copy: the non-tag audio payload must hash identically, and every field written must read back.
4. Refuse if the original's size or mtime changed meanwhile, or it vanished.
5. Swap the copy in atomically (`rename`, or `ReplaceFileW` on Windows), then carry any tag store row onto the new stamp.

If the container cannot take the write, fields go to the SQLite tag store (`tag_store.rs`) instead, stamped with the file's size and mtime and marked as user-owned when they came from the tag editor.

Other rules, covered by `data_safety_tests.rs` and module tests:

- App data (settings, favorites, caches) is written with `safe_write::write_atomic`: temp, fsync, rename.
- A settings or favorites file that cannot be read is moved aside (`.unreadable-<ts>`), never overwritten by defaults.
- Stale temps are reclaimed during directory walks (`SidecarSweep`) and in the cache/config dirs at startup. A live process's temp is never touched, and a deleted audio file is never resurrected from a temp.
- Favorites are never pruned automatically.
- Auto-tag only marks files whose instrument it wrote; hand-set instruments are never replaced.
- Bump `metadata_cache_v10` / `classify_cache_v6` / `TUNDRA_TAG_VERSION` when their layouts or meanings change.
