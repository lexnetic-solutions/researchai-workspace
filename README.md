<p align="center">
  <img src="apps/desktop/src-tauri/icons/icon.png" alt="ResearchAI Workspace" width="120" height="120" />
</p>

<h1 align="center">ResearchAI Workspace</h1>

<p align="center">
  <strong>A private, offline-first AI research intelligence workspace.</strong><br/>
  Import your papers, read them, question them with traceable citations,
  compare authors, and export academic artefacts — on your own machine.
</p>

<p align="center">
  <a href="https://github.com/pathway-solutions/researchai-workspace/actions/workflows/ci.yml"><img src="https://github.com/pathway-solutions/researchai-workspace/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <img src="https://img.shields.io/badge/release-v0.1.1-blue" alt="Release v0.1.1" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey" alt="Platforms" />
  <img src="https://img.shields.io/badge/licence-MIT-green" alt="Licence: MIT" />
</p>

> **Status: Phase 9 — Platform installers (final phase).** The app launches, manages projects,
> imports documents — files **or whole folders**, via picker or drag-and-drop
> (managed copy + checksum dedup) — parses PDF/DOCX/PPTX/XLSX/EPUB/HTML/TXT/MD
> through the bundled local Python engine, answers searches with **hybrid retrieval**
> (FTS5 keyword + fastembed/sqlite-vec vectors, debug panel), asks questions
> with **grounded local AI** (GGUF models via llama.cpp, citation-only
> answers), builds **cross-document evidence matrices**, formats **citations
> & bibliographies** (APA 7 · Harvard · Chicago), exports everything —
> analyses, matrices, bibliographies — as **Markdown, DOCX, PDF, BibTeX and
> RIS** into the managed workspace, **transcribes lecture recordings**
> with local whisper.cpp into timestamped, searchable transcripts, and
> **speaks documents aloud** — read-aloud, 5/10/20-minute spoken summaries
> and two-host podcast segments — via local Piper (or the macOS voice).
> Release CI builds the frozen sidecar + signed-when-configured installers
> (macOS DMG, Windows NSIS) and the first-run **Setup checklist** verifies
> every subsystem live.

## Screenshots

_App screenshots (three-panel workspace, evidence matrix, setup checklist)
will be captured on real hardware and placed here — presentation pattern
adapted from the Voicebox study; see [docs/DESIGN.md](docs/DESIGN.md)._

## Repository layout

```text
apps/desktop/        Tauri 2 + React + TypeScript desktop app
  src/               React UI (three-panel research workspace shell)
  src-tauri/         Rust core: SQLite, settings, retrieval, local AI runtime
packages/            shared-types · research-core · citation-core · retrieval-core · ui
services/
  document-engine/   Python sidecar: parsing/OCR/embeddings (Docling, fastembed)
scripts/             setup, dev and release helpers
docs/                 ARCHITECTURE · AI · DATABASE · RAG · CITATIONS · SECURITY ·
                     PACKAGING · SPEECH_SETUP · PHASE_REPORTS
tests/               cross-cutting test fixtures (Phase 1+)
.github/workflows/   CI: lint + test + desktop build
```

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Node.js | ≥ 20 | Node 22 tested |
| pnpm | ≥ 9 | `corepack enable` or `npm i -g pnpm` |
| Rust | stable | `rustup` — needed for the desktop app |
| Python | ≥ 3.11 | via [uv](https://docs.astral.sh/uv/) |
| Platform libs | — | Tauri prerequisites: Xcode CLT (macOS), WebView2 (Windows), webkit2gtk (Linux) |

## Quick start

```bash
# 1. Everything: deps, env, icons, engine env (idempotent)
pnpm setup

# 2. The document engine sidecar (required for parsing)
pnpm engine:run

# 3. The desktop app (compiles the Rust core on first run)
pnpm desktop:dev
```

Then: create a project → Library → Import files/folder. Documents flow
through `waiting → parsing → indexing → ready` (embeddings computed during
indexing); click a title to read the extracted text. Optional speech
(offline STT/TTS via piper + whisper.cpp) is covered by
[docs/SPEECH_SETUP.md](docs/SPEECH_SETUP.md), including a setup checker at
`scripts/dev/check-speech-setup.sh`. Open **Search** for
hybrid keyword + semantic retrieval — *Show retrieval debug* exposes
channels, scores and index coverage for every query.

Local AI (Phase 3): install
[llama.cpp](https://github.com/ggml-org/llama.cpp) and put a small instruct
GGUF (e.g. Qwen3 0.6B/1.7B Q4_K_M) somewhere on disk → Settings → **AI
enabled** → Local AI: import the model, point the app at your `llama-server`
binary, then open **Research AI** and ask — every answer cites the numbered
library excerpts it was grounded in (see [docs/AI.md](docs/AI.md)).

Frontend-only UI work in a browser:

```bash
pnpm dev        # http://localhost:1420 — uses an in-browser mock backend
```

## Testing

```bash
pnpm typecheck       # all TypeScript packages
pnpm test            # node:test suites (workspace packages)
pnpm engine:test     # Python sidecar + parsers
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml   # Rust core

# End-to-end ingestion test (needs the engine running):
pnpm engine:run &
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --test live_ingestion
```

Phase 3 adds `--test contract_locks` (IPC wire-format locks) and the local-AI
suites inside `cargo test` — providers, model manager, runtime supervisor and
the grounded ask pipeline all run without llama.cpp installed.

## Privacy

All data stays under the local app-data directory (`ResearchAIData` layout):
documents, database, notes, logs, and later models and exports. No telemetry,
no accounts, no cloud calls. Cloud AI remains an optional, explicit add-on —
the core never requires an API key (spec §41).

## License

MIT for this repository — see [LICENSE](LICENSE) and
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) for upstream components and
flagged copyleft risks (Piper TTS, citation-js).
