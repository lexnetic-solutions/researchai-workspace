# Phase Reports

Completion log per the master build spec: each phase closes with a report
(scope, verification, limitations) before the next phase begins. Newest phase
last.

---

## Phase 0 — Foundation (complete)

Tauri 2 + React/TS/Vite desktop shell; Rust core with SQLite migrations,
settings, diagnostics, hardware profiles, logging; Python FastAPI
document-engine scaffold; workspace layout, CI, licenses, docs.

Verified: TS typecheck clean, 7/7 TS tests, desktop production build, launch
smoke test. App runnable at close.

## Phase 1 — Document MVP (complete)

Real parsers (PDF/DOCX/TXT/MD/HTML) producing page+offset blocks with honest
errors; migration 2 (`documents` / `document_sections` / `chunks`, checksum
dedup); managed-copy imports with per-project duplicate rejection; background
queue (`waiting → parsing → indexing → ready | failed`, engine-offline
requeue); IPC commands; Library UI (status badges, polling, retry, delete,
reader).

Verified: 13/13 pytest, 8/8 Rust unit tests, live E2E ingestion against the
running engine. Fixtures committed under `tests/fixtures/phase1/`.

---

## Phase 2 — FTS5 + embeddings + hybrid retrieval (complete)

### Scope shipped

**Document engine (Python)** — [embeddings.py](../services/document-engine/src/researchai_document_engine/embeddings.py),
[app.py](../services/document-engine/src/researchai_document_engine/app.py)

- `POST /embeddings` and `GET /embeddings/status` endpoints.
- fastembed provider: `BAAI/bge-small-en-v1.5`, 384-dim, L2-normalized,
  locally cached (no network at query time). Deterministic hashing fallback
  keeps the API contract alive when the model can't load.
- Status endpoint reports `{engine, dimensions, model}` so the Rust side can
  hard-fail on dimension mismatch instead of writing garbage vectors.

**Rust core** — [retrieval.rs](../apps/desktop/src-tauri/src/services/retrieval.rs),
[db.rs](../apps/desktop/src-tauri/src/db.rs)

- Migration 3: external-content FTS5 table `chunks_fts`
  (`porter unicode61`) with AI/AU/AD triggers; `chunk_embeddings`
  (little-endian f32 BLOBs, `document_id` cascade — see
  [DATABASE.md](DATABASE.md) for why there is no rowid FK); `index_metadata`.
- sqlite-vec 0.1.9 registered via `sqlite3_auto_extension` **before**
  `Connection::open` (`register_vec_extension()`, Once-gated and idempotent);
  `vec_distance_cosine` available as a SQL function for exact (brute-force)
  KNN.
- `VectorStore` trait with a `SqliteVecStore` implementation — vector
  storage/KNN is swappable; queries embed via a small ureq-3 client with
  global timeout against `/embeddings`.
- Hybrid retrieval: FTS5 keyword channel (`"{q}"*` prefix query, bm25 ASC)
  + semantic channel (cosine KNN, ASC), fused with Reciprocal Rank Fusion
  (`RRF_K = 60`), per-document cap (4), near-duplicate collapse
  (token Jaccard ≥ 0.9), limit clamped to 1–50.
- `RetrievalTrace` on every response: per-channel candidate counts,
  embedding coverage, warnings (engine offline, dimension mismatch, empty
  query, …) and `matched_by` per hit (`keyword` / `semantic` / both).
- `ensure_document_embedded` wired into the queue's success path — best
  effort; on failure the document still reaches `ready` with a `status_detail`
  note rather than blocking ingestion.
- New IPC commands: `search_library`, `retrieval_status`.

**Frontend (TS/React)** — [SearchView.tsx](../apps/desktop/src/views/SearchView.tsx)

- Search nav entry and routing; project/document scope select; debounced
  query execution.
- Hit cards: score, `matched_by` channel badge, page number, snippet.
- Debug panel (per spec §16): live warnings, embedding coverage
  (`embedded/total`), per-channel candidate counts — the panel that makes
  hybrid retrieval auditable rather than magical.
- Shared types + typed client methods; mock-backend cases so the UI preview
  works without the engine.

**Docs** — RAG.md (architecture + interface status), DATABASE.md (migration-3
schema section), THIRD_PARTY_LICENSES.md (fastembed Apache-2.0, sqlite-vec
MIT), README.md (Phase 2 status + search quick-start).

### Verification

| Check | Result |
| --- | --- |
| Engine pytest | 18/18 pass |
| ruff | clean |
| Rust unit tests | 12/12 pass |
| Rust live E2E (`cargo test --test live_ingestion`, engine up) | 1/1 pass |
| TS typecheck + vitest | 0 errors, 0 failures |
| Desktop production build (`tsc && vite build`) | success |
| Preview bundle rebuilt | ✓ |

Live engine checks: `/embeddings/status` →
`{"engine":"fastembed","dimensions":384,"model":"BAAI/bge-small-en-v1.5"}`;
KNN self-sentence cosine distance < 0.35; semantic channel appears in
`matched_by`; RRF fusion merges keyword + semantic candidates with per-doc cap
applied. The `search_keyword_channel_works_end_to_end` unit test is
environment-agnostic (fixture has no embeddings → asserts
`vector_candidates == 0`, coverage `0/…`, keyword-only hits).

App runnable at close: import → parse → embed → search works end-to-end
against the live engine.

### Limitations / known trade-offs

- KNN is **brute-force** over `chunk_embeddings` — exact, not approximate;
  correct and fast at library scale. Swapping to sqlite-vec `vec0` virtual
  tables (ANN) is a future migration, deliberately deferred.
- No reranker or query expansion yet; RRF over two channels only.
- Hashing fallback produces non-semantic vectors — engine-tagged per row, but
  automatic re-embedding of an existing library after a model/engine change is
  not automated yet (import-time embedding only).
- FTS query is an escaped phrase-prefix match — robust, but no boolean/field
  syntax exposed to users yet.
- Debug panel reflects the Rust trace verbatim; no per-channel latency timing
  yet.

### Next

Phase 3 per spec: AIProvider abstraction over llama.cpp with a local model
manager (download/gguf selection, context settings), feeding citations and the
analysis pipeline. Candidate stretch items pulled from the limitations above:
reranker pass, re-embed backfill job, FTS query syntax.

---

## Phase 3 — Local AI: AIProvider + model manager (complete)

### Scope shipped

**AIProvider abstraction** —
[ai.rs](../apps/desktop/src-tauri/src/services/ai.rs)

- `AiProvider` trait (`complete` + streaming `complete_stream`) — the single
  seam for every AI feature (spec §47.7).
- `LlamaCppProvider` speaking llama-server's OpenAI-compatible API on
  loopback (`/v1/chat/completions`, `/health`), with SSE streaming and token
  usage parsing. The core never links llama.cpp.
- `EchoProvider` — deterministic offline fallback, loudly labelled as not a
  real answer (tests + visible "model not loaded" degradation).
