#!/usr/bin/env python3
"""Persistent tier-2 classifier worker.

Reads one JSON request per line on stdin and writes one JSON reply per line,
echoing the request id. Only replies go to the real stdout: anything else a
library prints is redirected to stderr so it cannot be mistaken for a reply.
"""

from __future__ import annotations

import io
import json
import os
import sys
from typing import Any, TextIO


def _protocol_streams() -> tuple[TextIO, TextIO]:
    """Claim fd 1 for replies and point everything else at stderr."""
    replies = io.TextIOWrapper(
        os.fdopen(os.dup(sys.stdout.fileno()), "wb"), encoding="utf-8", newline="\n"
    )
    os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
    sys.stdout = sys.stderr
    requests = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")
    return requests, replies


def _send(stream: TextIO, message: dict[str, Any]) -> None:
    stream.write(json.dumps(message) + "\n")
    stream.flush()


def handle_request(payload: dict[str, Any]) -> dict[str, Any]:
    import tier2_lib

    path = payload.get("path")
    if not isinstance(path, str) or not path:
        raise ValueError("missing path")
    tier1_zcr = payload.get("tier1_zcr")
    if tier1_zcr is not None and not isinstance(tier1_zcr, (int, float)):
        raise ValueError("invalid tier1_zcr")
    return tier2_lib.classify_path(
        path, tier1_zcr=float(tier1_zcr) if tier1_zcr is not None else None
    )


def main() -> None:
    requests, replies = _protocol_streams()
    try:
        import tier2_lib

        onnx_ready = tier2_lib.warm()
        _send(replies, {"ready": True, "onnx": onnx_ready})
    except Exception as err:
        _send(replies, {"ready": False, "error": str(err)})
        sys.exit(1)

    for line in requests:
        raw = line.strip()
        if not raw:
            continue
        request_id = None
        try:
            payload = json.loads(raw)
            if payload.get("quit"):
                return
            request_id = payload.get("id")
            result = handle_request(payload)
            _send(replies, {"id": request_id, "ok": True, "result": result})
        except Exception as err:
            _send(replies, {"id": request_id, "ok": False, "error": str(err)})


if __name__ == "__main__":
    try:
        main()
    except (KeyboardInterrupt, BrokenPipeError):
        pass
