//! Academic exports (Phase 6, spec §32) behind the ExportProvider interface
//! (spec §47.7).
//!
//! A neutral document model ([`ExportDoc`]) is rendered by format-specific
//! exporters:
//!
//! - **Markdown, BibTeX, RIS** — pure Rust, deterministic, byte-testable.
//! - **DOCX / PDF** — rendered by the Python sidecar (`/export/docx`,
//!   `/export/pdf`), keeping heavy formatting dependencies crash-isolated.
//!
//! Outputs land in the managed `exports/` workspace directory (spec §13) and
//! are never deleted implicitly; the UI reveals them via the OS file manager.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use ureq::Agent;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::services::citations::{self, CitationStyle, RefType};
use crate::services::documents::DocumentRow;

/// Export formats offered by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Markdown,
    Docx,
    Pdf,
    BibTeX,
    Ris,
}

impl ExportFormat {
    pub fn from_str(s: &str) -> AppResult<Self> {
        Ok(match s {
            "markdown" => ExportFormat::Markdown,
            "docx" => ExportFormat::Docx,
            "pdf" => ExportFormat::Pdf,
            "bibtex" => ExportFormat::BibTeX,
            "ris" => ExportFormat::Ris,
            other => {
                return Err(AppError::msg(format!(
                    "Unknown export format \"{other}\"."
                )))
            }
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "markdown",
            ExportFormat::Docx => "docx",
            ExportFormat::Pdf => "pdf",
            ExportFormat::BibTeX => "bibtex",
            ExportFormat::Ris => "ris",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Docx => "docx",
            ExportFormat::Pdf => "pdf",
            ExportFormat::BibTeX => "bib",
            ExportFormat::Ris => "ris",
        }
    }

    /// Which formats the sidecar renders (everything else is native Rust).
    pub fn needs_engine(self) -> bool {
        matches!(self, ExportFormat::Docx | ExportFormat::Pdf)
    }
}

/// What is being exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    Analysis,
    EvidenceTable,
    Bibliography,
}

impl ExportKind {
    pub fn from_str(s: &str) -> AppResult<Self> {
        Ok(match s {
            "analysis" => ExportKind::Analysis,
            "evidence_table" => ExportKind::EvidenceTable,
            "bibliography" => ExportKind::Bibliography,
            other => {
                return Err(AppError::msg(format!(
                    "Unknown export kind \"{other}\"."
                )))
            }
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ExportKind::Analysis => "analysis",
            ExportKind::EvidenceTable => "evidence_table",
            ExportKind::Bibliography => "bibliography",
        }
    }
}

/// A kind/format pair the UI can offer (validity checked at export time).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCapability {
    pub kind: String,
    pub formats: Vec<String>,
}