- Provider tests drive a canned loopback HTTP server, so SSE/JSON parsing is
  verified without llama.cpp installed.

**Model manager** —
[model_manager.rs](../apps/desktop/src-tauri/src/services/model_manager.rs)

- GGUF validation (magic bytes) + header walk extracting parameter count and
  quantisation label; weights never read.
- Import-in-place into the managed `models/` directory (SHA-256 computed in
  the same pass); streamed URL download with progress events
  (`ai://model-download`) and cooperative cancel; non-GGUF payloads rejected
  and cleaned up.
- Registry lifecycle with filesystem reconciliation (`scan_for_missing`),
  status machine `available | missing | downloading | failed`.
- Migration 4: `local_models`, `analyses` (see
  [DATABASE.md](DATABASE.md)).

**LLM runtime supervisor** —
[llm_runtime.rs](../apps/desktop/src-tauri/src/services/llm_runtime.rs)

- Sole-owner worker thread spawns `llama-server` (user-configured binary) on
  a free loopback port with `--ctx-size`, threads, GPU layers and user extra
  args; stdout/stderr stream to `logs/llama-server.log`.
- Health-poll until weights load; fail-fast with the log tail if the process
  dies; 10-minute health deadline; cancellation flag so unload/shutdown and
  app exit never hang behind a health wait.
- `models/llama-server.pid` + command-line sweep terminate stale servers
  from crashed previous runs at startup; idle auto-unload (spec §43);
  crash detection flips the state machine to `failed`.
- State machine exposed to the UI: `idle → loading → ready{port} | failed`,
  plus `unloaded`.

**Grounded ask pipeline** —
[analysis.rs](../apps/desktop/src-tauri/src/services/analysis.rs)

- Five modes (spec §17.2): Chat, Research, Quick Read, Deep Analysis,
  Critical — each with its own strict instruction.
- Evidence: hybrid retrieval (Phase 2) for project scope, reading-order
  chunks for single-document scope; retrieval trace warnings carried into
  the analysis trace.
- Strict citation-only system prompt; `[n]` markers parsed from answers;
  zero-citation answers are flagged as ungrounded in the trace.
- Answer + evidence + trace persisted to `analyses` (prompt-versioned) and
  re-openable from the Research view history.
- No-AI mode is a hard gate in the command layer (spec §35).

**IPC & contracts** — [commands/ai.rs](../apps/desktop/src-tauri/src/commands/ai.rs)

- 13 new commands (`ai_list_models` … `ai_get_analysis`) plus
  `set_ai_enabled` and `ai_pick_model_file`.
- **Wire-format fix**: several Phase 1/2 DTOs serialised snake_case while the
  TS contract reads camelCase (latent native-only bug; mock previews could
  not catch it). All IPC-crossing DTOs now camelCase, and
  [contract_locks.rs](../apps/desktop/src-tauri/tests/contract_locks.rs)
  locks every payload key-for-key against `shared-types` so the drift cannot
  return. `SystemInfo` now nests `cpu: {name, cores}` as the TS side always
  declared.

**Frontend** —
[ResearchView.tsx](../apps/desktop/src/views/ResearchView.tsx),
[SettingsView.tsx](../apps/desktop/src/views/SettingsView.tsx)

- Research view is now the Ask panel: five mode cards, scope selector,
  runtime banner with manual load/unload, answers with clickable citation
  chips that scroll to evidence cards, debug panel (engine, model, tokens,
  citations used, coverage, warnings), per-project history.
- Settings → Local AI: model library (import/download/select/delete with
  download progress), llama-server path, context size, max tokens,
  temperature, GPU layers, idle-unload, runtime control.
- AI mode toggle (No-AI ↔ AI enabled) in Settings; sidebar Research AI entry
  unlocked; preview mocks for the full AI surface.

**Docs** — new [AI.md](AI.md); DATABASE.md (migration 4), ARCHITECTURE.md
(component map + llama.cpp decision), SECURITY.md (network posture, GGUF
handling), README.md (Phase 3 status + llama.cpp quick-start).

### Verification

| Check | Result |
| --- | --- |
| Rust lib tests (providers, manager, runtime, pipeline, db) | 40/40 |
| Contract locks (IPC wire format) | 9/9 |
| Rust live E2E (engine up) | 1/1 |
| Engine pytest / ruff | 18/18 / clean |
| TS typecheck (all packages) | 0 errors |
| TS node tests / production build | 7/7 / ✓ |
| Preview rebuilt + Ask panel exercised in browser | ✓ |

Runtime supervisor verified against a fake llama-server (real loopback HTTP
server): ready-state health polling, unload-kills-process, missing binary/
model failures with readable messages, and stale-server termination across a
simulated crashed run (zombie-aware reaping). App runnable at close; all
work uncommitted on `main`.

### Limitations / known trade-offs

- First Ask blocks while weights load (up to minutes on 8 GB machines);
  streaming is implemented at the provider level but not surfaced in the Ask
  panel UI yet.
- Single-shot generation: multi-turn chat flattens history into the
  question; no provider-side conversation state.
- llama-server is not bundled or auto-downloaded — the user points Settings
  at an installed binary (packaging lands in a later phase).
- Prompt-injection hardening for imported document text is a follow-up for
  the security pass (noted in [SECURITY.md](SECURITY.md)).
- Reranker / query-expansion stretch items from Phase 2 remain open.

### Next

Phase 4 per spec: evidence tables / cross-document comparison over the
retrieval and analysis layers built here (`analyses` + hybrid retrieval are
the data sources; the Evidence view is scaffolded).

---

## Phase 4 — Evidence matrices / cross-document comparison (complete)

### Scope shipped

**Deterministic matrix engine** —
[evidence.rs](../apps/desktop/src-tauri/src/services/evidence.rs)

- One hybrid-retrieval pass per document (per-doc isolation avoids the
  Phase 2 per-doc cap skewing cross-document comparison), one matrix row per
  document with up to 3 page-referenced excerpts, channel badges and scores.
- **Evidence strength labels derived from the retrieval path** — `direct`
  (matched by both keyword and vector channels), `multiple` (≥2 excerpts),
  `indirect` (single-channel hit), `none` — describing HOW a document
  matched, never an invented truth score (spec §18).
- Rows sorted best-match first; scope defaults to all ready documents
  (capped at 20 with a visible warning); works with AI disabled (spec §35).

**Optional AI passes** (only when AI is enabled and the model is ready)

- Per-document findings: numbered excerpts → cited summary of what THAT
  document says, under a citation-only system prompt.
- Cross-document synthesis: AGREEMENTS / DISAGREEMENTS / GAPS over the
  per-document findings, with document-number citations; ungrounded output
  is detected by the shared citation parser.
- Failures degrade gracefully: per-document findings errors become trace
  warnings, and a missing provider downgrades to the deterministic table
  with an explanation instead of failing the request.

**Persistence** — migration 5: `evidence_tables` (question, scope, table
JSON, trace JSON, model id — NULL for deterministic tables, prompt version;
see [DATABASE.md](DATABASE.md)). Tables are saved from the UI, listed,
re-openable and deletable per project.

