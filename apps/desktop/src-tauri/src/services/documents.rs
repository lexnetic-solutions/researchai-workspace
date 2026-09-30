//! Document records and parse results crossing the IPC and engine
//! boundaries (spec §12, §14).

use serde::{Deserialize, Serialize};

/// Ingestion lifecycle shown in the UI (spec §14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestionStatus {
    Waiting,
    Parsing,
    Indexing,
    Ready,
    Failed,
}

impl IngestionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestionStatus::Waiting => "waiting",
            IngestionStatus::Parsing => "parsing",
            IngestionStatus::Indexing => "indexing",
            IngestionStatus::Ready => "ready",
            IngestionStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "parsing" => IngestionStatus::Parsing,
            "indexing" => IngestionStatus::Indexing,
            "ready" => IngestionStatus::Ready,
            "failed" => IngestionStatus::Failed,
            _ => IngestionStatus::Waiting,
        }
    }
}

/// Crosses the IPC boundary — the TS contract (packages/shared-types) uses
/// camelCase keys (fileName, indexingStatus, statusDetail, …).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRow {
    pub id: String,
    pub project_id: String,
    pub file_name: String,
    pub original_path: String,
    pub managed_path: Option<String>,
    pub document_type: String,
    pub checksum: String,
    pub title: Option<String>,
    /// Display string: "A. Author; B. Author" (migration 2). The citation
    /// formatter parses it into individual authors (spec §31).
    pub authors: Option<String>,
    pub year: Option<i64>,
    pub doi: Option<String>,
    pub journal: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    /// "article" | "book" | "chapter" | "report" | "webpage" | "thesis" (§31).
    pub ref_type: String,
    pub indexing_status: String,
    pub status_detail: Option<String>,
    pub page_count: Option<i64>,
    pub language: Option<String>,
    pub imported_at: String,
    pub chunk_count: i64,
}

/// Sections/blocks returned by the document engine's /parse endpoint
/// (mirrors the Python contracts exactly).
#[derive(Debug, Deserialize)]
pub struct EngineParseResponse {
    pub document_id: String,
    pub ok: bool,
    pub page_count: Option<i64>,
    pub language: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub sections: Vec<EngineSection>,
    #[serde(default)]
    pub blocks: Vec<EngineBlock>,
    pub error: Option<String>,
    /// Phase 5 bibliographic hints from the engine (hints only — the user's
    /// corrections stay authoritative, spec §31).
    #[serde(default)]
    pub doi: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EngineSection {
    pub heading: String,
    pub level: i64,
    pub page_start: Option<i64>,
    pub page_end: Option<i64>,
    pub order_index: i64,
}

/// User-editable bibliographic metadata (spec §31). `None` clears a field,
/// except `ref_type` where `None` keeps the current value (the column is
/// NOT NULL and every document must have a reference type).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BibliographyUpdate {
    pub title: Option<String>,
    pub authors: Option<String>,
    pub year: Option<i64>,
    pub doi: Option<String>,
    pub journal: Option<String>,
    pub volume: Option<String>,
    pub issue: Option<String>,
    pub pages: Option<String>,
    pub publisher: Option<String>,
    pub url: Option<String>,
    pub ref_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EngineBlock {
    pub section_index: usize,
    pub kind: String,
    pub text: String,
    pub page: Option<i64>,
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
}
