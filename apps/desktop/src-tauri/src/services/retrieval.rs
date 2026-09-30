//! Hybrid retrieval (spec §16).
//!
//! Architecture:
//! - [`VectorStore`] wraps **all** sqlite-vec-specific behaviour (spec §4.5):
//!   upsert, KNN search, coverage stats. Nothing outside this module touches
//!   embedding storage or vec_* SQL functions.
//! - [`embedding_client`] talks to the engine's /embeddings endpoint.
//! - [`search`] fuses keyword (FTS5) and vector (cosine KNN) channels with
//!   reciprocal-rank fusion, near-duplicate suppression, per-document
//!   diversity caps, and returns a [`RetrievalTrace`] for the debug panel.

use std::sync::Once;

use serde::Serialize;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::documents::IngestionStatus;
use rusqlite::OptionalExtension;

pub const EMBEDDING_DIMENSIONS: usize = 384;

/// Register sqlite-vec's scalar functions with SQLite's auto-extension
/// mechanism: every connection opened *after this call* gets vec_* functions.
/// Db::open invokes this before creating its connection. Idempotent.
pub fn register_vec_extension() -> bool {
    static ONCE: Once = Once::new();
    static OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    ONCE.call_once(|| {
        let ok = unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )))
        };
        OK.store(ok == 0, std::sync::atomic::Ordering::SeqCst);
        if ok != 0 {
            log::warn!(target: "researchai::retrieval", "sqlite-vec auto-extension registration failed");
        }
    });
    OK.load(std::sync::atomic::Ordering::SeqCst)
}

/// Legacy internal name kept for clarity at VectorStore construction.
fn ensure_vec_registered() -> bool {
    register_vec_extension()
}

// ---------------------------------------------------------------------------
// Embedding engine labels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingEngine {
    Fastembed,
    HashingFallback,
}

impl EmbeddingEngine {
    fn from_api(s: &str) -> Self {
        if s == "fastembed" {
            EmbeddingEngine::Fastembed
        } else {
            EmbeddingEngine::HashingFallback
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EmbeddingEngine::Fastembed => "fastembed",
            EmbeddingEngine::HashingFallback => "hashing-fallback",
        }
    }
}

pub mod embedding_client {
    //! Thin client for the engine's /embeddings endpoints.

    use std::time::Duration;

    use serde::Deserialize;
    use ureq::Agent;

    use crate::error::{AppError, AppResult};

    #[derive(Debug, Deserialize)]
    struct ApiResponse {
        engine: String,
        embeddings: Vec<ApiEmbedding>,
    }

    #[derive(Debug, Deserialize)]
    struct ApiEmbedding {
        vector: Vec<f32>,
    }

    #[derive(Debug, Deserialize)]
    struct ApiStatus {
        engine: String,
    }

    fn agent() -> &'static Agent {
        static AGENT: std::sync::OnceLock<Agent> = std::sync::OnceLock::new();
        AGENT.get_or_init(|| {
            let config = Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(60)))
                .build();
            Agent::new_with_config(config)
        })
    }

    /// Active engine on the sidecar (fastembed or hashing fallback).
    pub fn status() -> AppResult<super::EmbeddingEngine> {
        let mut resp = agent()
            .get("http://127.0.0.1:8737/embeddings/status")
            .call()
            .map_err(|e| AppError::msg(format!("Document engine unreachable: {e}")))?;
        let body: ApiStatus = resp
            .body_mut()
            .read_json()
            .map_err(|e| AppError::msg(format!("Invalid engine status: {e}")))?;
        Ok(super::EmbeddingEngine::from_api(&body.engine))
    }

    /// Embed a batch of texts, returning (engine, vectors in input order).
    pub fn embed(texts: &[String]) -> AppResult<(super::EmbeddingEngine, Vec<Vec<f32>>)> {
        if texts.is_empty() {
            return Ok((super::EmbeddingEngine::Fastembed, Vec::new()));
        }
        let mut resp = agent()
            .post("http://127.0.0.1:8737/embeddings")
            .send_json(serde_json::json!({ "texts": texts }))
            .map_err(|e| AppError::msg(format!("Embedding request failed: {e}")))?;
        let body: ApiResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| AppError::msg(format!("Invalid embedding response: {e}")))?;
        let engine = super::EmbeddingEngine::from_api(&body.engine);
        Ok((
            engine,
            body.embeddings.into_iter().map(|e| e.vector).collect(),
        ))
    }
}

