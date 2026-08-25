#!/usr/bin/env python3
"""Tier 2 classifier CLI — delegates to tier2_lib."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import tier2_lib


def main() -> None:
    if len(sys.argv) != 2:
        print(json.dumps({"error": "usage: tier2_essentia.py <audio-file>"}), file=sys.stderr)
        sys.exit(2)

    path = sys.argv[1]
    if not Path(path).is_file():
        print(json.dumps({"error": f"file not found: {path}"}), file=sys.stderr)
        sys.exit(1)

    try:
        print(json.dumps(tier2_lib.classify_path(path)))
    except Exception as err:
        print(json.dumps({"error": str(err)}), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
