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
  This script forces `HF_HUB_OFFLINE` so no network call can fail mid-run.
- The reference transcript is cached in a sidecar `<ref>.ref.txt` so later
  renders skip the whisper transcription pass (~40 s saved per render).
- **Long scripts are rendered in batches inside this one process** (the
  model is loaded once), each batch checkpointed to `<out>.parts.json` +
  `<out>-partNN.wav`. A crash loses at most one batch; rerunning the same
  command resumes from the manifest. Parts are concatenated into the final
  WAV only after every batch succeeded, then the part files are removed.
- **A batch longer than the model's per-segment position limit (8192
  positions ≈ 87 s) is split in half and retried automatically**, halves
  joined into the batch's part file — one over-long batch can never fail
  a render that is otherwise fine (the `size of tensor … must match`
  RuntimeError from the DiT backbone).
- **Everything is logged to `<out>.render.log`** as well as stdout, and all
  stdout/stderr writes are made dead-pipe-safe: the app that spawned us can
  exit first (its pipe then has no reader), and a 10-hour render must not
  die because nobody was listening. Failures leave the full traceback in
  the sidecar log instead of nowhere.
- `--device auto` (default) prefers the GPU: on Apple Silicon that is MPS,
  which is ~4x faster but intermittently aborts on macOS 26 — the app
  retries a failed render with `--device cpu`, the slow-but-certain path
  (68 s of audio ≈ 5 min).
- Output is 24 kHz mono 16-bit PCM WAV, what the app's audio pipeline expects.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import sys
import time
import traceback
import wave

# No network during synthesis: the model and tokenizer are cached, and a
# flapping connection must not be able to fail an hours-long render.
os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
# torch's multiprocessing children re-read PYTHONHASHSEED; without a valid
# value they die at startup with `config_init_hash_seed` — the fatal error
# seen behind the MPS aborts. Children inherit this.
os.environ.setdefault("PYTHONHASHSEED", "0")

# Bump when batch/manifest behaviour changes (invalidates old manifests).
CHUNKER_VERSION = "v2-batch"


class _SafeStream:
    """A stdout/stderr stand-in that survives a broken pipe.

    The parent app reads our output; when it exits, writes raise
    BrokenPipeError. Logging must never take a multi-hour render down with
    it, so failed writes are dropped (the sidecar log is the record).
    """

    def __init__(self, stream):
        self._stream = stream

    def write(self, data):
        try:
            return self._stream.write(data)
        except (BrokenPipeError, OSError):
            return 0

    def flush(self):
        try:
            self._stream.flush()
        except (BrokenPipeError, OSError):
            pass

    def __getattr__(self, name):
        return getattr(self._stream, name)


# Every write from this process (ours *and* F5's own prints) must survive a
# listener that exited hours ago — install the safe wrappers process-wide.
sys.stdout = _SafeStream(sys.stdout)
sys.stderr = _SafeStream(sys.stderr)


# Sidecar log path is known only after argparse; until then log() buffers
# into _EARLY which is flushed once the log path resolves.
_LOG_PATH: pathlib.Path | None = None
_EARLY: list[str] = []


def log(msg: str) -> None:
    line = f"{time.strftime('%Y-%m-%dT%H:%M:%S')} {msg}"
    if _LOG_PATH is None:
        _EARLY.append(line)
    else:
        try:
            with _LOG_PATH.open("a", encoding="utf-8") as f:
                f.write(line + "\n")
        except OSError:
            pass
    print(msg, flush=True)


def fail(msg: str) -> "None":
    log(f"RENDER_ERROR: {msg}")
    sys.exit(1)


def _init_log(out: pathlib.Path) -> None:
    global _LOG_PATH
    _LOG_PATH = out.parent / (out.name + ".render.log")
    try:
        with _LOG_PATH.open("a", encoding="utf-8") as f:
            f.write(f"--- render start {time.strftime('%Y-%m-%dT%H:%M:%S')} argv={sys.argv}\n")
            for line in _EARLY:
                f.write(line + "\n")
    except OSError:
        pass
    _EARLY.clear()