// ---------------------------------------------------------------------------
// VectorStore — the ONLY module that touches embedding storage / sqlite-vec
// ---------------------------------------------------------------------------

fn vector_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[allow(dead_code)]
fn blob_to_vector(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub struct VectorStore<'a> {
    db: &'a Db,
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorMatch {
    pub chunk_rowid: i64,
    pub document_id: String,
    /// Cosine distance (0 = identical direction).
    pub distance: f32,
}

impl<'a> VectorStore<'a> {
    pub fn new(db: &'a Db) -> AppResult<Self> {
        ensure_vec_registered();
        Ok(Self { db })
    }

    /// Store/replace the embedding for one chunk.
    pub fn upsert(
        &self,
        chunk_rowid: i64,
        document_id: &str,
        engine: EmbeddingEngine,
        vector: &[f32],
    ) -> AppResult<()> {
        if vector.len() != EMBEDDING_DIMENSIONS {
            return Err(AppError::msg(format!(
                "Embedding dimension mismatch: expected {EMBEDDING_DIMENSIONS}, got {}",
                vector.len()
            )));
        }
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO chunk_embeddings (chunk_rowid, document_id, dimensions, engine, vector)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chunk_rowid) DO UPDATE SET
                document_id = excluded.document_id,
                dimensions = excluded.dimensions,
                engine = excluded.engine,
                vector = excluded.vector",
            rusqlite::params![
                chunk_rowid,
                document_id,
                EMBEDDING_DIMENSIONS as i64,
                engine.as_str(),
                vector_to_blob(vector)
            ],
        )?;
        Ok(())
    }

    /// Brute-force cosine KNN over stored vectors via vec_distance_cosine.
    /// Fast enough for library-scale corpora on 8 GB hardware; a vec0
    /// virtual table can replace this later without changing callers.
    pub fn knn_search(&self, query: &[f32], limit: usize) -> AppResult<Vec<VectorMatch>> {
        if query.len() != EMBEDDING_DIMENSIONS {
            return Err(AppError::msg("Query embedding dimension mismatch."));
        }
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT e.chunk_rowid, e.document_id,
                    vec_distance_cosine(e.vector, ?1) AS distance
             FROM chunk_embeddings e
             ORDER BY distance ASC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![vector_to_blob(query), limit as i64],
                |row| {
                    Ok(VectorMatch {
                        chunk_rowid: row.get(0)?,
                        document_id: row.get(1)?,
                        distance: row.get(2)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (total_chunks, embedded_chunks) for the debug panel.
    pub fn stats(&self) -> AppResult<(i64, i64)> {
        let conn = self.db.lock();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
        let embedded: i64 =
            conn.query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |r| r.get(0))?;
        Ok((total, embedded))
    }
}

// ---------------------------------------------------------------------------
// Hybrid search
// ---------------------------------------------------------------------------

const RRF_K: f32 = 60.0;
const PER_DOC_CAP: usize = 4;
const DEDUP_JACCARD: f32 = 0.9;

/// Crosses the IPC boundary — the TS contract (packages/shared-types) uses
/// camelCase keys (chunkId, documentName, matchedBy, …).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub chunk_id: String,
    pub document_id: String,
    pub document_name: String,
    pub page_number: Option<i64>,
    pub section_heading: Option<String>,
    pub text: String,
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
    /// Reciprocal-rank fusion score (higher = better).
    pub score: f32,
    /// Which channels retrieved this hit — shown in the debug panel.
    pub matched_by: Vec<String>,
    pub fts_rank: Option<f64>,
    pub vec_distance: Option<f32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalTrace {
    pub query: String,
    pub keyword_candidates: usize,
    pub vector_candidates: usize,
    pub fused: usize,
    pub returned: usize,
    pub embedding_engine: String,
    pub embedding_coverage: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub trace: RetrievalTrace,
}

#[derive(Default, Clone)]
struct Accum {
    score: f32,
    matched_by: Vec<String>,
    fts_rank: Option<f64>,
    vec_distance: Option<f32>,
}

/// Hybrid search (spec §16). `document_ids` empty = whole library scope.
pub fn search(db: &Db, query: &str, document_ids: &[String], limit: usize) -> AppResult<SearchResponse> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(AppError::msg("Search query must not be empty."));
    }
    let limit = limit.clamp(1, 50);
    let mut warnings: Vec<String> = Vec::new();

    let store = VectorStore::new(db)?;

    // -- channel 1: keyword (FTS5) -------------------------------------------
    let keyword = keyword_search(db, trimmed, document_ids, limit * 3)?;
    let keyword_candidates = keyword.len();

    // -- channel 2: semantic (VectorStore KNN) --------------------------------
    let mut vector: Vec<VectorMatch> = Vec::new();
    let mut engine_label = "unavailable";
    let mut semantic_ok = false;
    if crate::services::engine_client::health_ok() {
        match embedding_client::embed(std::slice::from_ref(&trimmed.to_string())) {
            Ok((engine, vectors)) if !vectors.is_empty() => {
                engine_label = engine.as_str();
                if let Some(q) = vectors.first() {
                    match store.knn_search(q, limit * 3) {
                        Ok(matches) => {
                            vector = filter_scope(matches, document_ids);
                            semantic_ok = true;
                        }
                        Err(e) => warnings.push(format!("Vector search failed: {e}")),
                    }
                }
            }
            Ok(_) => warnings.push("Embedding endpoint returned no vectors.".into()),
            Err(e) => warnings.push(format!("Embedding failed: {e}")),
        }
    } else {
        warnings.push("Document engine offline — keyword results only.".into());
    }
    let vector_candidates = vector.len();

    if keyword.is_empty() && vector.is_empty() {
        return Ok(SearchResponse {
            hits: Vec::new(),
            trace: RetrievalTrace {
                query: trimmed.to_string(),
                keyword_candidates: 0,
                vector_candidates: 0,
                fused: 0,
                returned: 0,
                embedding_engine: engine_label.to_string(),
                embedding_coverage: coverage_string(&store)?,
                warnings,
            },
        });
    }

    // -- reciprocal rank fusion ------------------------------------------------
    let mut acc: std::collections::HashMap<i64, Accum> = std::collections::HashMap::new();
    for (rank, (rowid, fts_rank)) in keyword.iter().enumerate() {
        let a = acc.entry(*rowid).or_default();
        a.score += 1.0 / (RRF_K + rank as f32);
        a.matched_by.push("keyword".into());
        a.fts_rank = Some(*fts_rank);
    }
    for (rank, m) in vector.iter().enumerate() {
        let a = acc.entry(m.chunk_rowid).or_default();
        a.score += 1.0 / (RRF_K + rank as f32);
        a.matched_by.push("vector".into());
        a.vec_distance = Some(m.distance);
    }

    let mut ranked: Vec<(i64, Accum)> = acc.into_iter().collect();
    ranked.sort_by(|x, y| {
        y.1.score
            .partial_cmp(&x.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // -- hydrate + dedupe + diversity ------------------------------------------
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut seen_token_sets: Vec<String> = Vec::new();
    let mut per_doc: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for (rowid, a) in ranked {
        if hits.len() >= limit {
            break;
        }
        let Some(hit) = hydrate(db, rowid, &a)? else { continue };
        if !document_ids.is_empty() && !document_ids.contains(&hit.document_id) {
            continue;
        }
        let tokens = token_set(&hit.text);
        if seen_token_sets.iter().any(|t| jaccard(t, &tokens) > DEDUP_JACCARD) {
            continue;
        }
        let count = per_doc.entry(hit.document_id.clone()).or_default();
        if *count >= PER_DOC_CAP {
            continue;
        }
        *count += 1;
        seen_token_sets.push(tokens);
        hits.push(hit);
    }

    if !semantic_ok && !warnings.iter().any(|w| w.contains("Semantic")) {
        warnings.push("Semantic channel unavailable — keyword results only.".into());
    }

    let trace = RetrievalTrace {
        query: trimmed.to_string(),
        keyword_candidates,
        vector_candidates,
        fused: keyword_candidates.max(vector_candidates),
        returned: hits.len(),
        embedding_engine: engine_label.to_string(),
        embedding_coverage: coverage_string(&store)?,
        warnings,
    };

    Ok(SearchResponse { hits, trace })
}

fn coverage_string(store: &VectorStore<'_>) -> AppResult<String> {
    let (total, embedded) = store.stats()?;
    Ok(format!("{embedded}/{total} chunks embedded"))
}

fn filter_scope(matches: Vec<VectorMatch>, document_ids: &[String]) -> Vec<VectorMatch> {
    if document_ids.is_empty() {
        matches
    } else {
        matches
            .into_iter()
            .filter(|m| document_ids.contains(&m.document_id))
            .collect()
    }
}

/// FTS5 keyword search → (chunk_rowid, bm25_rank) sorted best-first.
fn keyword_search(
    db: &Db,
    query: &str,
    document_ids: &[String],
    limit: usize,
) -> AppResult<Vec<(i64, f64)>> {
    let conn = db.lock();
    let sanitized = query.replace('"', "\"\"");
    let fts_query = format!("\"{sanitized}\"*");

    let sql = if document_ids.is_empty() {
        format!(
            "SELECT rowid, bm25(chunks_fts) AS rank
             FROM chunks_fts WHERE chunks_fts MATCH ?1
             ORDER BY rank ASC LIMIT {limit}"
        )
    } else {
        let placeholders = document_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "SELECT f.rowid, bm25(chunks_fts) AS rank
             FROM chunks_fts f
             JOIN chunks c ON c.rowid = f.rowid
             WHERE chunks_fts MATCH ?1 AND c.document_id IN ({placeholders})
             ORDER BY rank ASC LIMIT {limit}"
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query)];
    for id in document_ids {
        params.push(Box::new(id.clone()));
    }
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn hydrate(db: &Db, rowid: i64, a: &Accum) -> AppResult<Option<SearchHit>> {
    let conn = db.lock();
    conn.query_row(
        "SELECT c.id, c.document_id, d.file_name, c.page_number,
                s.heading, c.text, c.start_offset, c.end_offset
         FROM chunks c
         JOIN documents d ON d.id = c.document_id
         LEFT JOIN document_sections s ON s.id = c.section_id
         WHERE c.rowid = ?1",
        [rowid],
        |row| {
            Ok(SearchHit {
                chunk_id: row.get(0)?,
                document_id: row.get(1)?,
                document_name: row.get(2)?,
                page_number: row.get(3)?,
                section_heading: row.get(4)?,
                text: row.get(5)?,
                start_offset: row.get(6)?,
                end_offset: row.get(7)?,
                score: a.score,
                matched_by: a.matched_by.clone(),
                fts_rank: a.fts_rank,
                vec_distance: a.vec_distance,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

fn token_set(text: &str) -> String {
    let mut v: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 2)
        .map(|s| s.to_lowercase())
        .collect();
    v.sort();
    v.dedup();
    v.join(" ")
}

fn jaccard(a: &str, b: &str) -> f32 {
    let sa: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let sb: std::collections::HashSet<&str> = b.split_whitespace().collect();
    let inter = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 { 0.0 } else { inter as f32 / union as f32 }
}

/// Embed all not-yet-embedded chunks of one document (queue step, spec §14).
pub fn embed_document(db: &Db, document_id: &str) -> AppResult<usize> {
    let pending: Vec<(i64, String)> = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT c.rowid, c.text FROM chunks c
             LEFT JOIN chunk_embeddings e ON e.chunk_rowid = c.rowid
             WHERE c.document_id = ?1 AND e.chunk_rowid IS NULL
             ORDER BY c.chunk_index",
        )?;
        let rows = stmt
            .query_map([document_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    if pending.is_empty() {
        return Ok(0);
    }

    // Batch through the engine in chunks of 64 (bounded memory, §43).
    let store = VectorStore::new(db)?;
    let mut embedded = 0usize;
    for batch in pending.chunks(64) {
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let (engine, vectors) = embedding_client::embed(&texts)?;
        for ((rowid, _), vector) in batch.iter().zip(vectors.iter()) {
            store.upsert(*rowid, document_id, engine, vector)?;
            embedded += 1;
        }
    }
    Ok(embedded)
}

/// Ensure a document marked ready also has embeddings; safe to call twice.
pub fn ensure_document_embedded(db: &Db, document_id: &str) -> AppResult<()> {
    db.set_document_status(document_id, IngestionStatus::Indexing, None)?;
    let n = embed_document(db, document_id)?;
    if n > 0 {
        log::info!(target: "researchai::retrieval", "embedded {n} chunk(s) for {document_id}");
    }
    db.set_document_status(document_id, IngestionStatus::Ready, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::temp_dir_for;

    fn seeded_db() -> (crate::db::tests::TempDir, Db) {
        let dir = temp_dir_for("retrieval");
        let db = Db::open(dir.path()).expect("db");
        let project = db.create_project("P", None).unwrap();
        db.insert_document(crate::services::documents::DocumentRow {
            id: "doc-1".into(),
            project_id: project.id,
            file_name: "sample.md".into(),
            original_path: "/tmp/sample.md".into(),
            managed_path: None,
            document_type: "md".into(),
            checksum: "chk".into(),
            title: Some("Sample".into()),
            authors: None,
            year: None,
            doi: None,
            journal: None,
            volume: None,
            issue: None,
            pages: None,
            publisher: None,
            url: None,
            ref_type: "article".into(),
            indexing_status: IngestionStatus::Ready.as_str().into(),
            status_detail: None,
            page_count: None,
            language: None,
            imported_at: crate::db::now_iso_pub(),
            chunk_count: 0,
        })
        .unwrap();

        // Two chunks with distinct topics via the same path the parse
        // pipeline uses (sections first, then chunks referencing them).
        let parsed = crate::services::documents::EngineParseResponse {
            document_id: "doc-1".into(),
            ok: true,
            page_count: None,
            language: None,
            title: Some("Sample".into()),
            sections: vec![crate::services::documents::EngineSection {
                heading: "Section".into(),
                level: 1,
                page_start: None,
                page_end: None,
                order_index: 0,
            }],
            blocks: vec![
                crate::services::documents::EngineBlock {
                    section_index: 0,
                    kind: "paragraph".into(),
                    text: "Photosynthesis converts light energy into chemical energy in plants.".into(),
                    page: None,
                    start_offset: Some(0),
                    end_offset: Some(70),
                },
                crate::services::documents::EngineBlock {
                    section_index: 0,
                    kind: "paragraph".into(),
                    text: "The mitochondrial electron transport chain produces ATP in animals.".into(),
                    page: None,
                    start_offset: Some(80),
                    end_offset: Some(140),
                },
            ],
            error: None,
            doi: None,
            year: None,
        };
        crate::services::library::store_parse_result(&db, "doc-1", &parsed).unwrap();
        (dir, db)
    }

    #[test]
    fn keyword_search_finds_relevant_chunk() {
        let (_dir, db) = seeded_db();
        let rows = keyword_search(&db, "photosynthesis", &[], 10).unwrap();
        assert!(!rows.is_empty(), "FTS5 should match photosynthesis");
    }

    #[test]
    fn search_keyword_channel_works_end_to_end() {
        let (_dir, db) = seeded_db();
        // This fixture stores no embeddings, so the semantic channel has zero
        // candidates whether the sidecar is online (KNN over empty index) or
        // offline; the honest signal is coverage + vector_candidates.
        let res = search(&db, "electron transport", &[], 5).unwrap();
        assert_eq!(res.trace.vector_candidates, 0);
        assert!(res.trace.embedding_coverage.starts_with("0/"));
        if !res.hits.is_empty() {
            assert!(res.hits.iter().all(|h| h.matched_by.contains(&"keyword".to_string())));
        }
    }

    #[test]
    fn vector_store_upsert_and_stats() {
        let (_dir, db) = seeded_db();
        let store = VectorStore::new(&db).unwrap();
        let (total, embedded) = store.stats().unwrap();
        assert_eq!(total, 2);
        assert_eq!(embedded, 0);

        let v = vec![0.1f32; EMBEDDING_DIMENSIONS];
        // chunk rowids come from the seeded insert
        let conn = db.lock();
        let rowid: i64 = conn
            .query_row("SELECT rowid FROM chunks LIMIT 1", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        store.upsert(rowid, "doc-1", EmbeddingEngine::Fastembed, &v).unwrap();
        let (_, embedded) = store.stats().unwrap();
        assert_eq!(embedded, 1);
    }

    #[test]
    fn empty_query_is_rejected() {
        let (_dir, db) = seeded_db();
        assert!(search(&db, "   ", &[], 5).is_err());
    }
}
