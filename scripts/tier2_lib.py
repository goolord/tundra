#!/usr/bin/env python3
"""Tier 2 classification library — models loaded once, reused by CLI and worker.

Grey-zone tier 1 passes ZCR from Rust (`src/auto_tag/tier1.rs`, 22050 Hz decode).
Tier 2 audio analysis here uses 16 kHz (librosa mel + ONNX).

Uses MTG's official ONNX exports: ``discogs-effnet-bsdynamic-1.onnx`` (dynamic
batch; same EffNet family as the retired ``discogs-effnet-bs64-1.pb`` TensorFlow
graph) plus ``mtg_jamendo_instrument-discogs-effnet-1.onnx``.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

import numpy as np

SAMPLE_RATE = 16000
ANALYSIS_SECONDS = 30.0

# Official MTG ONNX weights (see essentia.upf.edu/models).
EFFNET_MODEL = "discogs-effnet-bsdynamic-1.onnx"
INSTRUMENT_MODEL = "mtg_jamendo_instrument-discogs-effnet-1.onnx"
INSTRUMENT_LABELS = "mtg_jamendo_instrument-discogs-effnet-1.json"

PATCH_SIZE = 128
PATCH_HOP = 62
MEL_BINS = 96
FRAME_SIZE = 512
HOP_LENGTH = 256
EMBEDDING_DIM = 1280

JAMENDO_CLASS_MAP = {
    "drums": "Kick",
    "drummachine": "Kick",
    "beat": "Kick",
    "bongo": "Percussion",
    "percussion": "Percussion",
    "bass": "Bass",
    "acousticbassguitar": "Bass",
    "doublebass": "Bass",
    "bell": "Cymbal",
    "brass": "Brass",
    "piano": "Piano",
    "electricpiano": "Piano",
    "rhodes": "Piano",
    "keyboard": "Synth",
    "organ": "Organ",
    "pipeorgan": "Organ",
    "synthesizer": "Synth",
    "sampler": "One-Shot",
    "computer": "One-Shot",
    "guitar": "Guitar",
    "acousticguitar": "Guitar",
    "classicalguitar": "Guitar",
    "electricguitar": "Guitar",
    "strings": "Strings",
    "violin": "Strings",
    "viola": "Strings",
    "cello": "Strings",
    "pad": "Pad",
    "voice": "Vocal",
    "flute": "Flute",
    "saxophone": "Saxophone",
    "trumpet": "Trumpet",
    "trombone": "Trombone",
    "horn": "Horn",
    "harp": "Harp",
    "harmonica": "Harmonica",
    "clarinet": "Clarinet",
    "oboe": "Oboe",
    "accordion": "Accordion",
    "orchestra": "Orchestra",
}

_ONNX_MODELS: dict[str, Any] | None = None


def map_jamendo_class(raw_class: str) -> str:
    key = raw_class.strip().lower()
    if key in JAMENDO_CLASS_MAP:
        return JAMENDO_CLASS_MAP[key]
    cleaned = raw_class.strip().replace("_", " ")
    return cleaned[:1].upper() + cleaned[1:] if cleaned else "One-Shot"


def _env_truthy(*names: str) -> bool:
    for name in names:
        if os.environ.get(name, "").strip().lower() in {"1", "true", "yes", "on"}:
            return True
    return False


def dl_enabled() -> bool:
    return _env_truthy("TUNDRA_ONNX_DL", "TUNDRA_ESSENTIA_DL")


def bundled_model_dir() -> Path | None:
    candidates: list[Path] = []
    for env_name in ("TUNDRA_MODELS", "ESSENTIA_MODELS"):
        if env := os.environ.get(env_name):
            candidates.append(Path(env))
    script_dir = Path(__file__).resolve().parent
    project = script_dir.parent
    candidates.append(project / "resources" / "models")
    target_root = project / "target"
    for profile in ("debug", "release", "release-fast"):
        candidates.append(target_root / profile / "models")
        if target_root.is_dir():
            for child in target_root.iterdir():
                candidates.append(child / profile / "models")
    for candidate in candidates:
        if required_models_present(candidate):
            return candidate
    return None


def required_models_present(root: Path) -> bool:
    return (
        (root / EFFNET_MODEL).is_file()
        and (root / INSTRUMENT_MODEL).is_file()
        and (root / INSTRUMENT_LABELS).is_file()
    )


def load_audio(path: str):
    import librosa

    audio, _sample_rate = librosa.load(
        path,
        sr=SAMPLE_RATE,
        mono=True,
        duration=ANALYSIS_SECONDS,
    )
    if audio.size == 0:
        raise ValueError("librosa loader returned empty audio")
    return audio


def compute_mel_spectrogram(audio) -> np.ndarray:
    """Librosa mel matching Essentia ``TensorflowInputMusiCNN`` (see MTG/essentia#1471)."""
    import librosa

    mel = librosa.feature.melspectrogram(
        y=audio,
        sr=SAMPLE_RATE,
        n_fft=FRAME_SIZE,
        hop_length=HOP_LENGTH,
        n_mels=MEL_BINS,
        power=2.0,
        htk=False,
        center=True,
    )
    mel = np.log10(10000 * mel + 1)
    return mel.T.astype(np.float32)


def mel_patches(mel: np.ndarray) -> np.ndarray:
    if mel.shape[0] == 0:
        return np.zeros((0, PATCH_SIZE, MEL_BINS), dtype=np.float32)
    if mel.shape[0] < PATCH_SIZE:
        # Essentia lastPatchMode=repeat: pad short clips so one-shots still infer.
        pad = np.repeat(mel[-1:, :], PATCH_SIZE - mel.shape[0], axis=0)
        mel = np.concatenate([mel, pad], axis=0)
    patches = []
    start = 0
    while start + PATCH_SIZE <= mel.shape[0]:
        patches.append(mel[start : start + PATCH_SIZE])
        start += PATCH_HOP
    return np.stack(patches, axis=0)


def _session_io_name(session, *, output: bool, trailing: int | None = None) -> str:
    items = session.get_outputs() if output else session.get_inputs()
    if trailing is not None:
        for item in items:
            shape = item.shape
            if shape and shape[-1] == trailing:
                return item.name
    if output and len(items) > 1:
        return items[1].name
    return items[0].name


def warm() -> bool:
    """Load ONNX models once. Returns True when DL inference is available."""
    global _ONNX_MODELS
    if _ONNX_MODELS is not None:
        return bool(_ONNX_MODELS)
    if not dl_enabled():
        _ONNX_MODELS = {}
        return False

    model_dir = bundled_model_dir()
    if model_dir is None:
        _ONNX_MODELS = {}
        return False

    try:
        import onnxruntime as ort

        options = ort.SessionOptions()
        options.log_severity_level = 3
        providers = ["CPUExecutionProvider"]

        effnet = ort.InferenceSession(
            str(model_dir / EFFNET_MODEL),
            sess_options=options,
            providers=providers,
        )
        instrument = ort.InferenceSession(
            str(model_dir / INSTRUMENT_MODEL),
            sess_options=options,
            providers=providers,
        )
        labels_meta = json.loads((model_dir / INSTRUMENT_LABELS).read_text(encoding="utf-8"))
        class_count = len(labels_meta["classes"])
        _ONNX_MODELS = {
            "classes": labels_meta["classes"],
            "effnet": effnet,
            "effnet_input": _session_io_name(effnet, output=False),
            "effnet_embeddings": _session_io_name(effnet, output=True, trailing=EMBEDDING_DIM),
            "instrument": instrument,
            "instrument_input": _session_io_name(instrument, output=False, trailing=EMBEDDING_DIM),
            "instrument_output": _session_io_name(instrument, output=True, trailing=class_count),
        }
        return True
    except Exception as err:
        print(f"tier2: failed to warm ONNX models: {err}", file=sys.stderr, flush=True)
        _ONNX_MODELS = {}
        return False


def classify_with_onnx(path: str) -> tuple[str, float] | None:
    warm()
    if not _ONNX_MODELS:
        return None

    try:
        audio = load_audio(path)
        mel = compute_mel_spectrogram(audio)
        patches = mel_patches(mel)
        if patches.shape[0] == 0:
            return None

        classes = _ONNX_MODELS["classes"]
        embeddings = _ONNX_MODELS["effnet"].run(
            [_ONNX_MODELS["effnet_embeddings"]],
            {_ONNX_MODELS["effnet_input"]: patches},
        )[0]
        predictions = _ONNX_MODELS["instrument"].run(
            [_ONNX_MODELS["instrument_output"]],
            {_ONNX_MODELS["instrument_input"]: embeddings},
        )[0]
        scores = np.mean(predictions, axis=0)
        top_idx = int(np.argmax(scores))
        raw_class = classes[top_idx] if top_idx < len(classes) else "sampler"
        confidence = float(scores[top_idx])
        instrument = map_jamendo_class(raw_class)
        return instrument, min(0.98, max(0.55, confidence))
    except Exception as err:
        print(f"tier2: ONNX classifier failed: {err}", file=sys.stderr, flush=True)
        return None


def classify_with_librosa(path: str, tier1_zcr: float | None = None) -> tuple[str, float, float]:
    import librosa

    audio, sample_rate = librosa.load(
        path,
        sr=SAMPLE_RATE,
        mono=True,
        duration=ANALYSIS_SECONDS,
    )
    zcr = (
        float(tier1_zcr)
        if tier1_zcr is not None
        else float(np.mean(librosa.feature.zero_crossing_rate(audio)[0]))
    )
    centroid = float(np.mean(librosa.feature.spectral_centroid(y=audio, sr=sample_rate)[0]))
    rolloff = float(
        np.mean(librosa.feature.spectral_rolloff(y=audio, sr=sample_rate, roll_percent=0.85)[0])
    )

    if centroid < 900 and zcr < 0.07:
        instrument = "Kick"
    elif centroid < 1800 and zcr < 0.11:
        instrument = "Snare"
    elif zcr > 0.14 or rolloff > 9000:
        instrument = "Hi-Hat"
    elif centroid < 650:
        instrument = "Bass"
    elif centroid > 4500:
        instrument = "Cymbal"
    else:
        instrument = "One-Shot"

    return instrument, 0.62, zcr


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
            "engine": "onnx",
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
    mel = np.zeros((200, MEL_BINS), dtype=np.float32)
    patches = mel_patches(mel)
    assert patches.shape == (2, PATCH_SIZE, MEL_BINS), patches.shape
    short = mel_patches(np.ones((10, MEL_BINS), dtype=np.float32))
    assert short.shape == (1, PATCH_SIZE, MEL_BINS), short.shape
    assert mel_patches(np.zeros((0, MEL_BINS), dtype=np.float32)).shape[0] == 0


def _smoke_test_onnx() -> None:
    import soundfile as sf

    global _ONNX_MODELS
    _ONNX_MODELS = None
    os.environ.setdefault("TUNDRA_ONNX_DL", "1")
    if bundled_model_dir() is None:
        print("tier2_lib smoke: skipped (ONNX models not present)")
        return
    assert warm(), "ONNX warm failed with bundled models"

    path = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
    seconds = 5
    t = np.linspace(0, seconds, SAMPLE_RATE * seconds, endpoint=False)
    sf.write(path, (0.3 * np.sin(2 * np.pi * 440 * t)).astype(np.float32), SAMPLE_RATE)
    try:
        result = classify_path(path)
        assert result["engine"] == "onnx", result
        confidence = result["confidence"]
        assert confidence is not None and 0.55 <= confidence <= 0.98
        assert isinstance(result["instrument"], str) and result["instrument"]
    finally:
        Path(path).unlink(missing_ok=True)
        _ONNX_MODELS = None


def run_tests() -> None:
    _self_test()
    try:
        _smoke_test_onnx()
    except ImportError as err:
        print(f"tier2_lib smoke: skipped ({err})")
    print("tier2_lib tests ok")


if __name__ == "__main__":
    run_tests()
