# /// script
# requires-python = ">=3.11"
# dependencies = ["f5-tts"]
# ///
"""One-shot Master Voice renderer: clone a reference recording and speak text.

Run with `uv run render.py --ref <recording> --text-file <file> --out <wav>`
(Python and all dependencies come from the inline metadata above — no
system Python setup is required).

Design notes:
- F5-TTS runs entirely offline once the model is cached; the first run
  downloads ~1.3 GB to the Hugging Face cache (~/.cache/huggingface).
- The reference transcript is cached in a sidecar `<ref>.ref.txt` so later
  renders skip the whisper transcription pass (~40 s saved per render).
- `--device auto` (default) prefers the GPU: on Apple Silicon that is MPS,
  which is ~4x faster but intermittently aborts with a command-buffer
  assertion on macOS 26 — the app retries a failed render with
  `--device cpu`, the slow-but-certain path (68 s of audio ≈ 5 min).
- Output is 24 kHz mono 16-bit PCM WAV, what the app's audio pipeline expects.
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time


def log(msg: str) -> None:
    print(msg, flush=True)


def fail(msg: str) -> "None":
    print(f"RENDER_ERROR: {msg}", flush=True)
    sys.exit(1)


def main() -> None:
    p = argparse.ArgumentParser(description="ResearchAI Master Voice renderer (F5-TTS)")
    p.add_argument("--ref", required=True, help="reference recording (wav/mp3/m4a/...)")
    p.add_argument("--ref-text", default="", help="known transcript of the reference")
    p.add_argument("--text-file", required=True, help="file with the text to speak")
    p.add_argument("--out", required=True, help="output wav path")
    p.add_argument("--speed", type=float, default=1.0, help="speech rate multiplier")
    p.add_argument(
        "--device",
        default="auto",
        help="torch device: auto (GPU when available) | cpu | mps | cuda",
    )
    args = p.parse_args()

    if args.device == "auto":
        try:
            import torch

            args.device = "mps" if torch.backends.mps.is_available() else "cpu"
        except Exception:
            args.device = "cpu"
        log(f"[render] device {args.device}")

    ref = pathlib.Path(args.ref)
    if not ref.is_file():
        fail(f"reference recording not found: {ref}")
    text_path = pathlib.Path(args.text_file)
    if not text_path.is_file():
        fail(f"text file not found: {text_path}")
    text = text_path.read_text(encoding="utf-8").strip()
    if not text:
        fail("nothing to speak: the text file is empty")
    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)

    # Cached transcript sidecar next to the reference (best effort — the
    # reference may live in a read-only location).
    sidecar = ref.with_name(ref.name + ".ref.txt")
    ref_text = args.ref_text.strip()
    if not ref_text and sidecar.is_file():
        try:
            ref_text = sidecar.read_text(encoding="utf-8").strip()
        except OSError:
            ref_text = ""

    t0 = time.time()
    try:
        from f5_tts.api import F5TTS
    except Exception as e:  # ImportError or a broken dependency
        fail(f"could not import f5-tts ({e}) — check network access so uv can install it")
    log(f"[render] import {time.time() - t0:.1f}s")

    t1 = time.time()
    try:
        f5 = F5TTS(device=args.device)
    except Exception as e:
        fail(f"could not load the F5-TTS model ({e}) — the first run downloads it "
             "from Hugging Face and needs network access")
    log(f"[render] model {time.time() - t1:.1f}s")

    # Resolve the reference transcript once, then cache it beside the ref so
    # future renders skip whisper entirely.
    if not ref_text:
        t2 = time.time()
        try:
            from f5_tts.infer.utils_infer import preprocess_ref_audio_text

            _, ref_text = preprocess_ref_audio_text(str(ref), "")
        except Exception as e:
            fail(f"could not transcribe the reference recording ({e}) — "
                 "pass --ref-text to skip automatic transcription")
        log(f"[render] ref transcript {time.time() - t2:.1f}s")
        try:
            sidecar.write_text(ref_text, encoding="utf-8")
            log(f"[render] cached transcript -> {sidecar}")
        except OSError:
            log(f"[render] transcript not cached (read-only location): {sidecar}")

    t3 = time.time()
    try:
        wav, sr, spec = f5.infer(
            ref_file=str(ref),
            ref_text=ref_text,
            gen_text=text,
            speed=args.speed,
            file_wave=str(out),
        )
    except Exception as e:
        fail(f"synthesis failed ({e})")
    dur = len(wav) / sr if hasattr(wav, "__len__") and sr else 0.0
    log(f"[render] synth {time.time() - t3:.1f}s -> {dur:.1f}s of audio")
    if not out.is_file() or out.stat().st_size < 1024:
        fail(f"renderer produced no usable audio at {out}")
    log(f"[render] done {time.time() - t0:.1f}s total")


if __name__ == "__main__":
    main()
