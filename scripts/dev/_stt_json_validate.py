#!/usr/bin/env python3
"""whisper -oj JSON validator for the speech setup checker.

Parses exactly the schema the Rust core consumes (transcription.rs
`parse_whisper_json`): a top-level object with a `transcription` array of
`{offsets: {from, to}, text}` entries and an optional `result.language`.
Prints a compact summary on success; exits 1 with a diagnostic otherwise.
"""
from __future__ import annotations

import json
import sys


def validate_json(path: str) -> str:
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        doc = json.load(f)
    if not isinstance(doc, dict):
        raise ValueError("top level is not an object")
    segs = doc.get("transcription")
    if not isinstance(segs, list):
        raise ValueError("missing transcription array (was -oj used?)")
    for i, seg in enumerate(segs):
        if not isinstance(seg, dict):
            raise ValueError(f"segment {i} is not an object")
        offs = seg.get("offsets")
        if not isinstance(offs, dict) or "from" not in offs or "to" not in offs:
            raise ValueError(f"segment {i} has no offsets.from/to")
        if not isinstance(offs["from"], int) or not isinstance(offs["to"], int):
            raise ValueError(f"segment {i} offsets are not integers")
        if offs["to"] < offs["from"]:
            raise ValueError(f"segment {i} ends before it starts")
        if not isinstance(seg.get("text", ""), str):
            raise ValueError(f"segment {i} text is not a string")
    lang = (doc.get("result") or {}).get("language")
    n = len(segs)
    return f"{n} segment(s), language={lang or 'unknown'}"


if __name__ == "__main__":
    try:
        print(validate_json(sys.argv[1]))
    except (IndexError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(exc, file=sys.stderr)
        sys.exit(1)