/// Static capability map — the UI renders pickers from it.
pub fn capabilities() -> Vec<ExportCapability> {
    vec![
        ExportCapability {
            kind: ExportKind::Analysis.as_str().into(),
            formats: ["markdown", "docx", "pdf"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
        ExportCapability {
            kind: ExportKind::EvidenceTable.as_str().into(),
            formats: ["markdown", "docx", "pdf", "bibtex", "ris"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
        ExportCapability {
            kind: ExportKind::Bibliography.as_str().into(),
            formats: ["markdown", "docx", "pdf", "bibtex", "ris"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
    ]
}

/// Evidence strength mirrored for export rendering (avoids importing the
/// evidence module's serde shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTableExport {
    pub question: String,
    pub rows: Vec<EvidenceRowExport>,
    #[serde(default)]
    pub findings: Vec<serde_json::Value>,
    #[serde(default)]
    pub synthesis: Option<String>,
    #[serde(default)]
    pub synthesis_citations: Vec<usize>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRowExport {
    pub document_id: String,
    pub document_name: String,
    pub strength: String,
    pub score: Option<f32>,
    pub excerpts: Vec<EvidenceExcerptExport>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceExcerptExport {
    pub chunk_id: String,
    pub page_number: Option<i64>,
    pub section_heading: Option<String>,
    pub text: String,
    #[serde(default)]
    pub matched_by: Vec<String>,
    pub vec_distance: Option<f32>,
    pub fts_rank: Option<f64>,
}

// ---------------------------------------------------------------------------
// Neutral document model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ExportBlock {
    Paragraph(String),
    Bullet(String),
    Heading(String),
    /// Rendered as a real table in DOCX/PDF; pipe table in Markdown.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

#[derive(Debug, Clone)]
pub struct ExportDoc {
    pub title: String,
    pub subtitle: Option<String>,
    pub blocks: Vec<ExportBlock>,
}

impl ExportDoc {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            blocks: Vec::new(),
        }
    }

    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }

    pub fn para(mut self, s: impl Into<String>) -> Self {
        self.blocks.push(ExportBlock::Paragraph(s.into()));
        self
    }

    pub fn bullet(mut self, s: impl Into<String>) -> Self {
        self.blocks.push(ExportBlock::Bullet(s.into()));
        self
    }

    pub fn heading(mut self, s: impl Into<String>) -> Self {
        self.blocks.push(ExportBlock::Heading(s.into()));
        self
    }

    pub fn table(mut self, columns: &[&str], rows: Vec<Vec<String>>) -> Self {
        self.blocks.push(ExportBlock::Table {
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows,
        });
        self
    }
}

// ---------------------------------------------------------------------------
// ExportProvider — the swappable rendering seam (spec §47.7)
// ---------------------------------------------------------------------------

pub trait ExportProvider: Send + Sync {
    fn name(&self) -> &'static str;
    /// Render `doc` and write it to `dest`, returning the bytes written.
    fn render(&self, doc: &ExportDoc, dest: &Path) -> AppResult<u64>;
}

/// Markdown renderer (pure Rust).
pub struct MarkdownExporter;

impl ExportProvider for MarkdownExporter {
    fn name(&self) -> &'static str {
        "markdown"
    }

    fn render(&self, doc: &ExportDoc, dest: &Path) -> AppResult<u64> {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", doc.title));
        if let Some(sub) = &doc.subtitle {
            out.push_str(&format!("_{sub}_\n\n"));
        }
        for block in &doc.blocks {
            match block {
                ExportBlock::Paragraph(text) => {
                    out.push_str(text);
                    out.push_str("\n\n");
                }
                ExportBlock::Bullet(text) => {
                    out.push_str(&format!("- {text}\n"));
                }
                ExportBlock::Heading(text) => {
                    out.push_str(&format!("## {text}\n\n"));
                }
                ExportBlock::Table { columns, rows } => {
                    out.push_str(&format!("| {} |\n", columns.join(" | ")));
                    out.push_str(&format!(
                        "|{}|\n",
                        columns.iter().map(|_| " --- ").collect::<String>()
                    ));
                    for row in rows {
                        out.push_str(&format!("| {} |\n", row.join(" | ")));
                    }
                    out.push('\n');
                }
            }
        }
        // Consecutive bullets end with a blank line for readability.
        let bytes = out.as_bytes();
        write_and_flush(dest, bytes)
    }
}

fn write_and_flush(dest: &Path, bytes: &[u8]) -> AppResult<u64> {
    std::fs::write(dest, bytes)
        .map_err(|e| AppError::msg(format!("Could not write export file: {e}")))?;
    Ok(bytes.len() as u64)
}

// ---------------------------------------------------------------------------
// Bibliography rendering (BibTeX / RIS)
// ---------------------------------------------------------------------------

/// Sanitise a string for use inside a BibTeX braced value.
fn bibtex_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('{', "\\{").replace('}', "\\}")
}

/// Stable citation key: FirstAuthorYEARword, e.g. `doe2021delta`.
fn citation_key(doc: &DocumentRow) -> String {
    // Surname of the first author: "Doe, Jane" → "Doe"; "Jane Doe" → "Doe".
    let surname = doc
        .authors
        .as_deref()
        .and_then(|a| {
            a.split([';', '&'])
                .map(str::trim)
                .find(|s| !s.is_empty())
                .map(|first| {
                    if let Some((family, _)) = first.split_once(',') {
                        family.trim().to_string()
                    } else {
                        first
                            .split_whitespace()
                            .next_back()
                            .unwrap_or(first)
                            .to_string()
                    }
                })
        })
        .unwrap_or_else(|| "unknown".into());
    let first: String = surname.to_ascii_lowercase();
    let first: String = first.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let title_word = doc
        .title
        .as_deref()
        .and_then(|t| t.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "work".to_string())
        .to_ascii_lowercase();
    let title_word: String = title_word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let year = doc.year.map(|y| y.to_string()).unwrap_or_default();
    format!("{first}{year}{title_word}")
}

fn bibtex_entry_type(doc: &DocumentRow) -> &'static str {
    match RefType::from_str(&doc.ref_type) {
        RefType::Article => "article",
        RefType::Book => "book",
        RefType::Chapter => "incollection",
        RefType::Report => "techreport",
        RefType::Thesis => "phdthesis",
        RefType::Webpage => "misc",
    }
}

/// Render one BibTeX entry (no trailing newline).
pub fn bibtex_entry(doc: &DocumentRow) -> String {
    let key = citation_key(doc);
    let mut f: Vec<(String, String)> = Vec::new();
    if let Some(a) = doc.authors.as_deref().filter(|s| !s.trim().is_empty()) {
        f.push(("author".into(), bibtex_escape(a)));
    }
    if let Some(t) = doc.title.as_deref() {
        f.push(("title".into(), bibtex_escape(t)));
    }
    match RefType::from_str(&doc.ref_type) {
        RefType::Article => {
            if let Some(j) = doc.journal.as_deref() {
                f.push(("journal".into(), bibtex_escape(j)));
            }
        }
        RefType::Book | RefType::Thesis => {
            if let Some(p) = doc.publisher.as_deref() {
                f.push(("publisher".into(), bibtex_escape(p)));
            }
        }
        RefType::Chapter => {
            if let Some(p) = doc.publisher.as_deref() {
                f.push(("booktitle".into(), bibtex_escape(p)));
            }
        }
        _ => {}
    }
    if let Some(v) = doc.volume.as_deref() {
        f.push(("volume".into(), bibtex_escape(v)));
    }
    if let Some(n) = doc.issue.as_deref() {
        f.push(("number".into(), bibtex_escape(n)));
    }
    if let Some(pg) = doc.pages.as_deref() {
        f.push(("pages".into(), bibtex_escape(pg)));
    }
    if let Some(y) = doc.year {
        f.push(("year".into(), y.to_string()));
    }
    if let Some(d) = doc.doi.as_deref() {
        f.push(("doi".into(), bibtex_escape(d)));
    }
    if let Some(u) = doc.url.as_deref() {
        f.push(("url".into(), bibtex_escape(u)));
    }

    let fields = f
        .iter()
        .map(|(k, v)| format!("  {k} = {{{v}}},\n"))
        .collect::<String>();
    format!("@{}{{{},\n{}}}", bibtex_entry_type(doc), key, fields)
}

/// Render one RIS record (no trailing blank line beyond the spec's ER).
pub fn ris_entry(doc: &DocumentRow) -> String {
    let mut out = String::new();
    let ty = match RefType::from_str(&doc.ref_type) {
        RefType::Article => "JOUR",
        RefType::Book => "BOOK",
        RefType::Chapter => "CHAP",
        RefType::Report => "RPRT",
        RefType::Thesis => "THES",
        RefType::Webpage => "EWEB",
    };
    out.push_str(&format!("TY  - {ty}\n"));
    if let Some(a) = doc.authors.as_deref() {
        for author in a.split([';', '&']).map(str::trim).filter(|s| !s.is_empty()) {
            out.push_str(&format!("AU  - {author}\n"));
        }
    }
    if let Some(t) = doc.title.as_deref() {
        out.push_str(&format!("TI  - {t}\n"));
    }
    if let Some(j) = doc.journal.as_deref() {
        out.push_str(&format!("JO  - {j}\n"));
    }
    if let Some(p) = doc.publisher.as_deref() {
        out.push_str(&format!("PB  - {p}\n"));
    }
    if let Some(v) = doc.volume.as_deref() {
        out.push_str(&format!("VL  - {v}\n"));
    }
    if let Some(n) = doc.issue.as_deref() {
        out.push_str(&format!("IS  - {n}\n"));
    }
    if let Some(pg) = doc.pages.as_deref() {
        out.push_str(&format!("SP  - {pg}\n"));
    }
    if let Some(y) = doc.year {
        out.push_str(&format!("PY  - {y}\n"));
    }
    if let Some(d) = doc.doi.as_deref() {
        out.push_str(&format!("DO  - {d}\n"));
    }
    if let Some(u) = doc.url.as_deref() {
        out.push_str(&format!("UR  - {u}\n"));
    }
    out.push_str("ER  - \n");
    out
}

fn project_documents(db: &Db, project_id: &str) -> AppResult<Vec<DocumentRow>> {
    db.list_documents(project_id)
}

// ---------------------------------------------------------------------------
// Facade
// ---------------------------------------------------------------------------

pub struct ExportService {
    exports_dir: PathBuf,
    engine_agent: OnceLock<Agent>,
}

/// Result payload for the UI.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub bytes: u64,
    pub kind: String,
    pub format: String,
}

