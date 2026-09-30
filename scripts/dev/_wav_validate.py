#!/usr/bin/env python3
"""Strict WAV validator for the speech setup checker.

Mirrors what the Rust core accepts: a RIFF/WAVE container with a readable
`fmt ` chunk (integer PCM or IEEE float) and a `data` chunk whose payload is
present, frame-aligned, and declared non-zero — i.e. a file the app's
WAV-walking code can parse deterministically.

Prints a compact summary on success ("0:01.02, 22050 Hz, 1ch, 16-bit PCM").
Exits 1 with a diagnostic on anything unplayable.
"""
from __future__ import annotations

import struct
import sys


def validate_wav(path: str) -> str:
    with open(path, "rb") as f:
        raw = f.read()

    if len(raw) < 12:
        raise ValueError("file is too small to be WAV")
    if raw[0:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise ValueError("missing RIFF/WAVE header")

    fmt = None
    data_len = None
    pos = 12
    # Chunks are word-aligned in RIFF.
    while pos + 8 <= len(raw):
        cid = raw[pos : pos + 4]
        (size,) = struct.unpack_from("<I", raw, pos + 4)
        body = pos + 8
        if cid == b"fmt " and size >= 16:
            (
                audio_format,
                channels,
                rate,
                _byte_rate,
                _align,
                bits,
            ) = struct.unpack_from("<HHIIHH", raw, body)
            if audio_format not in (1, 3):
                raise ValueError(
                    f"format tag {audio_format} is neither PCM nor IEEE float"
                )
            if channels == 0 or rate == 0 or bits == 0:
                raise ValueError("fmt chunk has zeroed channels/rate/bits")
            fmt = (audio_format, channels, rate, bits)
        elif cid == b"data":
            data_len = size
            if size == 0:
                raise ValueError("data chunk is empty")
            if body + size > len(raw):
                raise ValueError("data chunk runs past the end of the file")
        pos = body + size + (size & 1)

    if fmt is None:
        raise ValueError("no fmt chunk found")
    if data_len is None:
        raise ValueError("no data chunk found")

    audio_format, channels, rate, bits = fmt
    frame = channels * (bits // 8)
    if frame == 0:
        raise ValueError("impossible frame size")
    if data_len % frame != 0:
        raise ValueError("data payload is not frame-aligned")
    ms = data_len * 1000 // (rate * frame)
    kind = "PCM" if audio_format == 1 else "float"
    return f"{ms // 60000}:{(ms % 60000) / 1000:05.2f}, {rate} Hz, {channels}ch, {bits}-bit {kind}"


if __name__ == "__main__":
    try:
        print(validate_wav(sys.argv[1]))
    except (IndexError, OSError, ValueError) as exc:
        print(exc, file=sys.stderr)
        sys.exit(1)
