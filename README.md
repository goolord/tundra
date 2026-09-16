# Tundra

A fast sample browser for your audio library.

https://github.com/user-attachments/assets/eee147c7-0ccd-4125-9657-f97054487d6c

Browse folders, search by filename or tag, preview waveforms, and auto-tag untagged samples.

## Features

- Browse directories for audio samples (FLAC, WAV, MP3, OGG, AIFF)
- Fuzzy file search with directory caching
- Tag filters (`title:value`, `artist:value`, etc.) with autocomplete
- Auto-tag untagged files by instrument type
- Bulk auto-tag: scan a folder, review suggestions, apply in batch
- Waveform preview with playback; zoom in to see individual sample points

## Your files

Tundra only writes to an audio file when you save tags or apply auto-tags. Every write:

- edits a copy next to the original and swaps it in atomically, so a crash or full disk never leaves a half-written file;
- checks that the copy's audio is unchanged (byte-for-byte for WAV, AIFF, FLAC, and MP3) and that the new tags read back, and otherwise leaves the original alone;
- stops if another program changes the file meanwhile;
- keeps metadata Tundra does not manage (cover art, other ID3 frames, WAV sampler and cue chunks).

Instruments you set by hand are never replaced by auto-tag. When a file cannot hold tags, Tundra keeps them in a small database in its data directory instead, so search still finds the file.

Settings, favorites, and caches live in your OS config, cache, and data directories under `tundra/`. Caches can be rebuilt at any time; a settings file that cannot be read is kept aside as `*.unreadable-<timestamp>` rather than overwritten.

## Open files from your desktop

Pass a file path on the command line:

```bash
tundra ~/Music/kick.wav
```

**Linux:** install `packaging/linux/tundra.desktop` into `~/.local/share/applications/` (set `Exec=` to your binary path), then pick Tundra as the default app or use Open With.

**macOS:** put the binary in `Tundra.app/Contents/MacOS/tundra` and use `packaging/macos/Info.plist` as `Contents/Info.plist` so Finder passes file paths at launch.

**Windows:** edit `packaging/windows/open-with.reg` with your `tundra.exe` path and import it, or use Open with once per extension.

Opening a file jumps to its folder, selects it, and starts playback.

## Build from source

See [CONTRIBUTING.md](CONTRIBUTING.md).

Some icons from [Font Awesome](https://fontawesome.com/license). Auto-tag uses [YAMNet](https://github.com/tensorflow/models/tree/master/research/audioset/yamnet) (Apache-2.0) with class names from the [AudioSet ontology](https://research.google.com/audioset/ontology/index.html) (CC BY-SA 4.0); see `resources/models/NOTICE.md`.
