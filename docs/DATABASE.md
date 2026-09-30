# Database

Single SQLite database: `research.db` inside the managed workspace directory.
WAL journal mode; foreign keys ON. Accessed only by the Rust core.

## Migrations

Versioned, append-only, applied at startup by
[db.rs](../apps/desktop/src-tauri/src/db.rs). Each entry is idempotent and
recorded in `schema_migrations`. Crash-safe: migrations run in one transaction
batch per version; a partially-applied version rolls back. Never edit an
applied migration — add a new one.

## Phase 0 schema (migration 1)

```sql
projects(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  created_at TEXT NOT NULL,   -- RFC 3339, UTC
  updated_at TEXT NOT NULL
);
settings_kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

`settings_kv` is a typed-key store (theme, import mode, OCR toggle,
concurrency, AI enabled). Structured settings live in
[settings.rs](../apps/desktop/src-tauri/src/services/settings.rs).

## Phase 1 schema (migration 2 — implemented)

```sql
documents(id, project_id → projects ON DELETE CASCADE, file_name, original_path,
          managed_path, mime_type, document_type, checksum,
          UNIQUE(project_id, checksum), title, authors, year, doi,
          imported_at, indexing_status, status_detail, page_count, language);
document_sections(id, document_id → documents ON DELETE CASCADE, heading,
                  hierarchy_level, page_start, page_end, order_index);
chunks(id, document_id → documents ON DELETE CASCADE,
       section_id → document_sections ON DELETE SET NULL, page_number,
       chunk_index, text, start_offset, end_offset, embedding_id);
```

Indexes: `documents(project_id, imported_at DESC)`, `documents(project_id, checksum)`
(dedup), `document_sections(document_id, order_index)`, `chunks(document_id, chunk_index)`.

`indexing_status` cycles `waiting → parsing → indexing → ready | failed`
(spec §14); `status_detail` carries a human-readable message for failures and
"waiting for engine" notices. Document deletion removes the managed copy file,
sections and chunks (cascades); originals on disk are never touched.

## Phase 2 schema (migration 3 — implemented)

```sql
chunks_fts USING fts5(
  text, content='chunks', content_rowid='rowid',
  tokenize='porter unicode61'
);
chunks_ai / chunks_au / chunks_ad — AFTER INSERT/UPDATE/DELETE triggers on
  chunks keeping the FTS index in sync (external-content delete protocol);
chunk_embeddings(
  chunk_rowid INTEGER PRIMARY KEY,   -- matches chunks.rowid; NO FK (rowid
                                     -- can't be an FK target; deletion
                                     -- cascades via document_id instead)
  document_id → documents ON DELETE CASCADE,
  dimensions INTEGER NOT NULL,       -- 384 (BAAI/bge-small-en-v1.5)
  engine TEXT NOT NULL,              -- e.g. "fastembed"
  vector BLOB NOT NULL               -- little-endian f32 vector
);
index_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

Indexes: `chunk_embeddings(document_id)`.

Design notes (see [RAG.md](RAG.md) for the retrieval layer above this):

- `chunks_fts` is an **external-content** FTS5 table — text lives only in
  `chunks`; the triggers above maintain index consistency on every chunk
  mutation (AI/AU/AD).
- sqlite-vec `vec0` virtual tables cannot be created before the extension is
  loaded, so vectors are stored in a plain table as BLOBs; KNN is a
  brute-force `ORDER BY vec_distance_cosine(vector, ?) ASC` query executed by
  the extension's SQL function. This is exact (not approximate) nearest
  neighbour — fine at library scale. A future migration can move to `vec0`.
- `index_metadata` records index-wide state (e.g. embedding dimensions,
  engine version) so a model change can be detected before querying.

## Phase 3 schema (migration 4 — implemented)

```sql
local_models(
  id, file_name, file_path UNIQUE,   -- GGUF inside <data>/models/
  size_bytes, sha256,                -- integrity: one hash per registration
  parameters, quantization,          -- parsed from the GGUF header
  context_tokens,                    -- from header when present
  status,                            -- available | missing | downloading | failed
  status_detail, source,             -- imported | downloaded
  added_at, last_used_at
);
analyses(
  id, project_id → projects ON DELETE CASCADE,
  document_id → documents ON DELETE SET NULL,   -- NULL for library-wide asks
  analysis_type,                                -- chat | research | quick_read | …
  model_id, prompt_version,                     -- prompt_version pins prompt wording
  question, answer_text,
  evidence_json,                                -- numbered excerpts used
  trace_json,                                   -- debug trace (tokens, citations…)
  created_at
);
```

Indexes: `local_models(status)`, `analyses(project_id, created_at DESC)`,
`analyses(document_id)`. Model registry rows never imply file presence —
`ModelManager::scan_for_missing` reconciles statuses with the filesystem
([AI.md](AI.md)).

## Phase 4 schema (migration 5 — implemented)

```sql
evidence_tables(
  id, project_id → projects ON DELETE CASCADE,
  question,
  scope_json,        -- JSON array of scoped document ids ([] = whole project)
  table_json,        -- full row layout: per-document excerpts + strengths
  trace_json,        -- debug trace (strength counts, warnings, duration)
  model_id,          -- NULL = deterministic table (no AI pass)
  prompt_version,    -- pins the AI prompt wording used
  created_at
);
```

Indexes: `evidence_tables(project_id, created_at DESC)`. The matrix layout is
stored as JSON (see [AI.md](AI.md) / [ARCHITECTURE.md](ARCHITECTURE.md) for
the shape): rows are per-document with evidence-strength labels derived from
the retrieval path, never a truth score.

## Planned schema (Phase 5+)

```sql
citations(id, document_id → documents, source_chunk_id, citation_metadata,
          bibliography_metadata);
notes(id, project_id, document_id, text, note_type, created_at, updated_at);
conversations(id, project_id, title, created_at);
messages(id, conversation_id → conversations, role, content, evidence_json,
         created_at);
audio_outputs(id, project_id, source_type, source_id, output_path, duration,
              voice, created_at);
```

Retrieval (Phase 2), local-AI (Phase 3) and evidence-matrix (Phase 4) tables
shipped above; later phases add citations, notes, conversations/messages and
audio outputs in the same database file. sqlite-vec is registered before the connection opens
([db.rs](../apps/desktop/src-tauri/src/db.rs)); the vector store sits behind
the `VectorStore` interface ([RAG.md](RAG.md)).

## Conventions

- Timestamps: RFC 3339 UTC strings (sortable, human-readable).
- IDs: UUIDv4 strings.
- Checksums: SHA-256 hex of file bytes ([ingestion.rs](../apps/desktop/src-tauri/src/services/ingestion.rs)) — used for duplicate detection.
- Deleting a project cascades to its documents, sections and chunks
  (`ON DELETE CASCADE`); the managed copy files of a project's documents are
  removed per-document by the Rust layer when documents are deleted
  individually. A project-level file purge lands with the storage manager
  hardening pass.
