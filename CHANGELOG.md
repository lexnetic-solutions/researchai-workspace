# Changelog

All notable changes to ResearchAI Workspace are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The `[Unreleased]` section is a non-destructive working draft — update it any
time during development (see `.agents/skills/draft-release-notes/`). Release
cuts stamp it with a version and date (see `.agents/skills/release-bump/`),
and the stamped section becomes the GitHub Release body.

## [Unreleased]

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

[Unreleased]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/pathway-solutions/researchai-workspace/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/pathway-solutions/researchai-workspace/releases/tag/v0.1.0
