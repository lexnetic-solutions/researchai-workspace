//! Document commands: import, list, read, retry, delete, search (Phase 1+2 IPC).

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::services::documents::DocumentRow;
use crate::services::retrieval::SearchResponse;
use crate::state::AppState;

/// Hybrid library search (spec §16). `document_ids` empty = whole library.
#[tauri::command]
pub fn search_library(
    state: State<'_, AppState>,
    query: String,
    document_ids: Option<Vec<String>>,
    limit: Option<usize>,
) -> AppResult<SearchResponse> {
    crate::services::retrieval::search(
        &state.db,
        &query,
        document_ids.as_deref().unwrap_or(&[]),
        limit.unwrap_or(12),
    )
}

/// Embedding index status for the debug panel.
#[tauri::command]
pub fn retrieval_status(state: State<'_, AppState>) -> AppResult<RetrievalStatus> {
    let store = crate::services::retrieval::VectorStore::new(&state.db)?;
    let (total, embedded) = store.stats()?;
    Ok(RetrievalStatus {
        embedding_engine: crate::services::retrieval::embedding_client::status()
            .map(|e| e.as_str().to_string())
            .unwrap_or_else(|_| "unavailable".to_string()),
        total_chunks: total,
        embedded_chunks: embedded,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalStatus {
    pub embedding_engine: String,
    pub total_chunks: i64,
    pub embedded_chunks: i64,
}

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>, project_id: String) -> AppResult<Vec<DocumentRow>> {
    state.db.list_documents(&project_id)
}

#[tauri::command]
pub fn get_document_text(state: State<'_, AppState>, document_id: String) -> AppResult<String> {
    state.db.get_document_text(&document_id)
}

/// Import files/folders into a project. Non-fatal per-file problems are
/// reported in the returned summary; a file that stops the whole batch does
/// not exist — every failure is isolated (spec §14, §42).
///
/// Selected paths may be files *or* directories: folders are walked
/// recursively for supported documents (spec §14 folder import), with
/// unsupported files inside them counted as skipped rather than failed.
#[tauri::command]
pub fn import_documents(
    state: State<'_, AppState>,
    project_id: String,
    paths: Vec<String>,
    mode: String,
) -> AppResult<ImportSummary> {
    if paths.is_empty() {
        return Err(AppError::msg("No files were selected."));
    }

    let effective_mode = if mode == "link-original" {
        "link-original"
    } else {
        "managed-copy"
    };

    let selection = crate::services::ingestion::expand_selection(&paths);

    let mut imported = 0usize;
    let mut duplicates = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for source in &selection.files {
        match crate::services::library::import_file(
            &state.db,
            &state.data_dir,
            &project_id,
            source,
            effective_mode,
        ) {
            Ok(result) => {
                if result.duplicated {
                    duplicates += 1;
                    log::info!(target: "researchai::ingestion", "duplicate skipped: {}", result.file_name);
                } else {
                    imported += 1;
                    log::info!(target: "researchai::ingestion", "imported: {}", result.file_name);
                }
            }
            Err(e) => {
                let name = source
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| source.to_string_lossy().to_string());
                log::warn!(target: "researchai::ingestion", "import failed for {name}: {e}");
                errors.push(format!("{name}: {e}"));
            }
        }
    }

    // A folder that contained no importable document at all is a user-facing
    // condition, not a silent no-op.
    if selection.files.is_empty() {
        if selection.skipped_unsupported > 0 {
            return Err(AppError::msg(format!(
                "None of the selected items are supported documents. Supported: {}.",
                crate::services::ingestion::supported_list()
            )));
        }
        return Err(AppError::msg(format!(
            "No documents found in the selection. Supported: {}.",
            crate::services::ingestion::supported_list()
        )));
    }

    Ok(ImportSummary {
        imported,
        duplicates,
        skipped: selection.skipped_unsupported,
        errors,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub imported: usize,
    pub duplicates: usize,
    /// Files inside selected folders that are not supported document types.
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Requeue a failed document (spec §42 recoverability).
#[tauri::command]
pub fn retry_document(state: State<'_, AppState>, document_id: String) -> AppResult<()> {
    let doc = state.db.get_document(&document_id)?;
    if doc.indexing_status != "failed" {
        return Err(AppError::msg(
            "Only failed documents can be retried. This one is not in the failed state.",
        ));
    }
    // Verify a readable source exists before requeueing.
    let path = doc
        .managed_path
        .clone()
        .unwrap_or_else(|| doc.original_path.clone());
    if !std::path::Path::new(&path).is_file() {
        return Err(AppError::msg(
            "The document file is missing on disk and cannot be retried.",
        ));
    }
    state
        .db
        .set_document_status(&document_id, crate::services::documents::IngestionStatus::Waiting, None)?;
    Ok(())
}

#[tauri::command]
pub fn delete_document(state: State<'_, AppState>, document_id: String) -> AppResult<()> {
    state.db.delete_document(&document_id)?;
    log::info!(target: "researchai::documents", "deleted document {document_id}");
    Ok(())
}
