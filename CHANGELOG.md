# Changelog

All notable changes to ResearchAI Workspace are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The `[Unreleased]` section is a non-destructive working draft — update it any
time during development (see `.agents/skills/draft-release-notes/`). Release
cuts stamp it with a version and date (see `.agents/skills/release-bump/`),
and the stamped section becomes the GitHub Release body.

## [Unreleased]

### Fixed

- **In-app audio playback works**: renders succeeded but the built-in
  player only showed "Error" (MEDIA_ERR_SRC_NOT_SUPPORTED on every load).
  `index.html` shipped a second, older CSP as a `<meta>` tag with no
  `media-src` directive — CSP policies are conjunctive, so its
  `default-src 'self'` blocked all `asset://` and `blob:` media even
  though the authoritative header CSP from `tauri.conf.json` allowed it.
  The duplicate meta CSP is gone (single source of truth:
  `app.security.csp`), playback failures now land in the app log with the
  MediaError code (plus an actionable toast instead of a silent "Error"),
  and a startup self-check records the page origin, the enforced CSP, any
  duplicate meta CSP, and whether a narration file loads.
- **Audio generation works out of the box on macOS**: the "Render audio"
  button was dead for anyone who had not installed Piper — and the
  unfiltered file pickers in Settings → Speech made it easy to save a
  recording *as* the `piper`/`whisper-cli` binary, which silently left the
  voice "not ready". Speech settings now reject audio files, folders and
  non-executable paths at save time with actionable copy, and when Piper is
  selected but unusable the built-in macOS `say` voice takes over (status
  and both render commands share the same provider resolution, so what the
  UI reports as ready is exactly what renders). The Audio tab explains how
  to enable a voice when none is ready.

## [0.1.4] - 2026-10-04

### Added

- **AI that works out of the box**: the installer now ships the whole
  local AI stack — the llama.cpp `llama-server` runtime (pinned per-OS
  CPU builds), a ready-to-run **Qwen3-0.6B Q4_K_M** starter model
  (Apache-2.0, ~397 MB), and the **bge-small** embedding model. First
  launch seeds the model into the library, activates it and enables AI
  with zero setup; semantic search runs offline with no download. A
  user-configured `llama-server` path or model in Settings still takes
  precedence over the bundled runtime.
- **Persistent embedding cache**: the embedding model now lives under
  the app's data folder instead of `$TMPDIR`, which macOS periodically
  wipes — that silently forced re-downloads and dropped search to the
  hashing fallback while offline.
- Build pipeline: `scripts/package/fetch-ai-assets.mjs` and
  `packaging/seed_embeddings.py` fetch, pin and permission-normalise the
  bundled assets; `release.yml` runs both before every installer build,
  and `THIRD_PARTY_LICENSES.md` gains a bundled-assets section.

### Fixed

- **Ask prompts fit the model's context window**: a typical 12-excerpt
  ask could exceed the 4096-token window, so llama-server rejected the
  entire request ("request exceeds the available context size") and
  Ask-AI failed on its first use. The prompt is now budgeted to the
  configured window — excerpts are trimmed with a visible warning
  instead of an error — and the default window is 8192 tokens.

## [0.1.3] - 2026-10-03

Local acceptance testing of the v0.1.2 DMG caught a release-blocking
packaging bug: installed copies shipped the frozen engine without its
Python runtime, so the document engine could never start. This round
fixes the bundle, makes sidecar failures visible in the log, and keeps
`/health` honest about which engine version is running.

### Fixed

- **Packaged engine runtime**: release builds shipped the PyInstaller
  entry binary without its sibling `_internal/` directory (Python dylib,
  stdlib, libs), so the bundled document engine died at exec on every
  installed copy ("Failed to load Python shared library …/_internal/Python")
  and no installed app could parse anything. `collect-sidecar.mjs` now
  copies the runtime beside the binary. The bug was masked during
  testing by an externally started dev engine answering on the same port.
- The bundle config mapped the sidecar with a non-recursive
  `sidecar/*` glob (Tauri ignores sub-directories in `dir/*`), so even a
  correctly collected `_internal/` never reached the app; it now maps the
  directory itself (`packaging/resources/sidecar/` → `sidecar/`), which
  copies recursively.
- The engine supervisor now logs an instant sidecar death and a
  health-wait timeout instead of failing silently — a broken sidecar is
  diagnosable from `researchai-*.log` instead of an empty one.
- `/health` reports the real engine version from package metadata (the
  dist-info is bundled into the freeze) instead of a hardcoded 0.1.0
  that drifted from every release.
- Generated sidecar/PyInstaller artefacts (`_internal/`, collected
  binaries, `build/`) are git-ignored so the 138 MB runtime can never be
  committed.

## [0.1.2] - 2026-10-03

