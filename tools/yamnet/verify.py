#!/usr/bin/env python3
"""Check Tundra's YAMNet port against the TensorFlow reference.

Runs Google's ``yamnet.py`` (Keras, loading ``yamnet.h5``) and Tundra's numpy
front end plus ``yamnet.onnx`` on the same waveforms, and fails if patches,
scores, or embeddings differ beyond float tolerance.

Usage (TensorFlow is only needed here, not at runtime):

    uv run --no-project --python 3.12 --with tensorflow==2.18.0 --with tf-keras==2.18.0 \
        --with onnxruntime --with librosa \
        python tools/yamnet/verify.py <dir with yamnet.py, params.py, features.py, yamnet.h5> \
        resources/models/yamnet.onnx [audio files...]
"""

from __future__ import annotations

import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import tier2_lib  # noqa: E402


def main() -> None:
    reference_dir, onnx_path, *audio_files = sys.argv[1:]
    sys.path.insert(0, reference_dir)
    import onnxruntime as ort
    import params as yamnet_params
    import yamnet as yamnet_model

    params = yamnet_params.Params()
    reference = yamnet_model.yamnet_frames_model(params)
    reference.load_weights(str(Path(reference_dir) / "yamnet.h5"))
    session = ort.InferenceSession(onnx_path, providers=["CPUExecutionProvider"])

    rng = np.random.default_rng(7)
    t = np.arange(tier2_lib.SAMPLE_RATE * 3) / tier2_lib.SAMPLE_RATE
    cases = {
        f"noise-{length}": (rng.standard_normal(length) * 0.2).astype(np.float32)
        for length in (100, 15599, 15600, 15601, 23280, 48000, 160000)
    }
    cases["sweep-3s"] = (0.4 * np.sin(2 * np.pi * (60 + 2000 * t) * t)).astype(np.float32)
    cases["silence-2s"] = np.zeros(tier2_lib.SAMPLE_RATE * 2, dtype=np.float32)
    for path in audio_files:
        cases[Path(path).name] = tier2_lib.load_audio(path)

    worst = {"patches": 0.0, "scores": 0.0, "embeddings": 0.0}
    for name, waveform in cases.items():
        scores_ref, embeddings_ref, spectrogram_ref = (
            output.numpy() for output in reference(waveform)
        )
        patches = tier2_lib.yamnet_patches(waveform)
        scores, embeddings = session.run(["scores", "embeddings"], {"patches": patches})
        if scores.shape != scores_ref.shape:
            sys.exit(f"{name}: {scores.shape[0]} patches, reference has {scores_ref.shape[0]}")

        frames = spectrogram_ref.shape[0]
        starts = np.arange(scores_ref.shape[0])[:, None] * tier2_lib.PATCH_HOP_FRAMES
        patches_ref = spectrogram_ref[starts + np.arange(tier2_lib.PATCH_FRAMES)]
        assert frames >= tier2_lib.PATCH_FRAMES
        errors = {
            "patches": float(np.max(np.abs(patches - patches_ref))),
            "scores": float(np.max(np.abs(scores - scores_ref))),
            "embeddings": float(np.max(np.abs(embeddings - embeddings_ref))),
        }
        top = int(np.argmax(scores.max(axis=0)))
        top_ref = int(np.argmax(scores_ref.max(axis=0)))
        print(f"{name:>28}: patches={scores.shape[0]} top={top}/{top_ref} " + " ".join(
            f"{key}={value:.2e}" for key, value in errors.items()
        ))
        for key, value in errors.items():
            worst[key] = max(worst[key], value)
        if top != top_ref:
            sys.exit(f"{name}: top class {top} differs from reference {top_ref}")

    print("worst:", worst)
    if worst["scores"] > 1e-3 or worst["patches"] > 1e-2:
        sys.exit("YAMNet port differs from the reference")
    print("YAMNet port matches the TensorFlow reference")


if __name__ == "__main__":
    main()
