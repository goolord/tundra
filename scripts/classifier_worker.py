#!/usr/bin/env python3
"""Persistent tier-2 classifier worker. Reads JSON lines on stdin, writes JSON on stdout."""

from __future__ import annotations

import json
import sys
from typing import Any

import tier2_lib


def handle_request(payload: dict[str, Any]) -> dict[str, Any]:
    if payload.get("quit"):
        raise SystemExit(0)
    path = payload.get("path")
    if not isinstance(path, str) or not path:
        raise ValueError("missing path")
    tier1_zcr = payload.get("tier1_zcr")
    if tier1_zcr is not None and not isinstance(tier1_zcr, (int, float)):
        raise ValueError("invalid tier1_zcr")
    return tier2_lib.classify_path(path, tier1_zcr=float(tier1_zcr) if tier1_zcr is not None else None)


def main() -> None:
    try:
        onnx_ready = tier2_lib.warm()
        print(json.dumps({"ready": True, "onnx": onnx_ready}), flush=True)
    except Exception as err:
        print(json.dumps({"ready": False, "error": str(err)}), flush=True)
        sys.exit(1)

    try:
        for line in sys.stdin:
            raw = line.strip()
            if not raw:
                continue
            try:
                payload = json.loads(raw)
                result = handle_request(payload)
                print(json.dumps({"ok": True, "result": result}), flush=True)
            except SystemExit:
                return
            except Exception as err:
                print(json.dumps({"ok": False, "error": str(err)}), flush=True)
    except (KeyboardInterrupt, BrokenPipeError):
        pass


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