**IPC & contracts** —
[commands/evidence.rs](../apps/desktop/src-tauri/src/commands/evidence.rs)

- Five commands (`evidence_build/save/list/get/delete`); the No-AI gate is
  enforced in the command layer while the deterministic build always runs.
- [contract_locks.rs](../apps/desktop/src-tauri/tests/contract_locks.rs)
  now locks the full evidence wire format key-for-key (10 suites).

**Frontend** —
[EvidenceView.tsx](../apps/desktop/src/views/EvidenceView.tsx)

- The Evidence view is now the matrix builder: question input, "include AI
  findings & synthesis" checkbox (disabled outside AI mode), the matrix
  table (document / strength chip / excerpts with page + channel chips),
  debug panel (strength counts, engine, warnings), Save, and the saved-table
  history with delete.
- Citation rendering shared between Research and Evidence views
  ([CitedText.tsx](../apps/desktop/src/components/CitedText.tsx)); Evidence
  nav entry unlocked.

**Docs** — DATABASE.md (migration 5), ARCHITECTURE.md (boundary row, phase
status), README.md (Phase 4 status).

### Verification

| Check | Result |
| --- | --- |
| Rust lib tests (matrix engine incl. AI passes, db CRUD) | 47/47 |
| Contract locks (IPC wire format, now incl. evidence) | 10/10 |
| Rust live E2E (engine up) | 1/1 |
| Engine pytest / ruff | 18/18 / clean |
| TS typecheck (all packages) / node tests | 0 errors / 7/7 |
| Desktop production build + preview rebuild | ✓ |
| Preview: matrix built end-to-end in browser (import → build → render) | ✓ |

Engine-level checks: deterministic table matches only relevant documents
(sleep document row `none`, delta document row `indirect` on a keyword-only
corpus); AI passes verified with the EchoProvider producing findings for
each matched document plus a bounded synthesis; no-provider fallback warns
and degrades; tables roundtrip through `table_json`/`trace_json`.
App runnable at close; all work uncommitted on `main`.

### Limitations / known trade-offs

- Document scope is currently the whole project; a per-document picker for
  the matrix (the stored `scope_json` already supports it) is a small
  follow-up.
- AI findings/synthesis run sequentially per document — fine for library
  scale, parallelisable later.
- The synthesis cites document numbers, not pages; the per-document findings
  remain the page-level record.
- No CSV/export of the matrix yet (Phase 6 exports).

### Next

Phase 5 per spec: citations & bibliography — Citation.js/CSL formatting
(APA 7 / Harvard / Chicago) behind `citation-core`, user-correctable
metadata, and in-text citation resolution into the reader.

---

## Phase 5 — Citations & bibliography (complete)

### Scope shipped

**Pre-phase audit** — reviewed Phases 0–4 end-to-end (plugins/capabilities,
CSP, CI, dependency wiring, full test battery). Fixed: CI now runs `ruff`
and the Rust unit + contract-lock suites; removed the dead
`pnpm.onlyBuiltDependencies` block (pnpm 11 reads it from
pnpm-workspace.yaml). Committed as `822e5ed`.

**Licensing decision (deviation, documented)** — the spec suggested
Citation.js; it is **AGPL-3.0** and flagged for commercial review in
THIRD_PARTY_LICENSES.md. Shipped instead: a built-in deterministic
formatter — zero new dependencies, MIT-clean, offline. The
`CitationFormatter` trait keeps a CSL engine swappable
([CITATIONS.md](CITATIONS.md)).

**Rust core** —
[citations.rs](../apps/desktop/src-tauri/src/services/citations.rs)

- APA 7, Harvard, Chicago author-date formatters over structured metadata:
  author parsing ("Doe, Jane; Smith, J."), initials, et-al collapse,
  container variants (journal / book / webpage), DOI links, graceful
  degradation with an `incomplete` flag when title/year are missing.
- Alphabetised project bibliography (references shape, stable tiebreak).
- In-text citations: APA joins two authors with "&", Harvard with "and",
  3+ collapse to et al., no-author falls back to the title, no-year → n.d.
- Migration 6: bibliographic columns on `documents` (journal, volume,
  issue, pages, publisher, url, ref_type NOT NULL DEFAULT 'article').
- `update_document_bibliography` — user corrections are authoritative;
  `ref_type=None` keeps the current value (NOT NULL column).
- `apply_bibliographic_hints` — engine hints fill EMPTY fields only.

**Engine** — parse responses now carry DOI/year **hints** scraped from the
document text (first DOI with trailing punctuation trimmed, plausible
publication year); wired into the ingestion success path without touching
user-corrected fields.

**IPC + contract locks** — `update_document_bibliography`,
`bibliography_list`; `DocumentRow` wire format extended by 10 keys and the
contract locks updated (11 suites); `FormattedReference` locked.

**Frontend** —
[BibliographyView.tsx](../apps/desktop/src/views/BibliographyView.tsx),
[DocumentsView.tsx](../apps/desktop/src/views/DocumentsView.tsx)

- New Bibliography view: APA 7 / Harvard / Chicago selector, alphabetised
  formatted list, in-text form per entry, incomplete-metadata chips, copy
  per reference and copy-all.
- Library reader: citation-metadata panel (authors, year, ref type, journal,
  volume, issue, pages, publisher, DOI/URL) with a save flow and the
  "corrections are authoritative" note.
- Research evidence cards point at the Bibliography view.

### Verification

| Check | Result |
| --- | --- |
| Rust lib tests (formatter styles/variants, hints, bibliography CRUD) | 56/56 |
| Contract locks (extended wire format) | 11/11 |
| Rust live E2E (engine up, restarted with new fields) | 1/1 |
| Engine pytest (incl. DOI/year hint extraction) / ruff | 20/20 / clean |
| TS typecheck / node tests / build / preview | clean / 7/7 / ✓ / ✓ |
| Preview: import → Bibliography renders formatted entry | ✓ |

Self-deadlock found and fixed during development: `bibliography()` held the
DB mutex across `get_document` (re-locking) — ids are now collected under a
scoped lock. App runnable at close; engine restarted with the new `/parse`
fields and verified healthy (fastembed, 384-dim).

### Limitations / known trade-offs

- Three styles implemented by hand; exotic metadata (multiple containers,
  edition fields, translated titles) is not modelled yet.
- No CSL import; MLA/IEEE/Vancouver arrive as new formatters or a CSL engine
  if licensing is accepted.
- Author parsing is best-effort on the display string; no separate given/
  family storage yet.
- `citations` table (chunk-level citation links, spec §31) is deferred to
  the evidence/exports work — bibliography formatting doesn't need it.

### Next

Phase 6 per spec: academic exports — summaries and evidence matrices to
Markdown/DOCX/PDF, bibliography export (.bib/.ris/.csly), behind the
`ExportProvider` interface.

---

## Phase 6 — Academic exports (complete)

### Scope shipped

**Rust core** —
[exports.rs](../apps/desktop/src-tauri/src/services/exports.rs)

