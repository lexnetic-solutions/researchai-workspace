# RAG Design

How ResearchAI turns a question into a source-grounded answer.

## Status

**Phase 2 implemented:** FTS5 keyword channel, local embeddings
(fastembed BGE-small-en-v1.5, 384-dim, with a deterministic hashing fallback),
sqlite-vec cosine KNN behind the `VectorStore` interface, reciprocal-rank
fusion, near-duplicate suppression, per-document diversity caps, and a
retrieval debug panel in the UI. See [DATABASE.md](DATABASE.md) for the
`chunks_fts` / `chunk_embeddings` schema.

## Retrieval flow (spec §16)

```text
question
  → query parsing
  → keyword retrieval (SQLite FTS5)
  + vector retrieval  (embeddings + sqlite-vec)
  → merge
  → rerank
  → deduplicate (near-identical chunks)
  → source diversity check
  → evidence context
```

## Interfaces

- **`VectorStore`** ([retrieval.rs](../apps/desktop/src-tauri/src/services/retrieval.rs)) —
  wraps *all* sqlite-vec behaviour (spec §4.5): upsert, knn_search, stats.
  sqlite-vec is pre-v1; nothing outside this module may call vec_* functions
  or touch `chunk_embeddings`. Vectors are little-endian f32 blobs; KNN is
  brute-force `vec_distance_cosine` (fast at library scale, swappable for a
  vec0 virtual table later without changing callers).
- **`EmbeddingProvider`** — served by the document engine (`/embeddings`,
  `/embeddings/status`); the model is replaceable and the active engine is
  reported honestly in the UI (fastembed vs hashing fallback).
- **`AIProvider`** — `NoAIProvider | LlamaCppProvider | CloudProvider` (spec §17)
  with `health_check, list_models, load_model, unload_model, generate,
  stream_generate, estimate_context, stop_generation`. Not yet implemented.

## Hardware profiles (spec §2.4)

| Profile | RAM | Embedding | LLM class | Context |
|---|---|---|---|---|
| LIGHT | 8 GB | small (e.g. MiniLM-class, quantised) | 1–4B Q4 | conservative |
| STANDARD | 16 GB | compact | 7–8B Q4 | larger |
| ADVANCED | 32 GB+ | compact | 13B+ | deeper synthesis |

Profile detection already exists in
[hardware.rs](../apps/desktop/src-tauri/src/services/hardware.rs).

## Evidence rules

1. Every claim in an answer carries a citation to `document_id + page +
   chunk offsets` where available (spec §22).
2. Answer blocks are typed: `source-claim | source-evidence | ai-synthesis |
   ai-critique | user-note` (spec §21) — never visually blended.
3. Evidence strength is classified, not scored: `Direct / Multiple-source
   support / Indirect / Conflicting / AI inference / Insufficient evidence`
   (spec §23).
4. No-AI mode: all retrieval (keyword + vector) works without any LLM; AI-only
   steps are marked in the UI (spec §35).

## Debuggability

Phase 2 adds a retrieval debugging panel showing the fused candidate list,
scores, rerank order and final selection — essential for trusting local
semantic search.
