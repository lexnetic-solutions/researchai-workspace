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
