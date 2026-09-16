#!/usr/bin/env python3
"""Tier 2 classification library: models loaded once, reused by the worker.

Grey-zone tier 1 passes ZCR from Rust (`src/auto_tag/tier1.rs`).

The model is YAMNet (Google, Apache-2.0), an AudioSet event classifier
converted to ONNX by ``tools/yamnet/convert.py``. The log-mel front end below
reproduces YAMNet's ``features.py`` in numpy; ``tools/yamnet/verify.py``
checks both against the TensorFlow reference.
"""

from __future__ import annotations

import csv
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

import numpy as np

SAMPLE_RATE = 16000
ANALYSIS_SECONDS = 30.0

YAMNET_MODEL = "yamnet.onnx"
YAMNET_CLASS_MAP = "yamnet_class_map.csv"

# YAMNet feature parameters (params.py).
STFT_WINDOW = 400  # 25 ms
STFT_HOP = 160  # 10 ms
FFT_LENGTH = 512
MEL_BANDS = 64
MEL_MIN_HZ = 125.0
MEL_MAX_HZ = 7500.0
LOG_OFFSET = 0.001
PATCH_FRAMES = 96  # 0.96 s
PATCH_HOP_FRAMES = 48  # 0.48 s
MIN_WAVEFORM_SAMPLES = 15600  # one patch plus the last STFT window
PATCH_HOP_SAMPLES = 7680

# AudioSet classes that name something a sample could be labelled as. Classes
# not listed (Music, Speech-in-general context, environment) are ignored.
YAMNET_LABELS = {
    "Bass drum": "Kick",
    "Snare drum": "Snare",
    "Rimshot": "Rim",
    "Hi-hat": "Hi-Hat",
    "Cymbal": "Cymbal",
    "Clapping": "Clap",
    "Finger snapping": "Clap",
    "Cowbell": "Percussion",
    "Tabla": "Percussion",
    "Timpani": "Percussion",
    "Wood block": "Percussion",
    "Tambourine": "Percussion",
    "Rattle (instrument)": "Percussion",
    "Maraca": "Percussion",
    "Gong": "Percussion",
    "Tubular bells": "Percussion",
    "Mallet percussion": "Percussion",
    "Marimba, xylophone": "Percussion",
    "Glockenspiel": "Percussion",
    "Vibraphone": "Percussion",
    "Steelpan": "Percussion",
    "Beatboxing": "Percussion",
    "Bass guitar": "Bass",
    "Double bass": "Bass",
    "Synthesizer": "Synth",
    "Sampler": "Synth",
    "Theremin": "Synth",
    "Piano": "Piano",
    "Electric piano": "Piano",
    "Harpsichord": "Piano",
    "Organ": "Organ",
    "Electronic organ": "Organ",
    "Hammond organ": "Organ",
    "Guitar": "Guitar",
    "Electric guitar": "Guitar",
    "Acoustic guitar": "Guitar",
    "Steel guitar, slide guitar": "Guitar",
    "Tapping (guitar technique)": "Guitar",
    "Strum": "Guitar",
    "Banjo": "Guitar",
    "Mandolin": "Guitar",
    "Ukulele": "Guitar",
    "Sitar": "Guitar",
    "Bowed string instrument": "Strings",
    "String section": "Strings",
    "Violin, fiddle": "Strings",
    "Pizzicato": "Strings",
    "Cello": "Strings",
    "Orchestra": "Orchestra",
    "Brass instrument": "Brass",
    "French horn": "Brass",
    "Trumpet": "Brass",
    "Trombone": "Brass",
    "Flute": "Flute",
    "Saxophone": "Saxophone",
    "Clarinet": "Clarinet",
    "Harp": "Harp",
    "Harmonica": "Harmonica",
    "Accordion": "Accordion",
    "Singing": "Vocal",
    "Choir": "Vocal",
    "Chant": "Vocal",
    "Rapping": "Vocal",
    "Humming": "Vocal",
    "Vocal music": "Vocal",
    "A capella": "Vocal",
    "Speech": "Vocal",
    "Sound effect": "FX",
    "Whoosh, swoosh, swish": "FX",
    "Explosion": "FX",
    "Beep, bleep": "FX",
    "Scratching (performance technique)": "FX",
    "Boing": "FX",
    "Siren": "FX",
}

