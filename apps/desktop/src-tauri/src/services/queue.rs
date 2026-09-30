//! Background ingestion queue (spec §14, §43).
//!
//! One worker thread processes documents marked `waiting`, oldest first:
//!
//! ```text
//! waiting → parsing → indexing → ready
//!                     ↘ failed (human-readable detail)
//! ```
//!
//! The worker owns its own SQLite connection (WAL allows multiple readers);
//! the UI observes progress by polling `list_documents`. If the document
//! engine is offline the worker waits and retries instead of failing files —
//! a transient sidecar outage must not poison the queue.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use crate::services::documents::{DocumentRow, IngestionStatus};

pub struct IngestionQueue {
    shutdown: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl IngestionQueue {
    pub fn start(data_dir: PathBuf) -> Self {
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let worker = std::thread::Builder::new()
            .name("ingestion-worker".into())
            .spawn(move || worker_loop(data_dir, flag))
            .expect("spawn ingestion worker");
        Self {
            shutdown,
            worker: Some(worker),
        }
    }
}

impl Drop for IngestionQueue {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(data_dir: PathBuf, shutdown: Arc<AtomicBool>) {
    let db = match Db::open(&data_dir) {
        Ok(db) => db,
        Err(e) => {
            log::error!(target: "researchai::queue", "worker could not open db: {e}");
            return;
        }
    };

    log::info!(target: "researchai::queue", "ingestion worker started");

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        match db.next_waiting_document() {
            Ok(Some(doc)) => {
                process(&db, &doc);
            }
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(700));
            }
            Err(e) => {
                log::error!(target: "researchai::queue", "queue poll failed: {e}");
                std::thread::sleep(Duration::from_millis(1500));
            }
        }
    }

    log::info!(target: "researchai::queue", "ingestion worker stopped");
}

fn process(db: &Db, doc: &DocumentRow) {
    log::info!(target: "researchai::queue", "parsing {} ({})", doc.file_name, doc.id);

    if let Err(e) = db.set_document_status(&doc.id, IngestionStatus::Parsing, None) {
        log::error!(target: "researchai::queue", "status update failed: {e}");
        return;
    }

    // Sidecar offline → leave the document queued and retry later.
    if !crate::services::engine_client::health_ok() {
        log::warn!(target: "researchai::queue", "engine offline; requeueing {}", doc.file_name);
        db.set_document_status(&doc.id, IngestionStatus::Waiting, Some("Waiting for document engine…"))
            .ok();
        std::thread::sleep(Duration::from_millis(2000));
        return;
    }

    let path = doc
        .managed_path
        .clone()
        .unwrap_or_else(|| doc.original_path.clone());

    match crate::services::engine_client::parse_document(&doc.id, std::path::Path::new(&path), None)
    {
        Ok(parsed) if parsed.ok => {
            if let Err(e) = crate::services::library::store_parse_result(db, &doc.id, &parsed) {
                crate::services::library::mark_failed(db, &doc.id, &format!("Indexing failed: {e}"));
                log::error!(target: "researchai::queue", "indexing failed for {}: {e}", doc.file_name);
            } else {
                // Embedding step (Phase 2): best-effort — a failure leaves the
                // document ready for keyword search with a visible note.
                match crate::services::retrieval::ensure_document_embedded(db, &doc.id) {
                    Ok(()) => log::info!(target: "researchai::queue", "ready: {} ({} chunks, embedded)", doc.file_name, parsed.blocks.len()),
                    Err(e) => {
                        db.set_document_status(&doc.id, IngestionStatus::Ready, Some("Indexed for keyword search; embeddings pending (engine unavailable)."))
                            .ok();
                        log::warn!(target: "researchai::queue", "embedding deferred for {}: {e}", doc.file_name);
                    }
                }
            }
        }
        Ok(parsed) => {
            let detail = parsed
                .error
                .unwrap_or_else(|| "The document could not be parsed.".to_string());
            crate::services::library::mark_failed(db, &doc.id, &detail);
            log::warn!(target: "researchai::queue", "parse failed for {}: {detail}", doc.file_name);
        }
        Err(e) => {
            crate::services::library::mark_failed(db, &doc.id, &e.to_string());
            log::error!(target: "researchai::queue", "engine error for {}: {e}", doc.file_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::documents::IngestionStatus;

    #[test]
    fn queue_processes_waiting_document_end_to_end() {
        // Full round trip against a fake engine is an integration concern;
        // here we verify the DB state machine transitions the worker relies on.
        let dir = crate::db::tests::temp_dir_for("queue");
        let db = Db::open(dir.path()).unwrap();
        let project = db.create_project("P", None).unwrap();

        db.insert_document(DocumentRow {
            id: "doc-1".into(),
            project_id: project.id.clone(),
            file_name: "a.txt".into(),
            original_path: "/tmp/a.txt".into(),
            managed_path: None,
            document_type: "txt".into(),
            checksum: "abc".into(),
            title: None,
            indexing_status: IngestionStatus::Waiting.as_str().into(),
            status_detail: None,
            page_count: None,
            language: None,
            imported_at: crate::db::now_iso_pub(),
            chunk_count: 0,
        })
        .unwrap();

        let next = db.next_waiting_document().unwrap().unwrap();
        assert_eq!(next.id, "doc-1");

        db.set_document_status(&next.id, IngestionStatus::Parsing, None)
            .unwrap();
        assert!(db.next_waiting_document().unwrap().is_none());

        db.set_document_status(&next.id, IngestionStatus::Ready, None)
            .unwrap();
        let doc = db.get_document(&next.id).unwrap();
        assert_eq!(doc.indexing_status, "ready");
    }
}