Second post-release round: the import path is fixed end-to-end — folder
selection, drag-and-drop, and the Office/EPUB formats the app advertised
but the engine could not yet parse.

### Added

- **Folder import**: the file picker's folder option now walks the selected
  directory recursively (depth-capped, symlink-safe) and imports every
  supported document — mixed selections of files and folders work in one
  batch. Unsupported files are counted in the import summary
  (`skipped`) instead of failing the run.
- **PPTX, XLSX and EPUB parsing**: the document engine registers parsers
  for the three formats the app already advertised — slides become
  page-numbered sections, worksheets become sheet sections with
  table blocks, EPUB chapters follow spine order with rebased offsets.
- **Drag-and-drop import**: dropping documents onto the window imports
  them into the active project (with a clear toast if none is selected),
  and the document picker filters to supported extensions up front.
- **Title-bar search**: the header search box is live — Enter seeds the
  Search view and runs the query.
- Release bodies now come from the stamped `CHANGELOG.md` section:
  `release.yml` extracts it via `scripts/release/extract-notes.sh`
  (shared with the dry-run's new fail-fast rehearsal step). A missing
  section falls back to a generic body — a release can no longer ship
  with empty notes the way v0.1.1 did before backfill.

### Fixed

- Selecting a **folder** to import no longer fails with "Not a regular
  file" — directories are expanded before validation, and an empty
  selection reports which formats are supported.
- Importing pptx/xlsx/epub (and wav/mp3/m4a/mp4/mov/png/jpg, which are
  media rather than documents) no longer reaches parse time only to be
  rejected; media belongs to the Audio/reading workflows and the picker,
  drop handler and folder walker all agree on the real allow-list.
- Deleting a project now purges its managed document copies under
  `documents/<project_id>/` instead of orphaning them on disk (link
  originals were always untouched).
- Stale "arrives in a later phase" copy removed from the Library, Notes
  and Home views; the reader modal now states what this release does.

## [0.1.1] - 2026-10-03

First post-release hardening round: the desktop app got faster, more
resilient asks and live feedback for long-running work, while CI learned to
run the full test battery on Windows and Ubuntu — not just macOS.

### Performance

- Analysis asks now emit evidence-first, question-last prompts, so the
  bundled llama-server reuses its prefix KV cache across regenerate,
  follow-up questions and retries; the server spawns with
  `--cache-reuse 256` and per-ask telemetry (tokens, ms/token) is logged
  for verification ([AI.md](docs/AI.md)).

### Added

- **Streaming asks**: answers stream into the Research view as
  `ai://ask-delta` deltas while generating; nothing is persisted until the
  response completes.
- **Live export progress**: exports emit `exports://progress` events
  (preparing → rendering → writing with elapsed seconds) and the Exports
  view refreshes automatically on completion.
- **Operations guide**: [docs/OPERATIONS.md](docs/OPERATIONS.md) covers dev
  engine management, the CI test pattern, release/tag discipline, dry-runs
  and log locations.

### Fixed

- The engine supervisor now kills **and reaps** its sidecar child on
  shutdown — the un-reaped zombie made `kill -0` report the process alive,
  failing the drop-kills test on CI.
- Dry-run bundle verification no longer dies on SIGPIPE under
  `set -e -o pipefail`.
- The test battery is portable across all three CI platforms: fake
  `piper`/`say` scripts generate WAV payloads in Rust instead of shell
  `printf '\xHH'` (unsupported by dash), the loopback HTTP responder
  drains gracefully on Windows, and the fake llama-server tests are
  gated to Unix (Windows cannot exec `#!` scripts).

## [0.1.0] - 2026-10-01

Initial release: the complete private, offline-first research workspace
(Phases 0–9 of the build spec).

### Added

- **Projects & documents**: project library, managed document import with
  checksum dedup, PDF/DOCX/TXT/MD/HTML parsing through the bundled local
  Python engine (Docling, fastembed).
- **Hybrid retrieval**: FTS5 keyword + sqlite-vec vector search with a
  debug panel, powering **grounded local AI asks** (GGUF models via
  llama.cpp, citation-only answers, per-ask evidence inspection).
- **Evidence matrices**: cross-document evidence comparison with strength
  grading; **citations & bibliographies** in APA 7, Harvard and Chicago.
- **Exports**: analyses, matrices and bibliographies as Markdown, DOCX,
  PDF, BibTeX and RIS into the managed workspace.
- **Speech**: local whisper.cpp transcription of lecture recordings into
  timestamped searchable transcripts; spoken summaries and two-host
  podcast segments via local Piper (or the macOS voice).
- **Setup checklist**: first-run verification of every subsystem live.
- **Release CI**: frozen document-engine sidecar + macOS DMG, Windows NSIS,
  AppImage and deb installers; Apple signing activates when secrets are
  configured.

[Unreleased]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/pathway-solutions/researchai-workspace/releases/tag/v0.1.0