- `ExportProvider` trait (spec §47.7) over a neutral `ExportDoc` model
  (paragraph / bullet / heading / table blocks).
- **Markdown, BibTeX, RIS render natively** — deterministic, byte-testable:
  pipe tables, `@article{key, …}` entries with escaped braces and stable
  `surnameYEARword` citation keys, spec-shaped RIS records (TY/AU/TI/JO/…/ER).
- DOCX/PDF delegate to the engine (crash-isolated heavy rendering); the
  engine-offline error is actionable (`pnpm engine:run`).
- Three export kinds — analysis (answer + numbered cited passages), evidence
  matrix (table + findings + synthesis), bibliography (formatted list or
  reference-database .bib/.ris) — written to the managed `exports/` directory
  with unique stamped filenames; invalid kind/format combinations are
  rejected up front.
- Migration-free: exports reuse analyses/evidence_tables/bibliography data.

**Engine** —
[exports.py](../services/document-engine/src/researchai_document_engine/exports.py)

- `POST /export/docx` (python-docx: heading, styled tables, bullet lists)
  and `POST /export/pdf` (reportlab, BSD-3-Clause, added to THIRD_PARTY
  licenses): flowing bullets, gridded tables, escaping. Validation errors
  surface as HTTP 400, never 500.

**IPC + frontend** —
[ExportsView.tsx](../apps/desktop/src/views/ExportsView.tsx)

- Five commands (`export_capabilities`, `export_document`,
  `export_bibliography`, `reveal_path`, `list_exports`); ExportsView is now
  functional: kind → format pickers driven by the capability map, source
  pickers listing recent analyses/tables, style selector, exported-file
  list with sizes, reveal via the opener plugin.
- Nav entry unlocked; preview mocks for the whole surface.

### Verification

| Check | Result |
| --- | --- |
| Rust lib tests (exporters, keys, escaping, kinds, rejections) | 67/67 |
| Contract locks | 11/11 |
| Rust live E2E (engine up) | 1/1 |
| Engine pytest (incl. DOCX magic bytes, PDF header/EOF, 400s) / ruff | 24/24 / clean |
| TS typecheck / build / preview rebuild | clean / ✓ / ✓ |

App runnable at close; engine restarted earlier in Phase 5 is unaffected by
the additive endpoints until next restart (health verified).

### Limitations / known trade-offs

- DOCX/PDF styling is deliberately minimal (no headers/footers/page numbers
  yet); the neutral block model keeps that additive.
- No .csly/CSL-JSON export yet; BibTeX/RIS cover the reference-manager
  round trip.
- Exports directory grows without a cleanup pass (files never deleted
  implicitly); a storage-manager sweep is a future hardening item.
- PDF export embeds base-14 fonts only (no custom font bundling yet).

### Next

Phase 7 per spec: lecture transcription (SpeechToTextProvider over local
whisper.cpp, mirroring the llama.cpp pattern: user-supplied binary, managed
models, idle unload).

## Phase 7 — Lecture transcription (complete)

### Scope shipped

- **`SpeechToTextProvider` trait** (`services/transcription.rs`, spec §47.7)
  with `transcribe(audio)` + `check()` — the neutral seam spec §20 calls for;
  a future provider (whisper server, other engine) slots in without touching
  UI or pipeline.
- **`WhisperCppProvider`**: runs the user-configured `whisper-cli` as a
  one-shot subprocess per job (batch mode — no resident server and therefore
  no idle-unload logic, unlike llama.cpp). Args: `-m <model> -f <wav> -oj -of
  <workdir> [-l <lang>]`. Binary and GGML model paths are user settings
  (`stt.*` keys, Settings → Speech); the app never bundles or downloads
  models. Failures surface the CLI's stderr with exit status — actionable,
  not raw.
- **Honest audio handling**: 16 kHz mono PCM WAV is sniffed (RIFF walk to the
  `fmt ` chunk) and passed through directly; anything else converts via
  `ffmpeg -ar 16000 -ac 1 -c:a pcm_s16le` when ffmpeg is on PATH and
  conversion is enabled — with a concrete error ("WAV is 44100 Hz", "not a WAV
  file") when it is not.
- **`-oj` JSON parsing**: `transcription[]` (`offsets.from/to` ms + `text`)
  into `TranscriptionSegment`s, plus detected language and total duration.
- **Transcript→document pipeline** (`library::save_transcription`): segments
  grouped into ~1200-char chunks, each line prefixed `[mm:ss]`, stored as a
  real `transcript` document (new `RefType::Transcript` — bibliography
  renders it as a plain non-punctuated line, never a fake journal article)
  through the same chunk writer the engine parse path uses, then embedded via
  `ensure_document_embedded` — transcripts join FTS + vector hybrid search,
  Ask and evidence matrices like any paper. Checksum dedup: re-transcribing
  the same audio returns the existing transcript.
- **Commands** (`commands/stt.rs`): `stt_check` (configured + cli/model/ffmpeg
  probes), `stt_get_settings` / `stt_save_settings` (trim + language
  normalisation), `stt_transcribe` (spawn_blocking, settings gate with an
  actionable message, best-effort embeddings with a visible note when the
  engine is offline). Four new contract locks pin the wire shapes.
- **UI**: functional **Audio tab** (engine status dots, file picker,
  transcribe → toast + transcript preview with `[mm:ss]` markers → Library),
  **Settings → Speech** section (paths, browse, language, ffmpeg toggle,
  found/missing chips), and the sidebar **Audio** entry unlocked.
- **Preview mocks** for all four commands (`stt_check` reports configured;
  `stt_transcribe` fabricates a transcript document + result), so the flow is
  demonstrable in the browser preview.

### Verification

- `cargo test --lib`: **77 passed**, 0 warnings (8 new transcription tests:
  WAV sniff accept/reject with concrete mismatch, fake-binary end-to-end,
  actionable failure, ffmpeg refusal, check probes, timestamps/grouping;
  STT settings roundtrip; transcript document pipeline).
- `cargo test --test contract_locks`: **15 passed** (4 new STT locks; the
  segment lock caught a real drift — segments initially serialised
  `start_ms` — fixed to `startMs` before it could reach the UI).
- `pnpm exec tsc --noEmit` + `pnpm build`: clean.
- Engine suite untouched and green from Phase 6 (24 pytest, ruff clean) — no
  engine changes in this phase.
- Browser preview rebuilt; Audio tab demo: pick file → mock transcribe →
  transcript card with `[mm:ss]` preview → Library.

### Limitations / known trade-offs

- One job at a time, UI-level only: a second transcription while one runs is
  possible in the backend (each is an independent subprocess) but the Audio
  tab serialises; a queue belongs with Phase 8's batch workflows.
- No progress events: whisper-cli prints segment lines we don't yet stream;
  the UI shows an indeterminate "Transcribing…" state. Streaming progress is
  a natural Phase 8 addition alongside TTS.
- Non-WAV conversion requires ffmpeg on PATH; we probe but never download it
  (same user-owned tooling rule as whisper.cpp itself).
- Whisper output keeps its own segment granularity; punctuation/paragraphing
  quality follows the chosen model, not post-processing.

### Next

Phase 8 per spec: text-to-speech behind `TextToSpeechProvider` — read-aloud,
5/10/20-minute audio summaries and podcast narration — plus transcription
progress streaming if the spec's batch workflow needs it.

## Phase 8 — Text-to-speech (complete)

### Scope shipped

- **`TextToSpeechProvider` trait** (`services/tts.rs`, spec §47.7) with
  `render(text, hint)` + `check()` — the neutral seam that keeps GPL Piper
  isolated from the app core (spec §7 license table).
- **`PiperProvider`** (spec default): runs the user-installed `piper` binary
  as a one-shot subprocess (`--model <onnx> --length-scale 1/speed
  --output_file <wav>`, text on stdin). Binary + voice model paths are user
  settings (`tts.*`); the app never bundles or downloads voices. Failures
  surface piper's stderr with exit status.
- **`MacOsSayProvider`** (cfg-gated macOS): zero-install fallback via the
  built-in `say` (`--data-format=LEF32@22050`, voice + rate from settings).
  Piper stays the default; `say` is explicit opt-in.
- **Narration scripting** (`services/narration.rs`): read-aloud passes the
  document's own text through light "speakable" cleanup (markdown/heading
  stripping) with no AI; 5/10/20-minute summaries (~650/1300/2600 words) and
  a two-host podcast segment (MAYA & Dr. PATEL, ≈1200 words) are generated by
  the local model through the existing `AiProvider` seam under strict
  spoken-prose-only prompts — no markdown that Piper would read aloud. No-AI
  mode gates the AI kinds and keeps read-aloud (spec §35).