# Umbrella classes fire alongside the specific ones ("Drum" with "Snare drum"),
# so they only win when nothing more specific scored.
GENERIC_LABELS = {
    "Percussion": "Percussion",
    "Drum": "Percussion",
    "Drum kit": "Percussion",
    "Drum machine": "Percussion",
    "Drum roll": "Percussion",
    "Keyboard (musical)": "Synth",
    "Plucked string instrument": "Guitar",
    "Wind instrument, woodwind instrument": "Flute",
}
GENERIC_WEIGHT = 0.5
# Catch-all classes that fire on almost any isolated hit (a hi-hat reads as
# Speech, a kick as Sound effect), weighted down so real instruments win.
CLASS_WEIGHTS = {"Speech": 0.3, "Sound effect": 0.5}

# YAMNet recognises that something is a drum hit far better than which drum it
# is, so these labels only pick the family and the spectrum picks the drum.
SPECTRAL_DRUM_LABELS = {"Kick", "Snare", "Hi-Hat", "Percussion"}

_MODEL: dict[str, Any] | None = None


def dl_enabled() -> bool:
    return os.environ.get("TUNDRA_ONNX_DL", "").strip().lower() in {"1", "true", "yes", "on"}


def bundled_model_dir() -> Path | None:
    candidates: list[Path] = []
    if env := os.environ.get("TUNDRA_MODELS"):
        candidates.append(Path(env))
    candidates.append(Path(__file__).resolve().parent.parent / "resources" / "models")
    for candidate in candidates:
        if (candidate / YAMNET_MODEL).is_file() and (candidate / YAMNET_CLASS_MAP).is_file():
            return candidate
    return None


def read_class_names(path: Path) -> list[str]:
    with path.open(newline="", encoding="utf-8") as handle:
        rows = csv.DictReader(handle)
        return [row["display_name"] for row in rows]


def load_audio(path: str) -> np.ndarray:
    import librosa

    audio, _sample_rate = librosa.load(path, sr=SAMPLE_RATE, mono=True, duration=ANALYSIS_SECONDS)
    if audio.size == 0:
        raise ValueError("librosa loader returned empty audio")
    return audio.astype(np.float32)


def _mel_weights() -> np.ndarray:
    """``tf.signal.linear_to_mel_weight_matrix`` (HTK mel, DC bin zeroed)."""

    def hertz_to_mel(hz):
        return 1127.0 * np.log(1.0 + hz / 700.0)

    bins = FFT_LENGTH // 2 + 1
    frequencies = np.linspace(0.0, SAMPLE_RATE / 2.0, bins, dtype=np.float32)[1:]
    spectrogram_mel = hertz_to_mel(frequencies)[:, np.newaxis]
    edges = np.linspace(
        hertz_to_mel(np.float32(MEL_MIN_HZ)),
        hertz_to_mel(np.float32(MEL_MAX_HZ)),
        MEL_BANDS + 2,
        dtype=np.float32,
    )
    lower, center, upper = edges[:-2], edges[1:-1], edges[2:]
    lower_slopes = (spectrogram_mel - lower) / (center - lower)
    upper_slopes = (upper - spectrogram_mel) / (upper - center)
    weights = np.maximum(0.0, np.minimum(lower_slopes, upper_slopes))
    return np.pad(weights, [[1, 0], [0, 0]]).astype(np.float32)


_MEL_WEIGHTS = _mel_weights()
_HANN = (0.5 - 0.5 * np.cos(2.0 * np.pi * np.arange(STFT_WINDOW) / STFT_WINDOW)).astype(np.float32)