impl ExportService {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            exports_dir: workspace_root.join("exports"),
            engine_agent: OnceLock::new(),
        }
    }

    pub fn exports_dir(&self) -> &Path {
        &self.exports_dir
    }

    fn agent(&self) -> &Agent {
        self.engine_agent.get_or_init(|| {
            let config = Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(300)))
                .build();
            Agent::new_with_config(config)
        })
    }

    /// Unique destination file: exports/<project>-<stamp>-<slug>.<ext>
    fn destination(&self, project_id: &str, slug: &str, format: ExportFormat) -> AppResult<PathBuf> {
        std::fs::create_dir_all(&self.exports_dir)
            .map_err(|e| AppError::msg(format!("Could not create exports directory: {e}")))?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let slug: String = slug
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect();
        let slug: String = slug.split_whitespace().collect();
        let file = format!("{project_id}-{}-{slug}.{}", stamp, format.extension());
        Ok(self.exports_dir.join(file))
    }

    /// Build the ExportDoc for the three export kinds.
    fn build_doc(
        &self,
        db: &Db,
        project_id: &str,
        kind: ExportKind,
        source_id: Option<&str>,
        style: CitationStyle,
    ) -> AppResult<ExportDoc> {
        match kind {
            ExportKind::Analysis => {
                let id = source_id.ok_or_else(|| AppError::msg("An analysis id is required."))?;
                let row = db.get_analysis(id)?;
                let mut doc = ExportDoc::new(row.question.clone().unwrap_or_else(|| "Analysis".into()))
                    .subtitle(format!(
                        "ResearchAI · {} · {}",
                        row.analysis_type,
                        &row.created_at[..row.created_at.len().min(19)]
                    ));
                doc = doc.heading("Answer").para(row.answer_text);
                doc = doc.heading("Cited passages");
                let parsed: Vec<serde_json::Value> = serde_json::from_str(&row.evidence_json)
                    .unwrap_or_default();
                for (i, ev) in parsed.iter().enumerate() {
                    let text = ev
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let name = ev
                        .get("documentName")
                        .and_then(|t| t.as_str())
                        .unwrap_or("document");
                    let page = ev.get("pageNumber").and_then(|p| p.as_i64());
                    let loc = page.map(|p| format!(" (p. {p})")).unwrap_or_default();
                    doc = doc.para(format!("[{}] {}{loc}: {text}", i + 1, name));
                }
                Ok(doc)
            }
            ExportKind::EvidenceTable => {
                let id = source_id.ok_or_else(|| AppError::msg("An evidence table id is required."))?;
                let row = db.get_evidence_table(id)?;
                let table: EvidenceTableExport = serde_json::from_str(&row.table_json)
                    .map_err(|e| AppError::msg(format!("Saved table is unreadable: {e}")))?;
                let mut doc = ExportDoc::new(table.question.clone())
                    .subtitle(format!("Evidence matrix · {}", &row.created_at[..row.created_at.len().min(19)]));
                let mut rows: Vec<Vec<String>> = Vec::new();
                for r in &table.rows {
                    let excerpt_count = r.excerpts.len();
                    let pages = r
                        .excerpts
                        .iter()
                        .filter_map(|e| e.page_number)
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    rows.push(vec![
                        r.document_name.clone(),
                        r.strength.clone(),
                        excerpt_count.to_string(),
                        if pages.is_empty() { "—".into() } else { pages },
                    ]);
                }
                doc = doc.table(
                    &["Document", "Strength", "Excerpts", "Pages"],
                    rows,
                );
                for (i, f) in table.findings.iter().enumerate() {
                    let name = f
                        .get("documentName")
                        .and_then(|n| n.as_str())
                        .unwrap_or("document");
                    let text = f.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    doc = doc.heading(format!("Findings — {}", name));
                    doc = doc.para(format!("[{}] {}", i + 1, text));
                }
                if let Some(s) = &table.synthesis {
                    doc = doc.heading("Cross-document synthesis").para(s.clone());
                }
                Ok(doc)
            }
            ExportKind::Bibliography => {
                let refs = citations::bibliography(db, project_id, style)?;
                let mut doc = ExportDoc::new("Bibliography").subtitle(
                    match style {
                        CitationStyle::Apa => "APA 7",
                        CitationStyle::Harvard => "Harvard",
                        CitationStyle::Chicago => "Chicago",
                    },
                );
                for r in refs {
                    doc = doc.para(r.reference);
                }
                Ok(doc)
            }
        }
    }

    /// Render a document-kind export (analysis / evidence / bibliography)
    /// in the requested format, writing into the managed exports/ directory.
    #[allow(clippy::too_many_arguments)]
    pub fn export_doc(
        &self,
        db: &Db,
        project_id: &str,
        kind: ExportKind,
        source_id: Option<&str>,
        format: ExportFormat,
        style: CitationStyle,
    ) -> AppResult<ExportResult> {
        match kind {
            ExportKind::Bibliography => {
                if matches!(format, ExportFormat::BibTeX | ExportFormat::Ris) {
                    return self.export_bibliography_database(db, project_id, format);
                }
            }
            ExportKind::Analysis | ExportKind::EvidenceTable => {
                if matches!(format, ExportFormat::BibTeX | ExportFormat::Ris) {
                    return Err(AppError::msg(
                        "BibTeX and RIS exports apply to bibliographies.",
                    ));
                }
            }
        }

        let doc = self.build_doc(db, project_id, kind, source_id, style)?;
        let short = source_id.and_then(|s| s.get(..8)).unwrap_or("unknown");
        let slug = match kind {
            ExportKind::Analysis => format!("analysis-{short}"),
            ExportKind::EvidenceTable => format!("evidence-{short}"),
            ExportKind::Bibliography => "bibliography".to_string(),
        };
        let dest = self.destination(project_id, &slug, format)?;

        let bytes = if format.needs_engine() {
            self.render_via_engine(&doc, format, &dest)?
        } else {
            let exporter: &dyn ExportProvider = &MarkdownExporter;
            exporter.render(&doc, &dest)?
        };

        Ok(ExportResult {
            path: dest.to_string_lossy().to_string(),
            bytes,
            kind: kind.as_str().into(),
            format: format.as_str().into(),
        })
    }

    /// BibTeX / RIS for the whole project bibliography.
    pub fn export_bibliography_database(
        &self,
        db: &Db,
        project_id: &str,
        format: ExportFormat,
    ) -> AppResult<ExportResult> {
        let docs = project_documents(db, project_id)?;
        let dest = self.destination(
            project_id,
            if matches!(format, ExportFormat::BibTeX) {
                "bibtex"
            } else {
                "ris"
            },
            format,
        )?;
        let mut out = String::new();
        for doc in &docs {
            match format {
                ExportFormat::BibTeX => out.push_str(&bibtex_entry(doc)),
                ExportFormat::Ris => out.push_str(&ris_entry(doc)),
                _ => unreachable!("caller validates format"),
            }
            out.push('\n');
        }
        let bytes = write_and_flush(&dest, out.as_bytes())?;
        Ok(ExportResult {
            path: dest.to_string_lossy().to_string(),
            bytes,
            kind: ExportKind::Bibliography.as_str().into(),
            format: format.as_str().into(),
        })
    }

    /// POST the ExportDoc to the engine and save the rendered binary.
    fn render_via_engine(&self, doc: &ExportDoc, format: ExportFormat, dest: &Path) -> AppResult<u64> {
        let endpoint = match format {
            ExportFormat::Docx => "/export/docx",
            ExportFormat::Pdf => "/export/pdf",
            _ => unreachable!("caller validates format"),
        };
        let body = serde_json::json!({
            "title": doc.title,
            "subtitle": doc.subtitle,
            "blocks": doc.blocks.iter().map(|b| match b {
                ExportBlock::Paragraph(t) => serde_json::json!({"kind": "paragraph", "text": t}),
                ExportBlock::Bullet(t) => serde_json::json!({"kind": "bullet", "text": t}),
                ExportBlock::Heading(t) => serde_json::json!({"kind": "heading", "text": t}),
                ExportBlock::Table { columns, rows } => serde_json::json!({
                    "kind": "table", "columns": columns, "rows": rows
                }),
            }).collect::<Vec<_>>(),
        });

        let resp = self
            .agent()
            .post(format!("http://127.0.0.1:8737{endpoint}"))
            .send_json(body)
            .map_err(|e| AppError::msg(format!("Document engine unreachable: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::msg(format!(
                "Export render failed: HTTP {}",
                resp.status()
            )));
        }
        let mut bytes = Vec::new();
        resp.into_body()
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| AppError::msg(format!("Export download failed: {e}")))?;
        write_and_flush(dest, &bytes)
    }
}

