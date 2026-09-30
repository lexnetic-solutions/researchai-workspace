//! Library service: document import (managed copy + checksum dedup),
//! document queries, and persistence of parse results (spec §13, §14).

use std::path::Path;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::documents::{EngineParseResponse, IngestionStatus};

pub struct ImportedDocument {
    pub id: String,
    pub file_name: String,
    pub duplicated: bool,
}

/// Import one file into a project: managed copy under the workspace,
/// checksum duplicate detection, DB row with `waiting` status.
///
/// Returns the existing document when the checksum already exists in the
/// project (duplicate import is a no-op, not an error).
pub fn import_file(
    db: &Db,
    workspace_root: &Path,
    project_id: &str,
    source: &Path,
    mode: &str,
) -> AppResult<ImportedDocument> {
    crate::services::storage::Workspace::validate_source(source)?;
    if !crate::services::ingestion::is_supported(source) {
        return Err(AppError::msg(format!(
            "Unsupported file type: {}",
            source.display()
        )));
    }

    let checksum = crate::services::ingestion::hash_file(source)?;

    if let Some(existing) = db.find_document_by_checksum(project_id, &checksum)? {
        return Ok(ImportedDocument {
            id: existing.id,
            file_name: existing.file_name,
            duplicated: true,
        });
    }

    let id = uuid::Uuid::new_v4().to_string();
    let file_name = source
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();

    // MANAGED COPY: copy into the workspace under documents/<project>/.
    // LINK ORIGINAL: keep the original path; managed_path stays NULL.
    let managed_path = if mode == "link-original" {
        None
    } else {
        let dest = workspace_root
            .join("documents")
            .join(project_id)
            .join(format!("{checksum}.{ext}"));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, &dest)?;
        Some(dest.to_string_lossy().to_string())
    };

    let document_type = ext.clone();
    let mime = mime_for(&ext);

    db.insert_document(crate::services::documents::DocumentRow {
        id: id.clone(),
        project_id: project_id.to_string(),
        file_name: file_name.clone(),
        original_path: source.to_string_lossy().to_string(),
        managed_path: managed_path.clone(),
        document_type: document_type.clone(),
        checksum,
        title: None,
        indexing_status: IngestionStatus::Waiting.as_str().to_string(),
        status_detail: None,
        page_count: None,
        language: None,
        imported_at: crate::db::now_iso_pub(),
        chunk_count: 0,
    })?;

    let _ = mime; // stored in a later metadata pass
    Ok(ImportedDocument {
        id,
        file_name,
        duplicated: false,
    })
}

/// Persist a successful parse result: document metadata, sections, chunks.
pub fn store_parse_result(
    db: &Db,
    document_id: &str,
    parsed: &EngineParseResponse,
) -> AppResult<()> {
    db.update_document_metadata(
        document_id,
        parsed.title.as_deref(),
        parsed.page_count,
        parsed.language.as_deref(),
        IngestionStatus::Indexing,
    )?;
    db.replace_sections_and_chunks(document_id, parsed)?;
    db.set_document_status(document_id, IngestionStatus::Ready, None)?;
    Ok(())
}

/// Mark a document failed with a human-readable detail (spec §42).
pub fn mark_failed(db: &Db, document_id: &str, detail: &str) {
    let _ = db.set_document_status(document_id, IngestionStatus::Failed, Some(detail));
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "html" | "htm" => "text/html",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "epub" => "application/epub+zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}
