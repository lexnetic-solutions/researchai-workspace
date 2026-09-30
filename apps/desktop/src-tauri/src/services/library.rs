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

    // Bibliographic hints from the engine (Phase 5, spec §31): fill EMPTY
    // fields only — user corrections are authoritative and never overwritten.
    if parsed.doi.is_some() || parsed.year.is_some() {
        db.apply_bibliographic_hints(document_id, parsed.doi.as_deref(), parsed.year)?;
    }

    db.set_document_status(document_id, IngestionStatus::Ready, None)?;
    Ok(())
}

/// Mark a document failed with a human-readable detail (spec §42).
pub fn mark_failed(db: &Db, document_id: &str, detail: &str) {
    let _ = db.set_document_status(document_id, IngestionStatus::Failed, Some(detail));
}

/// Persist a finished transcription as a real `transcript` document
/// (Phase 7): document row plus timestamped chunks, transactionally.
///
/// Chunks are one `[mm:ss]`-marked block per [`TranscriptionSegment`] group,
/// stored through the same writer the engine parse path uses — so every
/// transcript automatically lands in FTS keyword search, and the caller can
/// embed it for hybrid retrieval via `retrieval::ensure_document_embedded`.
/// Bibliography skips transcripts (unknown ref types fall back to a plain
/// non-punctuated line rather than a fake journal article).
pub fn save_transcription(
    db: &Db,
    project_id: &str,
    audio: &Path,
    result: &crate::services::transcription::TranscriptionResult,
) -> AppResult<String> {
    use crate::services::transcription::group_segments;

    if result.segments.is_empty() {
        return Err(AppError::msg(
            "The transcription contained no speech — nothing to save.",
        ));
    }

    let checksum = crate::services::ingestion::hash_file(audio)?;
    if let Some(existing) = db.find_document_by_checksum(project_id, &checksum)? {
        return Ok(existing.id);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let file_name = audio
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio".to_string());
    let ext = audio
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("audio")
        .to_ascii_lowercase();

    let title = format!(
        "Transcript: {}",
        file_name.trim_end_matches(&format!(".{ext}")).to_string()
    );
    let minutes = result.duration_ms / 60_000;
    let status_detail = format!(
        "Local transcription · {} min{}",
        minutes,
        result
            .language
            .as_deref()
            .map(|l| format!(" · {l}"))
            .unwrap_or_default()
    );

    db.insert_document(crate::services::documents::DocumentRow {
        id: id.clone(),
        project_id: project_id.to_string(),
        file_name: file_name.clone(),
        original_path: audio.to_string_lossy().to_string(),
        managed_path: None, // transcript is derived from the audio file
        document_type: "transcript".into(),
        checksum,
        title: Some(title),
        authors: None,
        year: None,
        doi: None,
        journal: None,
        volume: None,
        issue: None,
        pages: None,
        publisher: None,
        url: None,
        ref_type: "transcript".into(),
        indexing_status: IngestionStatus::Indexing.as_str().to_string(),
        status_detail: Some(status_detail.clone()),
        page_count: None,
        language: result.language.clone(),
        imported_at: crate::db::now_iso_pub(),
        chunk_count: 0,
    })?;

    db.replace_sections_and_chunks(
        &id,
        &crate::services::documents::EngineParseResponse {
            document_id: id.clone(),
            ok: true,
            page_count: None,
            language: result.language.clone(),
            title: None,
            sections: vec![],
            blocks: group_segments(&result.segments, 1200)
                .into_iter()
                .map(|c| crate::services::documents::EngineBlock {
                    section_index: 0,
                    kind: "transcript".into(),
                    text: c.text,
                    page: None,
                    start_offset: Some(c.start_ms.min(i64::MAX as u64) as i64),
                    end_offset: Some(c.end_ms.min(i64::MAX as u64) as i64),
                })
                .collect(),
            error: None,
            doi: None,
            year: None,
        },
    )?;
    db.set_document_status(&id, IngestionStatus::Ready, Some(&status_detail))?;
    Ok(id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;
    use crate::services::transcription::TranscriptionSegment;

    /// Minimal 16 kHz mono WAV so checksumming has real bytes to hash.
    fn write_wav(dir: &Path, name: &str) -> std::path::PathBuf {
        let mut v = Vec::new();
        let data = vec![0u8; 1600];
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data.len() as u32).to_le_bytes()));
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&16_000u32.to_le_bytes());
        v.extend_from_slice(&32_000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&data);
        let p = dir.join(name);
        std::fs::write(&p, v).unwrap();
        p
    }

    #[test]
    fn transcription_becomes_searchable_document() {
        let dir = TempDir::new_with_label("lib-stt");
        let db = Db::open(dir.path()).unwrap();
        let project = db.create_project("Seminar", None).unwrap();
        let wav = write_wav(dir.path(), "lecture-01.wav");
        let result = crate::services::transcription::TranscriptionResult {
            segments: vec![
                TranscriptionSegment {
                    start_ms: 0,
                    end_ms: 8_400,
                    text: "Welcome to coastal hydrodynamics.".into(),
                },
                TranscriptionSegment {
                    start_ms: 8_400,
                    end_ms: 15_200,
                    text: "Today we derive the shallow water equations.".into(),
                },
            ],
            language: Some("en".into()),
            duration_ms: 15_200,
        };

        let doc_id = save_transcription(&db, &project.id, &wav, &result).unwrap();
        let doc = db.get_document(&doc_id).unwrap();
        assert_eq!(doc.document_type, "transcript");
        assert_eq!(doc.ref_type, "transcript");
        assert_eq!(doc.indexing_status, "ready");
        assert_eq!(doc.chunk_count, 1); // short segments group into one chunk
        assert!(doc.title.unwrap().starts_with("Transcript: "));
        assert!(doc.status_detail.unwrap().contains("Local transcription"));

        // Chunk text carries the [mm:ss] markers.
        let text = db.get_document_text(&doc_id).unwrap();
        assert!(text.contains("[00:00]"), "{text}");
        assert!(text.contains("[00:08]"), "{text}");
        assert!(text.contains("shallow water equations"), "{text}");

        // Re-transcribing the same audio dedupes to the existing transcript.
        let again = save_transcription(&db, &project.id, &wav, &result).unwrap();
        assert_eq!(again, doc_id);
    }
}