use std::io::Read as _;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::temp_dir_for;

    fn sample_doc() -> ExportDoc {
        ExportDoc::new("Export title")
            .subtitle("ResearchAI · test")
            .para("A paragraph of body text.")
            .heading("Findings")
            .bullet("First finding")
            .bullet("Second finding")
            .table(
                &["Document", "Strength"],
                vec![
                    vec!["a.pdf".into(), "direct".into()],
                    vec!["b.pdf".into(), "none".into()],
                ],
            )
    }

    fn doc_row(author: &str, year: Option<i64>, title: &str, ref_type: &str) -> DocumentRow {
        DocumentRow {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: "p1".into(),
            file_name: format!("{title}.pdf"),
            original_path: "/tmp/x.pdf".into(),
            managed_path: None,
            document_type: "pdf".into(),
            checksum: uuid::Uuid::new_v4().to_string(),
            title: Some(title.into()),
            authors: Some(author.into()),
            year,
            doi: Some("10.1234/abc".into()),
            journal: Some("Coastal Research".into()),
            volume: Some("12".into()),
            issue: Some("3".into()),
            pages: Some("101–118".into()),
            publisher: None,
            url: None,
            ref_type: ref_type.into(),
            indexing_status: "ready".into(),
            status_detail: None,
            page_count: None,
            language: None,
            imported_at: crate::db::now_iso_pub(),
            chunk_count: 0,
        }
    }

    #[test]
    fn markdown_renders_all_block_types() {
        let dir = temp_dir_for("exports-md");
        let dest = dir.path().join("out.md");
        let written = MarkdownExporter.render(&sample_doc(), &dest).unwrap();
        let text = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(written, text.len() as u64);
        assert!(text.starts_with("# Export title\n\n"));
        assert!(text.contains("_ResearchAI · test_"));
        assert!(text.contains("## Findings\n\n- First finding\n- Second finding\n"));
        assert!(text.contains("| Document | Strength |"));
        assert!(text.contains("| a.pdf | direct |"));
    }

    #[test]
    fn bibtex_entry_shape_and_escaping() {
        let d = doc_row("Jane Doe; John Smith", Some(2021), "Delta retreat", "article");
        let entry = bibtex_entry(&d);
        assert!(entry.starts_with("@article{doe2021delta,"), "{entry}");
        assert!(entry.contains("author = {Jane Doe; John Smith}"));
        assert!(entry.contains("journal = {Coastal Research}"));
        assert!(entry.contains("volume = {12}"));
        assert!(entry.contains("number = {3}"));
        assert!(entry.contains("pages = {101–118}"));
        assert!(entry.contains("year = {2021}"));
        assert!(entry.contains("doi = {10.1234/abc}"));
        assert!(entry.ends_with(",\n}"));

        // Braces are escaped, not structural.
        let weird = doc_row("A. {Odd}", Some(2020), "Title with } brace", "misc");
        let entry = bibtex_entry(&weird);
        assert!(entry.contains("author = {A. \\{Odd\\}}"), "{entry}");
    }

    #[test]
    fn bibtex_entry_types_follow_ref_type() {
        assert!(bibtex_entry(&doc_row("A", Some(2020), "T", "book")).starts_with("@book{"));
        assert!(bibtex_entry(&doc_row("A", Some(2020), "T", "chapter")).starts_with("@incollection{"));
        assert!(bibtex_entry(&doc_row("A", Some(2020), "T", "report")).starts_with("@techreport{"));
        assert!(bibtex_entry(&doc_row("A", Some(2020), "T", "thesis")).starts_with("@phdthesis{"));
        assert!(bibtex_entry(&doc_row("A", Some(2020), "T", "webpage")).starts_with("@misc{"));
    }

    #[test]
    fn ris_record_shape() {
        let d = doc_row("Jane Doe; John Smith", Some(2021), "Delta retreat", "article");
        let rec = ris_entry(&d);
        let lines: Vec<&str> = rec.lines().collect();
        assert_eq!(lines[0], "TY  - JOUR");
        assert!(lines.contains(&"AU  - Jane Doe"));
        assert!(lines.contains(&"AU  - John Smith"));
        assert!(lines.contains(&"TI  - Delta retreat"));
        assert!(lines.contains(&"JO  - Coastal Research"));
        assert!(lines.contains(&"VL  - 12"));
        assert!(lines.contains(&"IS  - 3"));
        assert!(lines.contains(&"PY  - 2021"));
        assert!(lines.contains(&"DO  - 10.1234/abc"));
        assert_eq!(*lines.last().unwrap(), "ER  - ");
    }

    #[test]
    fn citation_key_is_stable_and_sanitised() {
        let d = doc_row("Jane Doe", Some(2021), "Delta retreat under rise", "article");
        assert_eq!(citation_key(&d), "doe2021delta");
        let no_meta = doc_row("", None, "", "article");
        assert!(citation_key(&no_meta).starts_with("unknown"));
    }

    #[test]
    fn bibliography_bibtex_export_writes_file() {
        let dir = temp_dir_for("exports-bib");
        let db = crate::db::Db::open(dir.path()).unwrap();
        let project = db.create_project("Exports", None).unwrap();
        let mut d = doc_row("Jane Doe", Some(2021), "Delta retreat", "article");
        d.project_id = project.id.clone();
        db.insert_document(d).unwrap();

        let svc = ExportService::new(dir.path());
        let result = svc
            .export_bibliography_database(&db, &project.id, ExportFormat::BibTeX)
            .unwrap();
        assert_eq!(result.format, "bibtex");
        assert!(result.path.contains("exports"));
        let text = std::fs::read_to_string(&result.path).unwrap();
        assert!(text.contains("@article{doe2021delta,"));

        let result = svc
            .export_bibliography_database(&db, &project.id, ExportFormat::Ris)
            .unwrap();
        let text = std::fs::read_to_string(&result.path).unwrap();
        assert!(text.contains("TY  - JOUR"));
        assert!(text.contains("ER  - "));
    }

    #[test]
    fn analysis_export_renders_markdown_with_evidence() {
        let dir = temp_dir_for("exports-analysis");
        let db = crate::db::Db::open(dir.path()).unwrap();
        let project = db.create_project("Exports", None).unwrap();
        let analysis = crate::db::AnalysisRow {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project.id.clone(),
            document_id: None,
            analysis_type: "research".into(),
            model_id: None,
            prompt_version: "v1".into(),
            question: Some("Why do deltas retreat?".into()),
            answer_text: "Sediment starvation is the driver.".into(),
            evidence_json: serde_json::json!([
                {"chunkId": "c1", "documentId": "d1", "documentName": "a.pdf",
                 "pageNumber": 4, "sectionHeading": null, "text": "Deltas shrink.",
                 "startOffset": null, "endOffset": null}
            ])
            .to_string(),
            trace_json: None,
            created_at: crate::db::now_iso_pub(),
        };
        db.insert_analysis(&analysis).unwrap();

        let svc = ExportService::new(dir.path());
        let result = svc
            .export_doc(
                &db,
                &project.id,
                ExportKind::Analysis,
                Some(&analysis.id),
                ExportFormat::Markdown,
                CitationStyle::Apa,
            )
            .unwrap();
        let text = std::fs::read_to_string(&result.path).unwrap();
        assert!(text.contains("# Why do deltas retreat?"));
        assert!(text.contains("## Answer\n\nSediment starvation is the driver."));
        assert!(text.contains("[1] a.pdf (p. 4): Deltas shrink."));
    }

    #[test]
    fn evidence_table_export_renders_matrix_and_synthesis() {
        let dir = temp_dir_for("exports-evidence");
        let db = crate::db::Db::open(dir.path()).unwrap();
        let project = db.create_project("Exports", None).unwrap();
        let table = EvidenceTableExport {
            question: "Which deltas retreat fastest?".into(),
            rows: vec![EvidenceRowExport {
                document_id: "d1".into(),
                document_name: "a.pdf".into(),
                strength: "direct".into(),
                score: Some(0.5),
                excerpts: vec![EvidenceExcerptExport {
                    chunk_id: "c1".into(),
                    page_number: Some(7),
                    section_heading: None,
                    text: "Excerpt".into(),
                    matched_by: vec!["keyword".into()],
                    vec_distance: None,
                    fts_rank: None,
                }],
            }],
            findings: vec![serde_json::json!({
                "documentId": "d1", "documentName": "a.pdf",
                "text": "Says X [1]", "citationsUsed": [1]
            })],
            synthesis: Some("Both agree.".into()),
            synthesis_citations: vec![1],
        };
        let row = crate::db::EvidenceTableRow {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project.id.clone(),
            question: table.question.clone(),
            scope_json: "[]".into(),
            table_json: serde_json::to_string(&table).unwrap(),
            trace_json: None,
            model_id: None,
            prompt_version: "ev-v1".into(),
            created_at: crate::db::now_iso_pub(),
        };
        db.insert_evidence_table(&row).unwrap();

        let svc = ExportService::new(dir.path());
        let result = svc
            .export_doc(
                &db,
                &project.id,
                ExportKind::EvidenceTable,
                Some(&row.id),
                ExportFormat::Markdown,
                CitationStyle::Apa,
            )
            .unwrap();
        let text = std::fs::read_to_string(&result.path).unwrap();
        assert!(text.contains("# Which deltas retreat fastest?"));
        assert!(text.contains("| a.pdf | direct | 1 | 7 |"));
        assert!(text.contains("## Findings — a.pdf"));
        assert!(text.contains("## Cross-document synthesis\n\nBoth agree."));
    }

    #[test]
    fn invalid_combinations_are_rejected() {
        let dir = temp_dir_for("exports-invalid");
        let db = crate::db::Db::open(dir.path()).unwrap();
        let project = db.create_project("P", None).unwrap();
        let svc = ExportService::new(dir.path());

        let err = svc
            .export_doc(
                &db,
                &project.id,
                ExportKind::Analysis,
                Some("whatever"),
                ExportFormat::Ris,
                CitationStyle::Apa,
            )
            .unwrap_err();
        assert!(err.to_string().contains("bibliographies"));

        let err = svc
            .export_doc(
                &db,
                &project.id,
                ExportKind::Analysis,
                None,
                ExportFormat::Markdown,
                CitationStyle::Apa,
            )
            .unwrap_err();
        assert!(err.to_string().contains("analysis id is required"));
    }

    #[test]
    fn capabilities_cover_the_matrix() {
        let caps = capabilities();
        assert_eq!(caps.len(), 3);
        let bib = caps.iter().find(|c| c.kind == "bibliography").unwrap();
        assert!(bib.formats.contains(&"bibtex".to_string()));
        assert!(bib.formats.contains(&"ris".to_string()));
    }

    #[test]
    fn format_parsing_and_extensions() {
        assert!(ExportFormat::from_str("docx").is_ok());
        assert!(ExportFormat::from_str("epub").is_err());
        assert_eq!(ExportFormat::BibTeX.extension(), "bib");
        assert_eq!(ExportFormat::Ris.extension(), "ris");
        assert!(ExportFormat::Pdf.needs_engine());
        assert!(!ExportFormat::Markdown.needs_engine());
    }
}
