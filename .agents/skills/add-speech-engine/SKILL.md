---
name: add-speech-engine
description: Use this skill to add a new text-to-speech engine/provider to ResearchAI Workspace. It walks through the provider trait, chunked-render integration, settings wiring, fake-binary test patterns, and cross-platform CI pitfalls. Read it fully before touching tts.rs.
---

# Add Speech Engine

## Goal

Integrate a new TTS provider into the desktop app end-to-end: Rust
provider implementation, chunked narration support, settings/UI wiring,
and portable fake-binary tests that hold on macOS, Windows and Ubuntu CI.

Adapted from Voicebox's `add-tts-engine` skill
(github.com/jamiepine/voicebox, MIT) for ResearchAI's Rust-side provider
architecture (Voicebox's engines live in a Python backend; ours do not).

## Read First

`apps/desktop/src-tauri/src/services/tts.rs` (~1500 lines) is the whole
subsystem. Understand these pieces before writing code:

- `TextToSpeechProvider` — the public trait: `render`, `check`
  (→ `TtsStatus`), `is_chunk_error`.
- `ChunkedRenderInternal` — the object-safe view `run_chunked` needs:
  `preflight`, `provider_id`, `render_internal`, `set_chunk_failed`,
  `mp3_enabled`, `out_dir`.
- `run_chunked` — splits text via `chunk_text` (≤ 220 words/sentence-
  packed), renders each chunk with `render_chunk_with_retry`, caches
  parts in `tts-parts.json` (signature = provider id + script) so failed
  runs resume, then `concat_wavs` joins parts into one exact-size RIFF.
- `provider_for(settings, out_dir)` — the dispatch point (~line 1090).
  Currently: `"macos-say"` → `MacOsSayProvider` (cfg-gated), `_` →
  `PiperProvider`.

## Phases

### Phase 1 — Provider implementation

1. Create `struct YourProvider` with a `chunked_failure: Arc<AtomicBool>`
   field (derived `Clone` must share the flag) and an overridable
   `binary: PathBuf` for tests.
2. Implement `ChunkedRenderInternal`: deterministic `preflight` (missing
   tool = fail fast, no retries), a stable `provider_id` (feeds the parts
   cache signature — include voice/model identity), and
   `render_internal` writing one WAV per chunk via a **one-shot
   subprocess** (never a long-lived process: the engine-supervisor lesson
   is that un-reaped children lie about being dead).
3. Implement `TextToSpeechProvider::render` as `run_chunked(self, ...)`;
   `check` reports `binary_found`/`model_found`/`ready`.

### Phase 2 — Dispatch & settings

- Add the settings string in `crate::db::TtsSettings` and match it in
  `provider_for`. Non-matching platforms must return a clear `AppError`
  (see the `macos-say` non-macOS arm), not panic.
- Surface the provider in `commands/tts.rs`, `backend/client.ts`,
  `SettingsView.tsx` (provider picker + status), and `AudioView.tsx` if
  narration UI needs a branch.

### Phase 3 — Long-text behaviour

- Chunking is automatic once `render` delegates to `run_chunked`.
- `chunk_text` packs sentences to `MAX_CHUNK_WORDS` (220) and never cuts
  mid-sentence; a pathological run without sentence ends is one
  over-length chunk. If your engine has a hard input limit, add a
  clause-boundary fallback rather than splitting mid-word.
- Voicebox's `backend/utils/chunked_tts.py` is the reference for
  abbreviation-aware splitting (`.e.g`, `Dr.`, decimals) and 50 ms
  crossfade joins — our `concat_wavs` is a byte-exact splice (piper/say
  output starts and ends with silence, so clicks are not an issue there;
  evaluate before assuming the same for a new engine).

### Phase 4 — Tests (the portability minefield)

- Tests live in `mod tests` gated `#[cfg(all(test, unix))]` — Windows
  cannot exec `#!` fake scripts (os error 193).
- **Generate fake-engine WAV bytes in Rust, never in shell.** dash's
  `printf` has no `\xHH`, which once produced garbage WAVs on Ubuntu CI.
  Pattern: `fake_piper_wav_bytes()` builds the RIFF in Rust, the fake
  script is `cat` onto stdout or into the output path.
- If your provider does loopback HTTP, read the full request via
  Content-Length, flush, `shutdown(Write)`, then drain — Windows resets
  the socket otherwise (os error 10053).
- PathBuf is not `Display`: shadow `let src = src.display().to_string();`
  before passing to shell-format strings (3 prior sites in tts.rs).
- Test matrix for new tests: `cargo test` locally, then confirm all of
  macOS-14 / windows / ubuntu jobs on the CI run (job-level status, not
  run-level — macos-13 Intel is perpetually queued on free runners).

### Phase 5 — Bundling & docs

- Engines with external binaries: document install in
  `docs/SPEECH_SETUP.md` and wire the setup-checklist status probe.
- Update `README.md` spoken-features line if user-visible.

## Key Lessons (from Voicebox's v0.2.3, mapped to our risks)

| Pattern | Symptom | Fix here |
|---|---|---|
| Skipping dependency research | release broken after tag | Phase 0: run the binary yourself before wiring |
| Long-lived subprocesses | zombie passes `kill -0` | one-shot per chunk; kill AND reap on drop (engine_runtime.rs lesson) |
| Shell-generated fixtures | green local / red CI | Rust-built fixtures, `#[cfg(unix)]` gates |
| Hardcoding RIFF header fields | silent audio corruption | parts format-compare before concat (`concat_wavs` refuses mismatched formats) |
| Untested Windows path | first Windows CI red | `serve_once` drain pattern + cfg gates |

## Notes

- Do NOT push or create a release. Hand the build to the user for local
  testing (pair with `release-bump` only after user sign-off).
- Keep `tts-parts.json` signatures honest: any provider-identity change
  must change `provider_id` or stale cached parts will be reused.