def split_batches(text: str, max_words: int) -> list[str]:
    """Pack text into batches of at most `max_words` words, cutting at
    sentence boundaries (never mid-word). Same packing rules as the app's
    own chunker: over-long sentences start a fresh batch on their own."""
    sentences: list[str] = []
    start = 0
    for i, ch in enumerate(text):
        if ch in ".!?" and (i + 1 == len(text) or text[i + 1] in " \n\t"):
            s = text[start : i + 1].strip()
            if s:
                sentences.append(s)
            start = i + 1
    if text[start:].strip():
        sentences.append(text[start:].strip())

    batches: list[str] = []
    current = ""
    words = 0
    for sentence in sentences:
        w = len(sentence.split())
        if w > max_words:
            if current:
                batches.append(current)
                current, words = "", 0
            # Hard split for pathological sentences: whole words only.
            piece = ""
            for word in sentence.split():
                pw = len((piece + " " + word).split())
                if pw > max_words:
                    batches.append(piece)
                    piece = word
                else:
                    piece = f"{piece} {word}".strip()
            if piece:
                batches.append(piece)
            continue
        if current and words + w > max_words:
            batches.append(current)
            current, words = "", 0
        current = f"{current} {sentence}".strip() if current else sentence
        words += w
    if current:
        batches.append(current)
    return batches


def parts_signature(ref: pathlib.Path, ref_text: str, text: str, speed: float, max_words: int) -> str:
    h = hashlib.sha256()
    h.update(CHUNKER_VERSION.encode())
    h.update(b"\0")
    h.update(str(ref).encode())
    h.update(b"\0")
    h.update(ref_text.encode())
    h.update(b"\0")
    h.update(text.encode())
    h.update(b"\0")
    h.update(f"{speed}:{max_words}".encode())
    return h.hexdigest()


