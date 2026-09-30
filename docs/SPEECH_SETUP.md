# Speech setup (STT / TTS)

ResearchAI Workspace does **not** bundle speech engines or models. It drives
user-installed, offline tooling as one-shot subprocesses — this guide gets a
real installation working, verified with the same CLI invocations the app
itself uses.

| Capability | Tool | Used for |
| --- | --- | --- |
| Text → speech | [Piper](https://github.com/rhasspy/piper) (default) or macOS `say` | Read-aloud, summaries, podcast narration |
| Speech → text | [whisper.cpp](https://github.com/ggml-org/whisper.cpp) `whisper-cli` | Lecture/audio transcription |

## 1. Install the engines

macOS (Homebrew):

```sh
brew install piper whisper-cpp ffmpeg   # ffmpeg is optional but recommended
```

Notes:

- If your Homebrew formula does not ship a `piper` binary, `pip install piper-tts`
  also provides one (`~/.local/bin/piper` with `pip install --user`).
- Building whisper.cpp from source also works; make sure the `whisper-cli`
  (or legacy `main`) binary is built (`cmake -B build && cmake --build build`).
- The Linux/Windows equivalents are the same binaries: any `piper` and
  `whisper-cli` on your disk work; only the paths differ.

## 2. Download the models (you own them; nothing is auto-downloaded)

- **Piper voices**: one `.onnx` file **plus** its `.onnx.json` config, from the
  `rhasspy/piper-voices` collection on Hugging Face. Keep the pair side by side:

  ```sh
  mkdir -p ~/researchai-models/piper
  cd ~/researchai-models/piper
  curl -LO https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx
  curl -LO https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/high/en_US-lessac-high.onnx.json
  ```

- **Whisper GGML models**: from the `ggml-org/whisper.cpp` collection on
  Hugging Face (`ggml-base.en.bin` is a good default; `ggml-small.bin` trades
  speed for accuracy):

  ```sh
  mkdir -p ~/researchai-models/whisper
  cd ~/researchai-models/whisper
  curl -LO https://huggingface.co/ggml-org/whisper.cpp/resolve/main/ggml-base.en.bin
  ```

## 3. Verify before opening the app

The checker runs the **same commands the app runs** and validates the outputs
with the same strictness as the Rust core:

```sh
scripts/dev/check-speech-setup.sh \
  --piper "$(command -v piper)" \
  --voice ~/researchai-models/piper/en_US-lessac-high.onnx \
  --whisper "$(command -v whisper-cli)" \
  --model ~/researchai-models/whisper/ggml-base.en.bin
```

A green run prints `[ok] piper rendered a valid WAV (…)` and
`[ok] whisper-cli transcribed and emitted valid JSON (…)`. Useful variants:

- `--skip-tts` / `--skip-stt` — check just one direction.
- `--out DIR --keep` — keep the probe files (listen to `tts-check.wav`).
- `WHISPER_LANG=en` (or `--` env) — pin the language instead of auto-detect.
- TTS-only quick start with no Piper yet: macOS has a built-in fallback —
  pick provider **macos-say** in Settings → Speech and skip Piper entirely.

Exit code is `0` only when every requested check passed, so it can gate
scripts/CI.

## 4. Point the app at the install

Settings → Speech:

- **TTS**: provider `piper`, binary path (e.g. `/opt/homebrew/bin/piper`),
  voice model path (the `.onnx`), speed (0.5–2.0), optional MP3 export
  (needs ffmpeg).
- **STT**: `whisper-cli` path, GGML model path, language (`auto` or an ISO
  code), "Convert with ffmpeg" enabled for non-WAV inputs.

Then in the **Audio** tab: Read aloud works in No-AI mode; summaries and the
podcast segment additionally need a local model loaded (Settings → Local AI).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| `piper produced no audio` | Run the piper command from the checker output by hand; the stderr there is the real cause (most often a missing `.onnx.json` beside the voice). |
| `whisper-cli produced no JSON` | The binary was built without JSON support (`-oj`); rebuild whisper.cpp or point the app at a `whisper-cli` build. |
| `not a readable PCM WAV` | Another tool wrote a non-WAV file at the output path; check the `--output_file` argument for typos. |
| WAV rejected by STT | whisper.cpp needs 16 kHz mono PCM; enable "Convert with ffmpeg" in Settings → Speech or pre-convert: `ffmpeg -i in.mp3 -ar 16000 -ac 1 -c:a pcm_s16le out.wav`. |
| Paths valid in the shell but not the app | The GUI app does not inherit your shell profile; enter absolute paths in Settings. |

## What the checker does *not* cover

It validates the engine layer (the same subprocess invocations and output
formats the app accepts). It does not drive the UI itself, and it does not
benchmark speed or voice quality.
