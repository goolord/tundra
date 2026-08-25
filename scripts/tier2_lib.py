#!/usr/bin/env python3
"""Tier 2 classification library — models loaded once, reused by CLI and worker.

Grey-zone tier 1 passes ZCR from Rust (`src/auto_tag/tier1.rs`, 22050 Hz decode).
Tier 2 audio analysis here uses 16 kHz (Essentia/librosa).
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any

SAMPLE_RATE = 16000
ANALYSIS_SECONDS = 30.0

EFFNET_MODEL = "discogs-effnet-bs64-1.pb"
INSTRUMENT_MODEL = "mtg_jamendo_instrument-discogs-effnet-1.pb"
INSTRUMENT_LABELS = "mtg_jamendo_instrument-discogs-effnet-1.json"

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

_TENSORFLOW_MODELS: dict[str, Any] | None = None
_ESSENTIA_LOGS_CONFIGURED = False


def _import_essentia():
    """Import essentia.standard with INFO logs suppressed."""
    global _ESSENTIA_LOGS_CONFIGURED
    configure_tensorflow_runtime()
    import essentia

    if not _ESSENTIA_LOGS_CONFIGURED:
        essentia.log.infoActive = False
        _ESSENTIA_LOGS_CONFIGURED = True

    import essentia.standard as es

    return es


def map_jamendo_class(raw_class: str) -> str:
    key = raw_class.strip().lower()
    if key in JAMENDO_CLASS_MAP:
        return JAMENDO_CLASS_MAP[key]
    cleaned = raw_class.strip().replace("_", " ")
    return cleaned[:1].upper() + cleaned[1:] if cleaned else "One-Shot"


def dl_enabled() -> bool:
    if os.environ.get("TUNDRA_ESSENTIA_DL", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }:
        return True
    return bundled_model_dir() is not None


def configure_tensorflow_runtime() -> None:
    os.environ.setdefault("TF_CPP_MIN_LOG_LEVEL", "3")
    os.environ.setdefault("CUDA_VISIBLE_DEVICES", "-1")


def bundled_model_dir() -> Path | None:
    candidates: list[Path] = []
    if env := os.environ.get("ESSENTIA_MODELS"):
        candidates.append(Path(env))
    script_dir = Path(__file__).resolve().parent
    candidates.extend(
        [
            script_dir.parent / "resources" / "models",
            script_dir.parent / "target" / "debug" / "models",
            script_dir.parent / "target" / "release" / "models",
        ]
    )
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


def load_audio_essentia(path: str):
    es = _import_essentia()

    loader = es.MonoLoader(filename=path, sampleRate=SAMPLE_RATE, downmix="mix")
    audio = loader()
    if audio.size == 0:
        raise ValueError("Essentia loader returned empty audio")
    max_samples = int(SAMPLE_RATE * ANALYSIS_SECONDS)
    return audio[:max_samples]


def warm() -> bool:
    """Load TensorFlow models once. Returns True when TF inference is available."""
    global _TENSORFLOW_MODELS
    if _TENSORFLOW_MODELS is not None:
        return bool(_TENSORFLOW_MODELS)
    if not dl_enabled():
        _TENSORFLOW_MODELS = {}
        return False

    configure_tensorflow_runtime()
    model_dir = bundled_model_dir()
    if model_dir is None:
        _TENSORFLOW_MODELS = {}
        return False

    try:
        es = _import_essentia()

        labels_meta = json.loads((model_dir / INSTRUMENT_LABELS).read_text(encoding="utf-8"))
        embedding_model = es.TensorflowPredictEffnetDiscogs(
            graphFilename=str(model_dir / EFFNET_MODEL),
            output="PartitionedCall:1",
        )
        instrument_model = es.TensorflowPredict2D(
            graphFilename=str(model_dir / INSTRUMENT_MODEL),
        )
        _TENSORFLOW_MODELS = {
            "classes": labels_meta["classes"],
            "embedding_model": embedding_model,
            "instrument_model": instrument_model,
        }
        return True
    except Exception as err:
        print(f"tier2: failed to warm TensorFlow models: {err}", file=sys.stderr, flush=True)
        _TENSORFLOW_MODELS = {}
        return False


def classify_with_tensorflow(path: str) -> tuple[str, float] | None:
    warm()
    if not _TENSORFLOW_MODELS:
        return None

    try:
        import numpy as np

        audio = load_audio_essentia(path)
        classes = _TENSORFLOW_MODELS["classes"]
        embeddings = _TENSORFLOW_MODELS["embedding_model"](audio)
        predictions = _TENSORFLOW_MODELS["instrument_model"](embeddings)
        scores = np.mean(predictions, axis=0)
        top_idx = int(np.argmax(scores))
        raw_class = classes[top_idx] if top_idx < len(classes) else "sampler"
        confidence = float(scores[top_idx])
        instrument = map_jamendo_class(raw_class)
        return instrument, min(0.98, max(0.55, confidence))
    except Exception as err:
        print(f"tier2: TensorFlow classifier failed: {err}", file=sys.stderr, flush=True)
        return None


def classify_with_librosa(path: str, tier1_zcr: float | None = None) -> tuple[str, float, float]:
    import librosa
    import numpy as np

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

    dl = classify_with_tensorflow(path)
    if dl is not None:
        instrument, confidence = dl
        return {
            "tier": 2,
            "decision": "definitive",
            "instrument": instrument,
            "confidence": confidence,
            "zcr": tier1_zcr,
            "engine": "tensorflow",
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
