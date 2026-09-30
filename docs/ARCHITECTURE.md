# Architecture

ResearchAI Workspace is a desktop application with a local service sidecar,
built so that every replaceable technology sits behind an internal interface
(spec §47.7).

## Component map

```text
┌────────────────────────────────────────────────────────────┐
│ Desktop app (Tauri 2)                                      │
│                                                            │
│  React UI ──IPC──> Rust core (researchai_lib)              │
│   three-panel       ├── SQLite (research.db)               │
│   workspace         ├── Workspace storage manager          │
│   shell             ├── Settings + diagnostics + logging   │
│                     ├── Ingestion staging (checksums)      │
│                     └── Hybrid retrieval (FTS5 + vectors)  │
└───────┬──────────────────────────┬─────────────────────────┘
        │ loopback HTTP            │ loopback HTTP (free port)
        │ (127.0.0.1:8737)         │ /v1/chat/completions
┌───────▼──────────────────┐   ┌───▼─────────────────────────┐
│ Document engine          │   │ llama-server (llama.cpp)    │
│ (Python sidecar, FastAPI)│   │ spawned/supervised by the   │
│ /health /capabilities    │   │ Rust core; GGUF models from │
│ /parse /embeddings       │   │ the managed models/ dir     │
│ Phase 1+: Docling, OCR,  │   │ Phase 3: local generation   │
│ embeddings               │   │ behind AiProvider           │
└──────────────────────────┘   └─────────────────────────────┘
```

## Key decisions

| Decision | Rationale |
|---|---|
| Tauri 2 + Rust core | Small footprint for 8 GB machines; native FS + packaging; web UI for fast iteration |
| SQLite single DB (`research.db`) | One file to back up; WAL mode; FTS5 + sqlite-vec later in the same DB |
| Python sidecar for parsing | Docling is Python-native; isolates heavy deps from the Rust core; crash-isolated |
| `pnpm` workspace packages | shared-types contracts; future research/citation/retrieval cores stay reusable |
| Managed workspace dir | Projects stay portable; `managed-copy` default, `link-original` opt-in |
| Interface boundaries | `AIProvider` (Phase 3: llama.cpp + echo), `VectorStore` (Phase 2: sqlite-vec), `DocumentParser` (Phase 1: engine sidecar), `ExportProvider` (Phase 6: Markdown/BibTeX/RIS native + DOCX/PDF via the engine), `SpeechToTextProvider` (Phase 7: user-installed whisper.cpp CLI, one batch subprocess per job), `TextToSpeechProvider` (Phase 8: user-installed Piper, cfg-gated macOS `say` fallback) |
| Export split | Text/reference formats (Markdown, BibTeX, RIS) render natively in Rust — deterministic and byte-testable; binary document formats (DOCX, PDF) render in the crash-isolated Python sidecar |
| llama.cpp as a supervised process | The core never links llama.cpp; `llama-server` runs crash-isolated on a free loopback port, is health-polled, idle-unloaded and killed on exit (spec §43, [AI.md](AI.md)) |
| Bundled sidecar, supervised | In packaged builds the frozen Python engine ships under `Resources/sidecar/` and `engine_runtime` spawns it (unless one already answers), health-waits and kills it on exit; in dev it stays dormant and the external `pnpm engine:run` flow is used (Phase 9, [PACKAGING.md](PACKAGING.md)) |

## IPC contract

The frontend calls typed commands (`list_projects`, `create_project`,
`get_settings`, `run_diagnostics`, …). Payload types live in
[packages/shared-types](../packages/shared-types/src/index.ts) and mirror the
Rust DTOs. The UI never depends on llama.cpp/Docling raw responses — services
normalise everything into the shared contract.

## Threading & performance

- The SQLite connection is mutex-guarded; commands are short transactions.
- Native file pickers run `blocking_*` dialogs on a blocking thread so the
  main thread never stalls.
- From Phase 1, ingestion runs through a bounded queue (1 heavy job on LIGHT
  hardware profiles) and the LLM unloads when idle (spec §43).

## Failure isolation

- A failed document parse must never crash the import queue (spec §14):
  per-document status + error text land in the DB in Phase 1.
- The document engine is optional in Phase 0; the app degrades gracefully
  (status bar shows offline) and never blocks startup.
- Human-readable errors only cross the IPC boundary ([error.rs](../apps/desktop/src-tauri/src/error.rs));
  details go to the local log file.

## What deliberately does NOT exist yet

Cloud AI (optional add-on) — every AI feature runs on-device. Phase status:
Phases 0–9 — the master build plan is complete: foundation, document MVP,
hybrid retrieval, local AI, evidence matrices, citations & bibliography,
academic exports, lecture transcription, text-to-speech, and platform
installers ([PHASE_REPORTS.md](PHASE_REPORTS.md)).