- **Commands** (`commands/tts.rs`): `tts_check`, `tts_get/save_settings`
  (provider normalisation, speed clamp 0.5–2.0), `tts_speak_document`
  (read-aloud) and `tts_narrate` (AI kinds, model load-on-demand identical to
  `ai_ask`). Audio lands in the managed `exports/` folder.
- **MP3 export**: optional WAV→MP3 transcode (`-ac 1 -b:a 64k`) via ffmpeg
  when enabled and on PATH; failures degrade to WAV with a logged note.
- **UI**: Audio tab gains a **Speak a document** card (provider status chips,
  ready-doc picker, narration-kind select, result card with duration/words/
  size/engine + Reveal in Finder via the Phase 6 reveal path); Settings →
  Speech gains the voice block (provider select, Piper paths, macOS voice,
  speed slider, MP3 toggle).
- **Preview mocks** for all five commands (`tts_check` reports ready;
  narrate honours the No-AI mode gate), so the flow is demonstrable in the
  browser preview.

### Verification

- `cargo test --lib`: **89 passed**, 0 warnings (12 new: Piper fake-binary
  render with byte-exact WAV, actionable binary/model errors, stderr
  surfacing, macOS say fake render, provider selection; narration speakable
  cleanup, summary/podcast prompt + script checks, empty-output error,
  sentence-boundary clipping; TTS settings roundtrip incl. unknown-provider
  fallback).
- `cargo test --test contract_locks`: **19 passed** (4 new TTS locks: settings,
  status, narration result, audio/status).
- `pnpm exec tsc --noEmit` + `pnpm build`: clean.
- Engine suite untouched and green (24 pytest + ruff) — no engine changes.
- Browser preview rebuilt; Speak demo: pick document → read-aloud → mock
  audio card (duration · words · size · engine) → Reveal toast.

### Limitations / known trade-offs

- In-app playback needs a Tauri asset-protocol scope (`media-src` + asset
  protocol allow-list); until then the UI follows the Phase 6 exports
  convention and reveals the file in the OS file manager.
- No streaming synthesis: long scripts render in one piper invocation, so a
  20-minute summary takes tens of seconds (indeterminate progress only).
  Chunked per-section synthesis is a future hardening item.
- Podcast voices are single-voice (both hosts share one model); true
  multi-voice needs per-line provider switching or a multi-speaker model.
- `say` duration estimates assume 22.05 kHz stereo f32 output; the WAV path
  (piper) is byte-exact, MP3 keeps the pre-transcode estimate.

### Next

Phase 9 per spec: platform installers (signed macOS .app/.dmg and Windows
NSIS), bundling the sidecar and finalising the first-run experience — the
final phase of the master build plan.

## Phase 9 — Platform installers (complete)

### Scope shipped

- **Sidecar lifecycle** (`services/engine_runtime.rs`): a supervisor thread
  that spawns the bundled document engine from
  `Resources/sidecar/researchai-engine*` at startup (unless an engine already
  answers on 127.0.0.1:8737 — the dev/screen flow is untouched), honours an
  `engine.env` ops file, waits ≤30 s for `/health`, and kills the child on
  app exit. Missing sidecar files are a normal dev condition: the supervisor
  stays dormant and keeps probing so externally started engines are picked
  up. Wired into `AppState` via `initialize_with_resources` (resource dir
  only exists packaged).
- **Bundle configuration**: `tauri.conf.json` now carries the generated
  platform icon set (icns/ico/png via `tauri icon`), category, descriptions,
  DMG/NSIS metadata, and the `packaging/resources/sidecar/*` resources
  mapping. Targets: `dmg`+`app` (macOS), `nsis` (Windows), `appimage`+`deb`
  (Linux, dev builds).
- **Packaging scripts**: `scripts/package/collect-sidecar.mjs` assembles the
  resources dir (frozen PyInstaller binary when present, `uv` fallback
  launcher for dev machines, explicit skip mode);
  `services/document-engine/packaging/pyinstaller.spec` + `run.py` freeze the
  engine per-OS.
- **First-run setup checklist** (Home, spec §48): live probes for document
  engine, local AI (mode + model state), speech-to-text and voice output,
  each with a concrete status line and a jump-to-fix button; replaces the
  static "Planned in later phases" grid now that all phases have shipped.
- **Release CI** (`.github/workflows/release.yml`): tag `v*` builds the
  frozen sidecar (PyInstaller) then `tauri build` on a macOS arm64 + macOS
  x64 + Windows matrix, uploads DMG/APP/NSIS artefacts; signing hooks read
  `APPLE_CERTIFICATE` secrets when configured (ad-hoc otherwise, documented
  Gatekeeper bypass).
- **Docs**: PACKAGING.md documents the full sidecar → bundle → runtime path
  and the release checklist; icons README flow now actually executed.

### Verification

- `cargo test`: **112/112** (92 lib incl. 3 new engine_runtime tests —
  dormant classification, sidecar discovery, spawn+drop-kills-child — plus
  19 contract locks and 1 live ingestion E2E), 0 warnings. The spawn test is
  environment-aware: on a machine with the dev engine already answering it
  asserts External classification (no double-spawn) instead.
- `pnpm exec tsc --noEmit` + `pnpm build`: clean; preview checklist renders
  live probe rows.
