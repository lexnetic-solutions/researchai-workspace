//! Live end-to-end ingestion test (real engine, real DB, real files).
//!
//! Requires the document engine running on 127.0.0.1:8737:
//!
//! ```bash
//! pnpm engine:run &          # or: cd services/document-engine && uv run uvicorn ...
//! cargo test --test live_ingestion
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use researchai_lib::db::Db;
use researchai_lib::services::documents::IngestionStatus;
use researchai_lib::services::{engine_client, library};

/// Unique scratch dir per run; removed on drop.
struct TempDir(PathBuf);
impl TempDir {
    fn new(label: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "researchai-live-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = <root>/apps/desktop/src-tauri → three levels up
    // is the repository root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/phase1")
}

#[test]
fn live_import_parse_store_roundtrip() {
    // Skip loudly (not silently) when the sidecar is not running.
    if !engine_client::health_ok() {
        panic!(
            "document engine is not running on 127.0.0.1:8737 — start it with `pnpm engine:run`"
        );
    }

    let dir = TempDir::new("live");
    let db = Db::open(dir.path()).expect("db");
    let project = db.create_project("Live smoke", None).expect("project");
    let workspace = dir.path().to_path_buf();

    // -- import markdown fixture (managed copy) ------------------------------
    let md = fixtures_dir().join("sample.md");
    let imported = library::import_file(&db, &workspace, &project.id, &md, "managed-copy")
        .expect("import md");
    assert!(!imported.duplicated);
    assert!(imported.id != "");

    // Managed copy must exist on disk under documents/<project>/.
    let doc = db.get_document(&imported.id).expect("doc row");
    let managed = doc.managed_path.expect("managed copy path");
    assert!(PathBuf::from(&managed).is_file(), "managed copy missing: {managed}");
    assert!(managed.contains(&project.id));

    // -- parse via the real engine and store ---------------------------------
    let parsed = engine_client::parse_document(&imported.id, PathBuf::from(&managed).as_path(), None)
        .expect("engine parse");
    assert!(parsed.ok, "engine reported: {:?}", parsed.error);

    library::store_parse_result(&db, &imported.id, &parsed).expect("store parse");

    let done = db.get_document(&imported.id).expect("doc after parse");
    assert_eq!(done.indexing_status, IngestionStatus::Ready.as_str());
    assert!(done.chunk_count > 0, "expected chunks, got 0");

    let text = db.get_document_text(&imported.id).expect("text");
    assert!(text.contains("semantic chunking"), "assembled text missing body content");

    // -- duplicate detection --------------------------------------------------
    let again = library::import_file(&db, &workspace, &project.id, &md, "managed-copy")
        .expect("re-import same file");
    assert!(again.duplicated);
    assert_eq!(again.id, imported.id);

    // -- docx fixture parses into sections ------------------------------------
    let docx = fixtures_dir().join("sample.docx");
    let imported_docx = library::import_file(&db, &workspace, &project.id, &docx, "managed-copy")
        .expect("import docx");
    let docx_path = imported_docx_id_path(&db, &imported_docx.id);
    let parsed_docx =
        engine_client::parse_document(&imported_docx.id, PathBuf::from(&docx_path).as_path(), None)
            .expect("engine parse docx");
    assert!(parsed_docx.ok, "docx engine error: {:?}", parsed_docx.error);
    assert!(
        parsed_docx.sections.iter().any(|s| s.heading.contains("Study Design")),
        "docx headings missing"
    );

    // -- Phase 2: embeddings + hybrid search ----------------------------------
    let embedded = researchai_lib::services::retrieval::ensure_document_embedded(&db, &imported.id)
        .expect("embedding step");
    let _ = embedded; // 0 if already embedded by the same call earlier

    let store = researchai_lib::services::retrieval::VectorStore::new(&db).unwrap();
    let (total, embedded_count) = store.stats().unwrap();
    assert!(total > 0, "no chunks to embed");
    assert_eq!(total, embedded_count, "not all chunks embedded");

    // Direct KNN probe: the embedding of the fixtures' own sentence must
    // match one of its chunks at essentially zero distance.
    let (engine, vecs) = researchai_lib::services::retrieval::embedding_client::embed(
        &["Retrieval quality depends on semantic chunking and hybrid search.".to_string()],
    )
    .expect("embed probe");
    assert_eq!(engine.as_str(), "fastembed");
    let knn = store.knn_search(&vecs[0], 3).expect("knn");
    assert!(!knn.is_empty(), "knn returned nothing");
    assert!(
        knn[0].distance < 0.35,
        "nearest neighbour too far: {} (self-sentence should be ~0)",
        knn[0].distance
    );

    let result = researchai_lib::services::retrieval::search(&db, "semantic chunking", &[], 5)
        .expect("hybrid search");
    assert!(!result.hits.is_empty(), "hybrid search returned no hits");
    assert!(
        result.trace.vector_candidates > 0,
        "semantic channel returned no candidates"
    );
    assert!(
        result.hits.iter().any(|h| h.matched_by.contains(&"vector".to_string())),
        "semantic channel did not contribute — engine: {}",
        result.trace.embedding_engine
    );
    let top = &result.hits[0];
    assert_eq!(top.document_id, imported.id, "top hit from wrong document");
    assert!(
        top.page_number.is_some() || top.start_offset.is_some(),
        "hit lacks citation location"
    );
    assert!(!result.trace.embedding_coverage.is_empty());
}

/// Helper: managed path for a document id (keeps the test readable).
fn imported_docx_id_path(db: &Db, document_id: &str) -> String {
    db.get_document(document_id)
        .expect("docx row")
        .managed_path
        .expect("docx managed path")
}