def load_manifest(path: pathlib.Path, signature: str, total: int) -> list[str]:
    """Completed part paths (index-aligned) for a matching manifest; empty
    on any mismatch, missing file or stale signature."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    if data.get("signature") != signature or data.get("total") != total:
        return []
    parts = data.get("parts", [])
    if len(parts) != total or not all(p and pathlib.Path(p).is_file() for p in parts):
        return []
    return parts


def part_is_complete(part: pathlib.Path) -> bool:
    """True only for a fully readable WAV: a crash mid-write must not be
    mistaken for a finished batch (header claims are cheap to lie about)."""
    try:
        with wave.open(str(part), "rb") as w:
            n = w.getnframes()
            width = w.getsampwidth() * w.getnchannels()
            return n > 0 and len(w.readframes(n)) == n * width
    except (wave.Error, EOFError, OSError):
        return False


def save_manifest(path: pathlib.Path, signature: str, total: int, parts: list[str | None]) -> None:
    payload = {"signature": signature, "total": total, "parts": parts}
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, indent=1), encoding="utf-8")
    tmp.replace(path)  # atomic: a crash never leaves a half-written manifest


def render_batch(
    f5,
    ref: pathlib.Path,
    ref_text: str,
    text: str,
    speed: float,
    dest: pathlib.Path,
    log_ctx: str = "",
) -> None:
    """Render `text` into `dest`, self-healing on a model length overflow.

    The DiT backbone precomputes 8192 positions (~87 s of audio); a batch
    whose internally-chunked segment is predicted slightly longer raises
    `RuntimeError: The size of tensor a (…) must match the size of tensor b
    (8192)`. Split the text after a sentence near the middle and retry each
    half (recursively if needed), then join the halves into `dest`.
    """
    try:
        f5.infer(
            ref_file=str(ref),
            ref_text=ref_text,
            gen_text=text,
            speed=speed,
            file_wave=str(dest),
        )
        return
    except RuntimeError as e:
        if "size of tensor" not in str(e):
            raise
        words = text.split()
        if len(words) < 16:
            raise  # too small to split further — real failure
        # Prefer breaking just after a sentence end near the middle.
        mid = len(words) // 2
        cut = mid
        lookahead = max(1, len(words) // 4)
        for j in range(mid, min(len(words), mid + lookahead + 1)):
            if words[j].endswith((".", "!", "?", ";")):
                cut = j + 1
                break
        first = " ".join(words[:cut])
        second = " ".join(words[cut:])
        log(
            f"[render] {log_ctx}model length limit hit — splitting "
            f"{len(words)} words into {len(first.split())}+{len(second.split())} "
            "and retrying"
        )
        a = dest.with_name(dest.stem + ".a" + dest.suffix)
        b = dest.with_name(dest.stem + ".b" + dest.suffix)
        try:
            render_batch(f5, ref, ref_text, first, speed, a, log_ctx)
            render_batch(f5, ref, ref_text, second, speed, b, log_ctx)
            concat_parts([a, b], dest)
        finally:
            a.unlink(missing_ok=True)
            b.unlink(missing_ok=True)


def concat_parts(parts: list[pathlib.Path], out: pathlib.Path) -> None:
    """Join same-format PCM WAV batches into the final file (exact sizes)."""
    with wave.open(str(parts[0]), "rb") as first:
        params = first.getparams()
        frames = [first.readframes(first.getnframes())]
    for p in parts[1:]:
        with wave.open(str(p), "rb") as w:
            if (
                w.getsampwidth() != params.sampwidth
                or w.getframerate() != params.framerate
                or w.getnchannels() != params.nchannels
            ):
                raise RuntimeError(f"{p.name} audio format differs from the first batch")
            frames.append(w.readframes(w.getnframes()))
    tmp = out.with_suffix(".wav.tmp")
    with wave.open(str(tmp), "wb") as w:
        w.setparams(params)
        for f in frames:
            w.writeframes(f)
    tmp.replace(out)


def main() -> None:
    p = argparse.ArgumentParser(description="ResearchAI Master Voice renderer (F5-TTS)")
    p.add_argument("--ref", required=True, help="reference recording (wav/mp3/m4a/...)")
    p.add_argument("--ref-text", default="", help="known transcript of the reference")
    p.add_argument("--text-file", required=True, help="file with the text to speak")
    p.add_argument("--out", required=True, help="output wav path")
    p.add_argument("--speed", type=float, default=1.0, help="speech rate multiplier")
    p.add_argument("--batch-words", type=int, default=1200,
                   help="words per checkpointed batch (default 1200 ≈ 8 min of speech)")
    p.add_argument(
        "--device",
        default="auto",
        help="torch device: auto (GPU when available) | cpu | mps | cuda",
    )
    args = p.parse_args()

    out = pathlib.Path(args.out)
    _init_log(out)

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

    batches = split_batches(text, max(50, args.batch_words))
    signature = parts_signature(ref, ref_text, text, args.speed, args.batch_words)
    manifest_path = out.parent / (out.name + ".parts.json")
    total = len(batches)
    log(f"[render] plan {total} batch(es) (~{len(text.split())} words) -> manifest {manifest_path.name}")

    parts: list[str | None] = [None] * total
    cached = load_manifest(manifest_path, signature, total)
    if cached:
        parts = cached
        log(f"[render] resuming: {sum(1 for x in parts if x)}/{total} batch(es) already rendered")

    for i, batch in enumerate(batches):
        if parts[i]:
            continue
        # The signature is part of the name, so a stale part from a different
        # script can never be mistaken for this run's audio.
        part = out.parent / f"{out.stem}-{signature[:8]}-part{i + 1:03d}.wav"
        if part.is_file():
            if part_is_complete(part):
                parts[i] = str(part)
                save_manifest(manifest_path, signature, total, parts)
                log(f"[render] batch {i + 1}/{total} reused from disk ({part.name})")
                continue
            part.unlink(missing_ok=True)  # truncated/corrupt leftover: re-render
        log(f"[render] batch {i + 1}/{total} synthesizing ({len(batch.split())} words)")
        tb = time.time()
        try:
            render_batch(
                f5,
                ref,
                ref_text,
                batch,
                args.speed,
                part,
                log_ctx=f"batch {i + 1}/{total} ",
            )
        except Exception as e:
            log(f"[render] batch {i + 1}/{total} FAILED: {type(e).__name__}: {e}")
            fail(f"synthesis failed on batch {i + 1}/{total} ({e}) — rerun the same "
                 "command to resume from the last completed batch")
        parts[i] = str(part)
        save_manifest(manifest_path, signature, total, parts)
        log(f"[render] batch {i + 1}/{total} done in {time.time() - tb:.1f}s "
            f"(elapsed {time.time() - t0:.0f}s)")

    t3 = time.time()
    try:
        concat_parts([pathlib.Path(p) for p in parts if p], out)
    except Exception as e:
        fail(f"could not assemble the final WAV ({e}) — part files kept for resume")
    # Success: drop the checkpoint files.
    for pth in parts:
        if pth:
            pathlib.Path(pth).unlink(missing_ok=True)
    manifest_path.unlink(missing_ok=True)

    if not out.is_file() or out.stat().st_size < 1024:
        fail(f"renderer produced no usable audio at {out}")
    dur = out.stat().st_size / (24_000 * 2)
    log(f"[render] concat {time.time() - t3:.1f}s -> {dur:.1f}s of audio")
    log(f"[render] done {time.time() - t0:.1f}s total")


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except BaseException:
        # The last resort: keep the traceback where a dead stdout cannot eat it.
        log("RENDER_ERROR: unhandled exception:\n" + traceback.format_exc())
        sys.exit(1)