- Real bundle attempt on this machine: `pnpm exec tauri build` (release
  profile, full Rust compile) — configuration validated end-to-end by the
  CLI; see Limitations for where artefacts land vs. CI.
- Engine suite untouched (24 pytest + ruff clean); only a new frozen-entry
  module was added.

### Limitations / known trade-offs

- Code signing/notarization needs an Apple Developer ID + secrets; builds
  are ad-hoc signed until then (Gatekeeper right-click → Open, documented).
- Windows NSIS artefacts are produced in CI only — no Windows machine in the
  local loop; the config is validated by the same schema the CI uses.
- The PyInstaller freeze is wired and scripted but a full frozen binary was
  not produced locally (dev engine runs via uv/screen); release.yml runs it
  on real runners per-OS.
- The sidecar listens on a fixed loopback port (8737); two simultaneous app
  instances share one engine (harmless — parse is stateless).

### Next

The master build plan is complete (Phases 0–9). Post-plan polish shipped
immediately after (see below), followed by chunked TTS synthesis; remaining
follow-up: signed/notarized release pipeline with real certificates.

## Post-plan polish — playback & storage sweep

- **In-app audio playback**: the Tauri asset protocol is enabled (with the
  matching `protocol-asset` Cargo feature) scoped to the managed audio
  folders (`$APPDATA/exports/*` and `$APPDATA/transcribe/*`) and the CSP
  gained `media-src asset:`. Narration results now render an inline
  `<audio>` player in the Audio tab (native builds; preview keeps the
  explanatory note), alongside Reveal.
- **Exports storage sweep** (Phase 6 trade-off resolved): `exports_stats`
  reports file count + total bytes; `delete_export` removes one file with a
  canonicalised containment check (refuses anything resolving outside the
  managed exports directory, `../` included). ExportsView shows the folder
  total and per-file Delete; new contract lock pins the wire shape.
- Verification: cargo battery green (93 lib + 20 locks + 1 E2E), engine
  untouched, tsc/build clean, and a full `tauri build` re-run produced the
  updated .app (9.93 MiB) + DMG (4.70 MiB) with the playback entitlements.

## Post-plan polish — chunked TTS synthesis

Long narrations (full-document read-aloud, 20-minute summaries) no longer run
through one giant synthesizer invocation. [tts.rs](../apps/desktop/src-tauri/src/services/tts.rs)
now packs scripts into sentence-boundary chunks of at most 220 words and
renders each chunk through its own one-shot piper/`say` process, then joins
the part WAVs into a single exact-size RIFF file (`concat_wavs`: header
walked strictly, sizes rewritten as true u32s, part headers stripped, partial
PCM frames refused). One MP3 transcode happens for the whole narration when
enabled — never per part. Short scripts keep the single-invocation fast path.