def yamnet_patches(waveform: np.ndarray) -> np.ndarray:
    """[N, 96, 64] log-mel patches, as YAMNet's ``pad_waveform`` and
    ``waveform_to_log_mel_spectrogram_patches`` compute them."""
    waveform = np.asarray(waveform, dtype=np.float32)
    length = max(waveform.shape[0], MIN_WAVEFORM_SAMPLES)
    after_first = length - MIN_WAVEFORM_SAMPLES
    hops = -(-after_first // PATCH_HOP_SAMPLES)
    padded_length = MIN_WAVEFORM_SAMPLES + hops * PATCH_HOP_SAMPLES
    waveform = np.pad(waveform, (0, padded_length - waveform.shape[0]))

    frame_count = 1 + (waveform.shape[0] - STFT_WINDOW) // STFT_HOP
    starts = np.arange(frame_count)[:, np.newaxis] * STFT_HOP
    frames = waveform[starts + np.arange(STFT_WINDOW)] * _HANN
    magnitude = np.abs(np.fft.rfft(frames, n=FFT_LENGTH)).astype(np.float32)
    log_mel = np.log(magnitude @ _MEL_WEIGHTS + LOG_OFFSET)

    patch_count = 1 + (log_mel.shape[0] - PATCH_FRAMES) // PATCH_HOP_FRAMES
    patch_starts = np.arange(patch_count)[:, np.newaxis] * PATCH_HOP_FRAMES
    return log_mel[patch_starts + np.arange(PATCH_FRAMES)].astype(np.float32)


def warm() -> bool:
    """Load the model once. Returns True when DL inference is available."""
    global _MODEL
    if _MODEL is not None:
        return bool(_MODEL)
    _MODEL = {}
    if not dl_enabled():
        return False
    model_dir = bundled_model_dir()
    if model_dir is None:
        return False
    try:
        import onnxruntime as ort

        options = ort.SessionOptions()
        options.log_severity_level = 3
        session = ort.InferenceSession(
            str(model_dir / YAMNET_MODEL),
            sess_options=options,
            providers=["CPUExecutionProvider"],
        )
        names = read_class_names(model_dir / YAMNET_CLASS_MAP)
        _MODEL = {"session": session, "label_classes": label_classes(names)}
        return True
    except Exception as err:
        print(f"tier2: failed to load YAMNet: {err}", file=sys.stderr, flush=True)
        _MODEL = {}
        return False


def label_classes(names: list[str]) -> dict[str, list[tuple[int, float]]]:
    """Tundra label -> [(class index, weight)]."""
    index = {name: position for position, name in enumerate(names)}
    labels: dict[str, list[tuple[int, float]]] = {}
    for table, weight in ((YAMNET_LABELS, 1.0), (GENERIC_LABELS, GENERIC_WEIGHT)):
        for name, label in table.items():
            if name not in index:
                raise ValueError(f"YAMNet class map has no class {name!r}")
            labels.setdefault(label, []).append((index[name], weight * CLASS_WEIGHTS.get(name, 1.0)))
    return labels


def best_label(
    scores: np.ndarray, labels: dict[str, list[tuple[int, float]]]
) -> tuple[str, float] | None:
    """Label with the highest weighted class score, from clip-level scores."""
    best: tuple[str, float] | None = None
    for label, classes in labels.items():
        score = max(float(scores[position]) * weight for position, weight in classes)
        if best is None or score > best[1]:
            best = (label, score)
    return best


def fill_one_patch(audio: np.ndarray) -> np.ndarray:
    """Loop a clip shorter than one YAMNet window instead of padding it with
    silence, which otherwise dominates the scores of short one-shots."""
    if audio.shape[0] == 0 or audio.shape[0] >= MIN_WAVEFORM_SAMPLES:
        return audio
    repeats = -(-MIN_WAVEFORM_SAMPLES // audio.shape[0])
    return np.tile(audio, repeats)[:MIN_WAVEFORM_SAMPLES]


def spectral_features(audio: np.ndarray, tier1_zcr: float | None = None) -> tuple[float, float, float]:
    """(zero-crossing rate, spectral centroid Hz, 85% rolloff Hz)."""
    import librosa

    zcr = (
        float(tier1_zcr)
        if tier1_zcr is not None
        else float(np.mean(librosa.feature.zero_crossing_rate(audio)[0]))
    )
    centroid = float(np.mean(librosa.feature.spectral_centroid(y=audio, sr=SAMPLE_RATE)[0]))
    rolloff = float(
        np.mean(librosa.feature.spectral_rolloff(y=audio, sr=SAMPLE_RATE, roll_percent=0.85)[0])
    )
    return zcr, centroid, rolloff


def spectral_label(zcr: float, centroid: float, rolloff: float, fallback: str) -> str:
    if centroid < 900 and zcr < 0.07:
        return "Kick"
    if centroid < 1800 and zcr < 0.11:
        return "Snare"
    if zcr > 0.14 or rolloff > 9000:
        return "Hi-Hat"
    if centroid < 650:
        return "Bass"
    if centroid > 4500:
        return "Cymbal"
    return fallback


def classify_with_onnx(path: str) -> tuple[str, float] | None:
    if not warm():
        return None
    try:
        audio = load_audio(path)
        patches = yamnet_patches(fill_one_patch(audio))
        scores = _MODEL["session"].run(["scores"], {"patches": patches})[0]
        # Max over windows: a one-shot's character is in its first window and
        # later windows of a long file should not dilute it.
        result = best_label(scores.max(axis=0), _MODEL["label_classes"])
        if result is None:
            return None
        label, score = result
        if label in SPECTRAL_DRUM_LABELS:
            label = spectral_label(*spectral_features(audio), fallback="Percussion")
        return label, min(0.98, max(0.55, score))
    except Exception as err:
        print(f"tier2: YAMNet classification failed: {err}", file=sys.stderr, flush=True)
        return None


def classify_with_librosa(path: str, tier1_zcr: float | None = None) -> tuple[str, float, float]:
    audio = load_audio(path)
    zcr, centroid, rolloff = spectral_features(audio, tier1_zcr)
    return spectral_label(zcr, centroid, rolloff, fallback="One-Shot"), 0.62, zcr


def classify_path(path: str, tier1_zcr: float | None = None) -> dict[str, Any]:
    if not Path(path).is_file():
        raise FileNotFoundError(f"file not found: {path}")

    dl = classify_with_onnx(path)
    if dl is not None:
        instrument, confidence = dl
        return {
            "tier": 2,
            "decision": "definitive",
            "instrument": instrument,
            "confidence": confidence,
            "zcr": tier1_zcr,
            "engine": "yamnet",
        }

    instrument, confidence, zcr = classify_with_librosa(path, tier1_zcr)
    return {
        "tier": 2,
        "decision": "definitive",
        "instrument": instrument,
        "confidence": confidence,
        "zcr": zcr,
        "engine": "librosa-spectral",
    }


def _self_test() -> None:
    assert yamnet_patches(np.zeros(100, dtype=np.float32)).shape == (1, PATCH_FRAMES, MEL_BANDS)
    assert yamnet_patches(np.zeros(MIN_WAVEFORM_SAMPLES, dtype=np.float32)).shape[0] == 1
    assert yamnet_patches(np.zeros(MIN_WAVEFORM_SAMPLES + 1, dtype=np.float32)).shape[0] == 2
    assert yamnet_patches(np.zeros(SAMPLE_RATE * 3, dtype=np.float32)).shape[0] == 6
    assert fill_one_patch(np.ones(100, dtype=np.float32)).shape == (MIN_WAVEFORM_SAMPLES,)
    long_clip = np.ones(MIN_WAVEFORM_SAMPLES + 5, dtype=np.float32)
    assert fill_one_patch(long_clip) is long_clip
    assert spectral_label(0.02, 500.0, 2000.0, fallback="x") == "Kick"
    labels = {"Kick": [(0, 1.0)], "Percussion": [(1, GENERIC_WEIGHT)]}
    assert best_label(np.array([0.3, 0.5]), labels) == ("Kick", 0.3)
    assert best_label(np.array([0.2, 0.5]), labels) == ("Percussion", 0.25)


def _smoke_test_onnx() -> None:
    import soundfile as sf

    global _MODEL
    _MODEL = None
    os.environ.setdefault("TUNDRA_ONNX_DL", "1")
    model_dir = bundled_model_dir()
    if model_dir is None:
        print("tier2_lib smoke: skipped (YAMNet model not present)")
        return
    label_classes(read_class_names(model_dir / YAMNET_CLASS_MAP))
    assert warm(), "YAMNet warm failed with bundled model"

    path = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
    seconds = 2
    t = np.linspace(0, seconds, SAMPLE_RATE * seconds, endpoint=False)
    sf.write(path, (0.3 * np.sin(2 * np.pi * 440 * t)).astype(np.float32), SAMPLE_RATE)
    try:
        result = classify_path(path)
        assert result["engine"] == "yamnet", result
        assert 0.55 <= result["confidence"] <= 0.98
        assert isinstance(result["instrument"], str) and result["instrument"]
    finally:
        Path(path).unlink(missing_ok=True)
        _MODEL = None


def run_tests() -> None:
    _self_test()
    try:
        _smoke_test_onnx()
    except ImportError as err:
        print(f"tier2_lib smoke: skipped ({err})")
    print("tier2_lib tests ok")


if __name__ == "__main__":
    run_tests()
