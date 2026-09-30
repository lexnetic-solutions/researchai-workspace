/**
 * @researchai/retrieval-core — reserved for Phase 2:
 * hybrid retrieval (keyword FTS5 + semantic sqlite-vec), reranking,
 * deduplication and source-diversity checks (spec §16).
 *
 * sqlite-vec specifics stay behind the internal VectorStore interface
 * (spec §4.5) so the vector backend can be swapped later.
 */
export const RETRIEVAL_CORE_PLACEHOLDER = true as const;