- Failure semantics: a chunk failure reports position ("Piper failed on
  chunk 2 of 3"), successful part files are cleaned up, part files from the
  failed run stay on disk for debugging, and the provider flags the chunked
  failure (`is_chunk_error`) for future retry/UI work.
- Durations are now exact for synthesized output: the RIFF header is parsed
  (data-chunk bytes ÷ byte rate) instead of estimating from file size; the
  size-based estimate remains the fallback. `say --data-format=LEF32` writes
  IEEE-float WAVs, so the parser accepts format tags 1 (PCM) and 3 (float).
- Tests: 6 new (chunker packing/losslessness, multi-chunk join with part
  cleanup, mid-run failure position + flag, exact 24 kHz concat header,
  non-strict-header fallback); existing tests now assert header-exact
  durations. Battery: 98 lib + 20 contract locks + 1 live E2E, 0 warnings;
  engine untouched; tsc/build clean.

## Post-plan polish — chunk retry + parts resume

Chunked synthesis (previous section) now recovers from failures instead of
throwing away rendered audio:

- **Per-chunk retry**: every chunk gets two automatic retries (1 s, 2 s
  backoff) for transient subprocess failures; the surfaced error keeps the
  chunk position. Deterministic problems (piper/model missing, `say`
  unresponsive) are caught by a preflight check before any chunk runs, so
  they neither burn retries nor touch the cache.
- **Parts resume**: rendered parts are tracked in `tts-parts.json` (written
  beside the parts). The manifest is keyed by a SHA-256 signature over
  script text + provider identity (voice model for piper, voice name for
  `say`; speech rate deliberately excluded — a speed change re-renders), so
  re-running the same narration resumes at the first missing chunk and a
  fully cached run skips synthesis entirely. Success clears the cache;
  vanished part files, a corrupt manifest, or a manifest longer than the
  current chunker produces are all treated as "no cache", never errors.
- **Concat safety**: part formats are now compared against the first part
  (format tag, channels, sample rate, bit depth) before splicing, and the
  output header propagates the real format tag instead of hardcoding PCM.
- Tests: 2 new — a counting fake piper proves retry+resume end-to-end
  (chunks 1–2 rendered once, chunk 3 exhausts 3 attempts, resume synthesizes
  only chunks 3–4, cache cleared on success) and a signature/cache unit
  test. Battery: 100 lib + 20 contract locks + 1 live E2E, 0 warnings;
  tsc/build clean.

## Post-release round — v0.1.0 hardening and performance

With the release shipped (private repo, 4/5 installers attached; the Intel
macOS build sits in GitHub's free-runner queue), three improvements landed:

- **Prompt-prefix caching for asks**: `build_prompt` now emits
  evidence-first, question-last prompts, so llama-server's prefix KV cache
  reuses the (large) evidence block across regenerate / follow-up asks /
  retries; the server is spawned with `--cache-reuse 256`. Per-ask telemetry
  (prompt/completion tokens, ms/token) is logged for verification
  (`researchai::ai` target). Documented in AI.md.
- **Live export progress**: `export_document` emits `exports://progress`
  events (preparing → rendering → writing, with elapsed seconds);
  ExportsView renders live status and auto-refreshes on completion. Wire
  shape pinned by a contract test.
- **Streaming asks**: the pipeline split into prepare/finish phases and a
  new `ai_ask_stream` command emits `ai://ask-delta` deltas while
  generating, persisting the identical AnalysisResponse at the end
  (ResearchView renders the live stream). Nothing is persisted until the
  stream completes.
- **Release dry-run dispatched** for its first live proof on all four
  targets (no-publish workflow, read-only token).

Verified after each change: cargo battery 123/123 (102 lib + 20 locks +
1 E2E), 0 warnings; tsc + vite clean. Limitations unchanged: speech and
LLM paths verified against fakes/canned servers pending real engines;
macOS builds ad-hoc signed until signing secrets are configured.

## Release round — v0.1.1 shipped, then a Voicebox design/skills study

**v0.1.1 released.** Version bumped across all 7 files (tauri.conf,
Cargo.toml/lock, two package.json, shared-types, pyproject), tag pushed,
release run 36812905404 completed with macos-14/windows/ubuntu assets
all correctly named `…_0.1.1_…` (4/4 attached; macos-13 stays queued on
free runners). An earlier bad cut that reused 0.1.0-named assets was
deleted and its run cancelled to free the release concurrency lock. The
release body — which the workflow ships empty — was backfilled from the
new CHANGELOG (`gh release edit v0.1.1 --notes-file …`, 1741 chars).

**Voicebox study → project design.** `github.com/jamiepine/voicebox`
(MIT) was cloned to a scratch dir outside this repo (spec §3) and mined
for transferable practice:

- **Four agent skills extracted** into `.agents/skills/`, rewritten for
  this repo's mechanics: `draft-release-notes` (CHANGELOG `[Unreleased]`
  workflow), `release-bump` (7-file bump, tag/watch/cleanup procedure,
  release-yml concurrency-lock recovery), `add-speech-engine` (our
  `TextToSpeechProvider`/`run_chunked`/fake-binary patterns + CI
  portability lessons), `triage-prs` (kept mostly intact; dormant until
  an external PR queue exists).
- **New `CHANGELOG.md`** (Keep a Changelog) seeded with stamped 0.1.0
  and 0.1.1 narratives — the target both release skills operate on.
- **Design concept extracted** into `docs/DESIGN.md`: token-pair
  discipline, radius scale, status-chip rules, focus/typography/density
  conventions, and the README imagery pattern (centered icon → tagline →
  badge row → screenshots). README now has the hero + badges and a
  Screenshots placeholder.
- **TTS splitter hardened** in `services/tts.rs`: `split_sentences` now
  skips abbreviation/initial/numeric periods (`Dr.`, `et al.`, `p.m.`,
  `J. Smith`, list markers) and a new `split_overlong_sentence` enforces
  the 220-word ceiling on pathological no-punctuation input by cutting at
  clause boundaries first — previously such input shipped as one
  over-length chunk. `CHUNKER_VERSION` was added to the parts-cache
  signature so cached parts can never splice across chunker changes.
- **Attribution**: THIRD_PARTY_LICENSES.md gained a "Studied reference
  projects" section (Voicebox/MIT, what landed where); skill headers and
  code comments credit the source.

Verified: cargo battery 125/125 (104 lib — 2 new splitter tests — + 20
locks + 1 E2E), 0 warnings; `tsc --noEmit` and vite build clean.
Known gaps unchanged: v0.1.1 assets are ad-hoc signed (Apple secrets
gated/ready), macos-13 still queued, real-hardware validation pending.

## Post-release round — release bodies from the CHANGELOG

v0.1.1 shipped with an **empty release body** (the workflow publishes
directly and never set one; it was backfilled by hand). The pipeline now
pulls release notes from the changelog instead:

- **`scripts/release/extract-notes.sh`** — extracts the stamped
  `## [X.Y.Z]` section for a tag from `CHANGELOG.md`. Stops at the next
  heading *and* at the reference-link footer, so a last-version section
  never leaks future-version links. Empty output = no such section.
- **`release.yml`** — new "Extract release notes from CHANGELOG" step
  feeds `body_path` on the attach action. Any extractor failure degrades
  to a generic CHANGELOG-pointer body with a `::warning::` — installers
  must never be blocked, and a release can never ship with empty notes.
- **`release-dry-run.yml`** — new fail-fast rehearsal step (first after
  checkout, before PyInstaller) asserts the newest stamped section
  extracts non-empty with no heading/footer bleed; the script and
  `CHANGELOG.md` are now dry-run path triggers.
- Docs: `release-bump` skill documents stamp-before-tag as mandatory and
  how to verify the body landed; OPERATIONS §3 corrected (release is
  published directly, not a draft) and now describes the body flow.

Verified locally: extraction for `0.1.1` (38 lines, clean), `0.1.0`
(footer excluded), missing version (empty → fallback path), plus both
workflows parse via `yaml.safe_load` with steps in position.

## Post-release round — document import bug hunt (folders, formats, drag-drop)

A user reported the app **would not accept documents**. Investigation
(evidence log + installed binary) showed single-PDF import actually worked —
but three real gaps sat around it, any of which could produce an import that
silently goes nowhere:

- **Folder import was broken end-to-end.** The picker's folder option
  returned a directory, which `validate_source` then rejected ("Not a
  regular file"). No directory walking existed anywhere in the pipeline.
- **The advertised format list outran the engine.** Rust's `SUPPORTED`
  const offered pptx/xlsx/epub (+ media) while the Python engine
  registered parsers only for pdf/docx/txt/md/html — those imports
  reached parse time and failed with "not supported yet". Media files
  aren't documents at all (transcription lives in AudioView).
- **Dead import affordances.** Tauri emitted `tauri://drag-drop` with no
  listener, the command-palette import item only toasted "lands in
  Phase 1", and the picker had no extension filter.

### Fixes

- `services/ingestion.rs`: `SUPPORTED` narrowed to the nine document
  formats, `expand_selection()` walks directories recursively
  (`MAX_WALK_DEPTH=12`, skips dotfiles, never follows symlinks) and
  returns `skipped_unsupported` alongside the files; 4 new tests cover
  media rejection, the allow-list, folder walking and mixed/absent paths.
- `commands/documents.rs`: `ImportSummary` gains `skipped`; per-file
  failures are logged without killing the batch; an empty selection
  reports the supported formats.
- `commands/system.rs`: picker filters to `SUPPORTED`;
  `commands/projects.rs`: `delete_project` purges its managed
  `documents/<project_id>/` copies (FK cascade removed rows only).
- Engine: new `parsers/office_extractors.py` — `parse_pptx`
  (slide-numbered sections from slide XML), `parse_xlsx` (shared strings
  + inline strings, sheet order from workbook.xml/rels, ` | `-joined rows
  as table blocks), `parse_epub` (container.xml→OPF→spine order, OPF
  title, chapters converted through the shared HTML→markdown path with
  rebased offsets); `_md_structure`/`_html_to_markdown` extracted into
  `text_extractors.py` for reuse.
- UI: `App.tsx` consumes drag-drop (routes to Library + import request,
  toast with project guidance when none is active), `DocumentsView`
  consumes dropped paths via a sequenced import-request bus,
  `CommandPalette` import actually imports, and the now-live title-bar
  search box seeds the Search view.
- Copy: stale "Phase 1/later phase" copy removed from Home/Notes/Library.

### Verification

- `cargo test`: **129/129 green, 0 warnings** (folder-walk, allow-list and
  import-summary tests included); `cargo check` clean.
- Engine: `pytest` green (incl. 8 new office-extraction tests and
  registry locks for pptx/xlsx/epub), `ruff check` clean.
- TS: `tsc --noEmit` clean; `vite build` clean.

## Post-release round — packaged sidecar runtime fix (v0.1.3)

Local acceptance testing of the **v0.1.2 DMG** caught a release-blocking
bug no CI job could see: the installed app's bundled engine died at exec.

- **Symptom**: app runs, `/health` never answers, log shows only the two
  startup lines; the supervisor's spawned child sits as a `<defunct>`
  zombie. The PyInstaller error: `Failed to load Python shared library
  …/sidecar/_internal/Python`.
- **Root cause**: the spec produces a **onedir** layout
  (`dist/researchai-engine/{researchai-engine,_internal/}`) but
  `collect-sidecar.mjs` copied **only the entry binary** into the Tauri
  resources — the entire 138 MB Python runtime was never bundled (the
  20 MB DMG was the tell). Masked until now because a dev engine on
  8737 answered the supervisor's probe as `External`.
- **Fixes**:
  - `collect-sidecar.mjs` copies the sibling `_internal/` runtime
    (and clears it on re-collect so it can never go stale).
  - `engine_runtime.rs` detects instant child death via `try_wait`
    during the health wait and logs it, plus a warning on the 30 s
    timeout — both were silent before.
  - Engine `__version__` now reads installed package metadata (dist-info
    bundled via `copy_metadata()` in the spec) instead of a hardcoded
    0.1.0; `/health` reports the true release version.
  - `.gitignore` covers collected sidecar + PyInstaller `build/`.
- **Verified locally**: frozen rebuild → collect → sidecar answers
  `/health` with `version 0.1.2`; cargo 129/129 (live E2E running
  against the frozen engine); engine ruff + pytest green.
- **Second root cause found while checking the first**: even with the
  runtime collected, `tauri.conf.json` mapped resources with
  `"packaging/resources/sidecar/*": "sidecar/"` — Tauri's `dir/*` glob
  is explicitly non-recursive ("sub-directories will be ignored"), so
  the v0.1.3 DMG CI built was still 20 MB. Map the directory itself
  (`"packaging/resources/sidecar/": "sidecar/"`) to copy recursively.
  Caught by comparing DMG sizes before shipping — 20 MB vs 69 MB.
- **End-to-end verified on the local release build**: installed app
  logs `bundled sidecar healthy (pid …)`, `/health` answers from the
  app-owned child process (no external engine on the port), app bundle
  174 MB / DMG 69 MB with the full `_internal/` tree (40/40 entries).

## Post-release round — bundled local AI stack (v0.1.4)

Request: ship the AI model *inside* the product — "comes with the
Products downloaded and install inside" — so Ask-AI and semantic search
work offline on first launch with zero setup.

- **Bundled assets (fetched at build time, never committed)**:
  `scripts/package/fetch-ai-assets.mjs` pins llama.cpp `b11370`
  per-OS CPU builds (smoke-tests `llama-server --version`; the macOS
  build is dylib-based so every extracted file must sit **flat** beside
  the binary) and the **Qwen3-0.6B-Q4_K_M** GGUF (396,705,472 B,
  Apache-2.0); `packaging/seed_embeddings.py` seeds
  **bge-small-en-v1.5** (MIT) via fastembed. `release.yml` gained two
  steps (Fetch bundled AI assets, Seed embedding model) before every
  installer build.
- **Tauri resources**: three recursive dir entries in `tauri.conf.json`
  (`sidecar/`, `llama/`, `models/`). tauri-build hard-fails when a
  declared path is missing, so the dirs are tracked via `.gitkeep` (the
  assets themselves are gitignored). HuggingFace downloads are
  owner-read-only; tauri-build's `fs::copy` preserves modes, so the
  second build truncating its own read-only copies dies with
  `Permission denied` — both fetchers now normalise to owner-writable
  (`chmodSync 0o644` / `_make_writable`). If it ever recurs:
  `rm -rf target/<profile>/{models,llama,sidecar}` and rebuild.
- **`services/bundled.rs`**: `resolve_llama_binary` (user path →
  bundled → actionable error) now used by ask/evidence/tts commands;
  `seed_bundled_models` runs before settings load — marker
  `.bundled-seeded`, copies the embeddings cache, imports the GGUF only
  into an empty model library, activates it and sets `ai_enabled` on
  first run only.
- **Embedding cache out of $TMPDIR**: engine reads
  `RESEARCHAI_MODELS_DIR` (set by the supervisor) and uses
  `<data>/models/embeddings` as the fastembed `cache_dir` — macOS no
  longer wipes the model on reboot (offline hashing-fallback gone).
- **First-run E2E (local release build installed to /Applications)**:
  log shows `seeded embedding model`, `seeded bundled model
  Qwen3-0.6B-Q4_K_M.gguf`, `bundled model active; AI enabled out of the
  box`, `using bundled llama-server: …/Resources/llama/llama-server`;
  live engine process has `RESEARCHAI_MODELS_DIR` in its env and the
  cache lives in the data dir; `/health` OK.
- **Context-budget fix found by E2E**: the first real Ask failed with
  `request (4390 tokens) exceeds the available context size (4096)` —
  llama-server rejects the *whole* request. `prepare_ask` now budgets
  the prompt to `(context − max_tokens − 128) × 4 chars`, drops
  excerpts from the end (last resort: truncate the first) and surfaces
  a "Context window: kept N of M excerpts…" warning instead of an
  error; default `context_size` 4096 → **8192** (stored 4096 installs
  are covered by the budget).
- **Live ask proof** with the bundled server, app-identical flags
  (`--ctx-size 4096 -fa off --cache-reuse 256`): an oversized prompt
  (17,523 tok) is rejected exactly as before the fix (the old failure
  mode); a budgeted prompt (10,482 chars ≤ 11,776 budget) returns a
  **cited answer `[1][2][3]` in 3.8 s**. Thinking mode stays on: with
  `enable_thinking: false` the model stopped citing entirely; an empty
  answer only appeared at `max_tokens 256` (thinking ate the budget) —
  the app default is 1024.
- **Local DMG gotcha**: `bundle_dmg.sh` fails at the final
  `hdiutil detach` with `Resource busy` — Finder/Spotlight hold the
  volume for a few seconds after the AppleScript `.DS_Store` dance, and
  the script only retries exit 16. Transient: plain detach succeeds
  seconds later; finish locally by detaching and running
  `hdiutil convert rw.*.dmg -format UDZO -o <name>.dmg`. CI runners
  were the real test — the v0.1.4 legs went green on every platform.
- **Release**: v0.1.4 published from tag with all three active CI legs
  green (macos-13 Intel remains perpetually queued — ignored); the new
  fetch/seed steps ran on macOS, Ubuntu and Windows alike (the Windows
  asset path concern is closed). Assets ship the AI stack — DMG
  674,722,532 B, x64-setup.exe 508,498,161 B, AppImage 614,279,672 B,
  deb 689,546,680 B — each with a published sha256 digest, and the
  release body was extracted from the stamped `[0.1.4]` section.
- **DMG verified**: 678,788,655 B (≈647 MB, matches the chosen "full
  bundle" size), attaches read-only, `.app` 812 MB with
  `Resources/{sidecar,llama×60,models/{Qwen3…gguf,embeddings}}`.
- **Battery**: cargo **135** green (114 lib incl. the new trim test +
  5 bundled tests, 20 contract, 1 live E2E), `tsc` clean, engine
  `ruff` + `pytest` **36** green.
