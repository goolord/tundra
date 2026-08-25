#!/usr/bin/env python3
"""Tier 1 classifier: Librosa zero-crossing rate (microseconds-scale)."""

from __future__ import annotations

import json
import sys

# Decisive bands only; grey area goes to tier 2.
VERY_LOW_THRESHOLD = 0.03
LOW_THRESHOLD = 0.04
HIGH_THRESHOLD = 0.20
VERY_HIGH_THRESHOLD = 0.28
ANALYSIS_SECONDS = 30.0
SAMPLE_RATE = 22050


def classify_zcr(zcr: float) -> dict:
    if zcr <= VERY_LOW_THRESHOLD:
        confidence = min(0.95, 0.74 + (VERY_LOW_THRESHOLD - zcr) * 5.0)
        return {
            "tier": 1,
            "decision": "definitive",
            "instrument": "Kick",
            "zcr": zcr,
            "confidence": confidence,
        }
    if zcr <= LOW_THRESHOLD:
        confidence = min(0.92, 0.70 + (LOW_THRESHOLD - zcr) * 4.0)
        return {
            "tier": 1,
            "decision": "definitive",
            "instrument": "Bass",
            "zcr": zcr,
            "confidence": confidence,
        }
    if zcr >= VERY_HIGH_THRESHOLD:
        confidence = min(0.95, 0.74 + (zcr - VERY_HIGH_THRESHOLD) * 2.0)
        return {
            "tier": 1,
            "decision": "definitive",
            "instrument": "Cymbal",
            "zcr": zcr,
            "confidence": confidence,
        }
    if zcr >= HIGH_THRESHOLD:
        confidence = min(0.92, 0.70 + (zcr - HIGH_THRESHOLD) * 2.5)
        return {
            "tier": 1,
            "decision": "definitive",
            "instrument": "Hi-Hat",
            "zcr": zcr,
            "confidence": confidence,
        }
    return {"tier": 1, "decision": "grey", "zcr": zcr}


def main() -> None:
    if len(sys.argv) != 2:
        print("usage: tier1_zcr.py <audio-file>", file=sys.stderr)
        sys.exit(2)

    path = sys.argv[1]
    try:
        import librosa
        import numpy as np
    except ImportError as err:
        print(
            json.dumps({"error": f"librosa is required for tier 1: {err}"}),
            file=sys.stderr,
        )
        sys.exit(1)

    audio, _sample_rate = librosa.load(
        path,
        sr=SAMPLE_RATE,
        mono=True,
        duration=ANALYSIS_SECONDS,
    )
    zcr = float(np.mean(librosa.feature.zero_crossing_rate(audio)[0]))
    print(json.dumps(classify_zcr(zcr)))


if __name__ == "__main__":
    main()
