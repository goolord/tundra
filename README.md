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

Some icons from [Font Awesome](https://fontawesome.com/license).
