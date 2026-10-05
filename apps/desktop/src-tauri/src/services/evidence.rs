//! Evidence matrix / cross-document comparison (Phase 4, spec §18).
//!
//! Two layers:
//!
//! 1. **Deterministic core** (No-AI, spec §35): for each document in scope,
//!    hybrid retrieval finds the best matching excerpts; rows carry page
//!    references and an evidence-strength label derived from *how* the
//!    document matched (which channels, how many candidates, what distance)
//!    — never an invented "truth score".
//! 2. **Optional AI pass** (only when AI is enabled and the model is loaded):
//!    per-document findings with citations, then one synthesis pass over all
//!    documents (agreements / disagreements / gaps) under the same
//!    citation-only discipline as [`crate::services::analysis`].
//!
//! Tables are persisted to `evidence_tables` (migration 5) and re-openable.

use serde::Serialize;

use crate::db::Db;
use crate::error::AppError;
use crate::services::ai::{AiProvider, CompletionRequest};
use crate::services::retrieval;

/// Bump when AI prompt wording for evidence tables changes.
pub const EVIDENCE_PROMPT_VERSION: &str = "ev-v1";

const MAX_EXCERPTS_PER_DOC: usize = 3;
const MAX_DOCS: usize = 20;
const MAX_SYNTHESIS_CHARS: usize = 6000;

// ---------------------------------------------------------------------------
// DTOs (IPC: camelCase, locked by tests/contract_locks.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceExcerpt {
    pub chunk_id: String,
    pub page_number: Option<i64>,
    pub section_heading: Option<String>,
    pub text: String,
    pub matched_by: Vec<String>,
    pub vec_distance: Option<f32>,
    pub fts_rank: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    /// Matched by BOTH keyword and vector channels — the document addresses
    /// the question directly.
    Direct,
    /// Several candidate passages matched — the topic is covered repeatedly.
    Multiple,
    /// Only one channel matched, or the match is weak.
    Indirect,
    /// No matching passage found in this document.
    None,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRow {
    pub document_id: String,
    pub document_name: String,
    pub strength: Strength,
    /// Best fusion score among this document's excerpts (None = no match).
    pub score: Option<f32>,
    pub excerpts: Vec<EvidenceExcerpt>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocFindings {
    pub document_id: String,
    pub document_name: String,
    /// Model output for this document (citation markers refer to the
    /// document's own numbered excerpts).
    pub text: String,
    pub citations_used: Vec<usize>,
}

/// Persisted as JSON in `evidence_tables.table_json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTable {
    pub question: String,
    pub rows: Vec<EvidenceRow>,
    /// Per-document AI findings; empty when no AI pass ran.
    pub findings: Vec<DocFindings>,
    /// Cross-document AI synthesis; None when no AI pass ran.
    pub synthesis: Option<String>,
    pub synthesis_citations: Vec<usize>,
}

/// Persisted as JSON in `evidence_tables.trace_json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceTrace {
    pub mode: String,
    pub engine: String,
    pub model: Option<String>,
    pub scope_documents: usize,
    pub documents_matched: usize,
    pub total_excerpts: usize,
    pub strength_counts: std::collections::BTreeMap<String, usize>,
    pub retrieval_warnings: Vec<String>,
    pub warnings: Vec<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceResponse {
    pub table_id: Option<String>,
    /// Model used for the AI pass (None = deterministic).
    pub model_id: Option<String>,
    pub table: EvidenceTable,
    pub trace: EvidenceTrace,
}

#[derive(Debug, Clone)]
pub struct EvidenceRequest {
    pub project_id: String,
    /// Empty = whole project.
    pub document_ids: Vec<String>,
    pub question: String,
    /// When true AND a provider is supplied, run the AI passes.
    pub with_ai: bool,
    /// Registry id of the model behind `provider` (persisted with the table).
    pub model_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Deterministic core
// ---------------------------------------------------------------------------

/// Build the evidence matrix for a question across the scoped documents.
/// `provider = Some(_)` + `req.with_ai` runs the optional AI passes.
pub fn build_table(
    db: &Db,
    provider: Option<&dyn AiProvider>,
    req: &EvidenceRequest,
) -> crate::error::AppResult<EvidenceResponse> {
    let started = std::time::Instant::now();
    let question = req.question.trim();
    if question.is_empty() {
        return Err(AppError::msg("The question must not be empty."));
    }

    // One retrieval per document keeps rows honest per document (per-doc caps
    // inside `search` would otherwise skew cross-document comparison).
    let mut rows: Vec<EvidenceRow> = Vec::new();
    let mut retrieval_warnings: Vec<String> = Vec::new();
    let mut doc_ids: Vec<String> = req.document_ids.clone();
    if doc_ids.is_empty() {
        doc_ids = db
            .list_documents(&req.project_id)?
            .into_iter()
            .filter(|d| d.indexing_status == "ready")
            .map(|d| d.id)
            .collect();
    }
    if doc_ids.len() > MAX_DOCS {
        doc_ids.truncate(MAX_DOCS);
        retrieval_warnings
            .push(format!("Scope truncated to the first {MAX_DOCS} ready documents."));
    }

    for doc_id in &doc_ids {
        let Ok(doc) = db.get_document(doc_id) else { continue };
        let response =
            retrieval::search(db, question, std::slice::from_ref(doc_id), MAX_EXCERPTS_PER_DOC)?;
        for w in &response.trace.warnings {
            let tagged = format!("{}: {w}", doc.file_name);
            if !retrieval_warnings.contains(&tagged) {
                retrieval_warnings.push(tagged);
            }
        }

        let excerpts: Vec<EvidenceExcerpt> = response
            .hits
            .iter()
            .map(|h| EvidenceExcerpt {
                chunk_id: h.chunk_id.clone(),
                page_number: h.page_number,
                section_heading: h.section_heading.clone(),
                text: h.text.clone(),
                matched_by: h.matched_by.clone(),
                vec_distance: h.vec_distance,
                fts_rank: h.fts_rank,
            })
            .collect();

        let strength = classify_strength(&excerpts);
        let best_score = response
            .hits
            .iter()
            .map(|h| h.score)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        rows.push(EvidenceRow {
            document_id: doc.id.clone(),
            document_name: doc.file_name.clone(),
            strength,
            score: best_score,
            excerpts,
        });
    }

    rows.sort_by(|a, b| {
        b.score
            .unwrap_or(f32::NEG_INFINITY)
            .partial_cmp(&a.score.unwrap_or(f32::NEG_INFINITY))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let documents_matched = rows.iter().filter(|r| r.strength != Strength::None).count();
    let total_excerpts: usize = rows.iter().map(|r| r.excerpts.len()).sum();
    let mut strength_counts = std::collections::BTreeMap::new();
    for r in &rows {
        let key = serde_json::to_value(&r.strength)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        *strength_counts.entry(key).or_insert(0) += 1;
    }

    // -- optional AI passes ---------------------------------------------------
    let mut findings: Vec<DocFindings> = Vec::new();
    let mut synthesis = None;
    let mut synthesis_citations: Vec<usize> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut engine = "deterministic".to_string();

    let want_ai = req.with_ai && provider.is_some();
    if req.with_ai && provider.is_none() {
        warnings.push("AI pass requested but no provider is available — deterministic table only.".into());
    }

    if want_ai && documents_matched > 0 {
        let provider = provider.expect("checked above");
        engine = provider.name().to_string();

        for row in rows.iter().filter(|r| r.strength != Strength::None) {
            match doc_findings(provider, row, question) {
                Ok(f) => findings.push(f),
                Err(e) => warnings.push(format!(
                    "AI findings for {} failed: {e}",
                    row.document_name
                )),
            }
        }

        match synthesize(provider, &findings, question) {
            Ok((text, used)) => {
                synthesis = Some(text);
                synthesis_citations = used;
            }
            Err(e) => warnings.push(format!("AI synthesis failed: {e}")),
        }
    }

    let trace = EvidenceTrace {
        mode: if findings.is_empty() && synthesis.is_none() {
            "deterministic".into()
        } else {
            "ai".into()
        },
        engine,
        // Model labels arrive per-completion in the findings text; the trace
        // carries the engine-level identifier.
        model: None,
        scope_documents: doc_ids.len(),
        documents_matched,
        total_excerpts,
        strength_counts,
        retrieval_warnings,
        warnings,
        duration_ms: started.elapsed().as_millis() as u64,
    };

    Ok(EvidenceResponse {
        table_id: None,
        model_id: if findings.is_empty() && synthesis.is_none() {
            None
        } else {
            req.model_id.clone()
        },
        table: EvidenceTable {
            question: question.to_string(),
            rows,
            findings,
            synthesis,
            synthesis_citations,
        },
        trace,
    })
}

/// Evidence-strength from the retrieval path — describes HOW the document
/// matched, never a truth score (spec §18: labels with reasons, no magic).
fn classify_strength(excerpts: &[EvidenceExcerpt]) -> Strength {
    if excerpts.is_empty() {
        return Strength::None;
    }
    let both = excerpts
        .iter()
        .filter(|e| e.matched_by.len() >= 2)
        .count();
    if both > 0 {
        Strength::Direct
    } else if excerpts.len() >= 2 {
        Strength::Multiple
    } else {
        Strength::Indirect
    }
}

// ---------------------------------------------------------------------------
// Optional AI passes
// ---------------------------------------------------------------------------

/// Numbered excerpts for one document → the model's findings for it.
fn doc_findings(
    provider: &dyn AiProvider,
    row: &EvidenceRow,
    question: &str,
) -> crate::error::AppResult<DocFindings> {
    let mut prompt = format!(
        "Question: {question}\n\nBelow are the matching passages from ONE document \
         (\"{}\"). Summarise what THIS document says that answers the question. \
         Cite the passage numbers like [1]. If the passages do not answer the \
         question, say so plainly.\n\nPassages:\n",
        row.document_name
    );
    for (i, e) in row.excerpts.iter().enumerate() {
        let page = e
            .page_number
            .map(|p| format!(", p. {p}"))
            .unwrap_or_default();
        prompt.push_str(&format!("[{}] ({}{}) {}\n\n", i + 1, row.document_name, page, e.text));
    }

    let completion = provider.complete(&CompletionRequest {
        system_prompt: DOC_FINDINGS_SYSTEM.into(),
        user_prompt: prompt,
        max_tokens: 512,
        temperature: 0.2,
        disable_thinking: false,
    })?;

    let citations_used =
        crate::services::analysis::used_citations(&completion.text, row.excerpts.len());
    Ok(DocFindings {
        document_id: row.document_id.clone(),
        document_name: row.document_name.clone(),
        text: completion.text,
        citations_used,
    })
}

const DOC_FINDINGS_SYSTEM: &str = "You are ResearchAI, comparing documents for an evidence table. You summarise ONE document's contribution, using ONLY the numbered passages provided. Every claim ends with a citation marker like [1]. Never invent content; if the passages do not answer the question, say so.";

/// All documents' findings → cross-document synthesis.
fn synthesize(
    provider: &dyn AiProvider,
    findings: &[DocFindings],
    question: &str,
) -> crate::error::AppResult<(String, Vec<usize>)> {
    let mut prompt = format!(
        "Question: {question}\n\nBelow are per-document summaries of what each \
         document says about the question, each citing that document's own \
         passage numbers. Write a cross-document synthesis:\n\
         AGREEMENTS: what the documents agree on.\n\
         DISAGREEMENTS: where they conflict, naming the documents.\n\
         GAPS: what none of the documents answer.\n\
         Cite per-document citations in brackets, e.g. [1] or [2][3], referring \
         to the numbered items in the input.\n\n"
    );
    let mut item_no = 0usize;
    for f in findings {
        item_no += 1;
        if prompt.len() > MAX_SYNTHESIS_CHARS {
            break;
        }
        prompt.push_str(&format!("--- Document {}: {} ---\n{}\n\n", item_no, f.document_name, f.text));
    }
    let completion = provider.complete(&CompletionRequest {
        system_prompt: SYNTHESIS_SYSTEM.into(),
        user_prompt: prompt,
        max_tokens: 768,
        temperature: 0.2,
        disable_thinking: false,
    })?;

    // Citations here refer to document items (1..=findings.len()).
    let used = crate::services::analysis::used_citations(&completion.text, findings.len());
    Ok((completion.text, used))
}

const SYNTHESIS_SYSTEM: &str = "You are ResearchAI, writing the synthesis row of an evidence table. You compare documents using ONLY the per-document summaries provided. Mark disagreements explicitly and attribute every claim to document numbers like [1]. Never invent sources; if the evidence conflicts, say so plainly.";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::temp_dir_for;
    use crate::services::ai::EchoProvider;
    use crate::services::documents::{DocumentRow, EngineBlock, EngineParseResponse};

    fn db_with_project(label: &str) -> (crate::db::tests::TempDir, Db, String) {
        let dir = temp_dir_for(label);
        let db = Db::open(dir.path()).unwrap();
        let project = db.create_project("Evidence tests", None).unwrap();
        (dir, db, project.id)
    }

    fn add_document(db: &Db, project_id: &str, name: &str, texts: &[&str]) -> String {
        let doc_id = uuid::Uuid::new_v4().to_string();
        db.insert_document(DocumentRow {
            id: doc_id.clone(),
            project_id: project_id.into(),
            file_name: name.into(),
            original_path: format!("/tmp/{name}"),
            managed_path: None,
            document_type: "txt".into(),
            checksum: uuid::Uuid::new_v4().to_string(),
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
            indexing_status: "ready".into(),
            status_detail: None,
            page_count: Some(2),
            language: None,
            imported_at: crate::db::now_iso_pub(),
            chunk_count: 0,
        })
        .unwrap();
        let parsed = EngineParseResponse {
            document_id: doc_id.clone(),
            ok: true,
            page_count: Some(2),
            language: None,
            title: None,
            sections: Vec::new(),
            blocks: texts
                .iter()
                .enumerate()
                .map(|(i, t)| EngineBlock {
                    section_index: 0,
                    kind: "text".into(),
                    text: (*t).into(),
                    page: Some((i as i64 % 2) + 1),
                    start_offset: Some(i as i64 * 50),
                    end_offset: Some(i as i64 * 50 + t.len() as i64),
                })
                .collect(),
            error: None,
            doi: None,
            year: None,
        };
        db.replace_sections_and_chunks(&doc_id, &parsed).unwrap();
        doc_id
    }

    fn request(project_id: &str, question: &str, docs: Vec<String>, with_ai: bool) -> EvidenceRequest {
        EvidenceRequest {
            project_id: project_id.into(),
            document_ids: docs,
            question: question.into(),
            with_ai,
            model_id: None,
        }
    }

    #[test]
    fn deterministic_table_rows_per_document_with_strengths() {
        let (_dir, db, project) = db_with_project("ev-deterministic");
        let delta = add_document(
            &db,
            &project,
            "deltas-2020.txt",
            &["River deltas retreat when sediment supply falls below sea-level rise."],
        );
        let sleep = add_document(
            &db,
            &project,
            "sleep-memory.txt",
            &["Sleep-dependent memory consolidation requires slow-wave activity."],
        );

        let resp = build_table(&db, None, &request(&project, "delta retreat", vec![], false)).unwrap();

        assert_eq!(resp.trace.mode, "deterministic");
        assert_eq!(resp.trace.scope_documents, 2);
        assert_eq!(resp.table.rows.len(), 2);

        // The delta doc matched; the sleep doc did not.
        let delta_row = resp.table.rows.iter().find(|r| r.document_id == delta).unwrap();
        let sleep_row = resp.table.rows.iter().find(|r| r.document_id == sleep).unwrap();
        assert_eq!(delta_row.strength, Strength::Indirect, "keyword-only hit → indirect");
        assert_eq!(sleep_row.strength, Strength::None);
        assert!(delta_row.score.is_some());
        assert!(sleep_row.score.is_none());
        assert!(sleep_row.excerpts.is_empty());
        assert_eq!(resp.trace.documents_matched, 1);
        assert_eq!(resp.trace.total_excerpts, 1);
        assert_eq!(resp.trace.strength_counts.get("none"), Some(&1));

        // Sorted: matched documents first.
        assert_eq!(resp.table.rows[0].document_id, delta);

        // Deterministic: no AI fields.
        assert!(resp.table.findings.is_empty());
        assert!(resp.table.synthesis.is_none());
        assert!(resp.table_id.is_none());
    }

    #[test]
    fn empty_question_rejected() {
        let (_dir, db, project) = db_with_project("ev-blank");
        let err = build_table(&db, None, &request(&project, "   ", vec![], false)).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn ai_pass_produces_findings_and_synthesis_with_citations() {
        let (_dir, db, project) = db_with_project("ev-ai");
        add_document(
            &db,
            &project,
            "a-deltas.txt",
            &["Deltas shrink as sediment supply drops."],
        );
        add_document(
            &db,
            &project,
            "b-deltas.txt",
            &["Delta subsidence accelerates with groundwater extraction."],
        );

        let resp = build_table(
            &db,
            Some(&EchoProvider),
            &request(&project, "deltas", vec![], true),
        )
        .unwrap();

        assert_eq!(resp.trace.mode, "ai");
        assert_eq!(resp.trace.engine, "echo");
        assert_eq!(resp.table.findings.len(), 2, "findings for each matched doc");
        // Row order depends on equal RRF scores; assert set membership.
        let names: Vec<&str> = resp.table.findings.iter().map(|f| f.document_name.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("a-")), "a- doc missing: {names:?}");
        assert!(names.iter().any(|n| n.starts_with("b-")), "b- doc missing: {names:?}");
        assert!(!resp.table.findings[0].citations_used.is_empty(), "echo echoes the prompt");
        let synthesis = resp.table.synthesis.as_deref().unwrap();
        assert!(!synthesis.is_empty());
        // Synthesis citations refer to document items (bounded by count).
        assert!(resp.table.synthesis_citations.iter().all(|n| *n >= 1 && *n <= 2));
    }

    #[test]
    fn ai_requested_without_provider_warns_and_falls_back() {
        let (_dir, db, project) = db_with_project("ev-noai");
        add_document(&db, &project, "a.txt", &["Delta content here."]);

        let resp = build_table(&db, None, &request(&project, "delta", vec![], true)).unwrap();

        assert_eq!(resp.trace.mode, "deterministic");
        assert!(resp
            .trace
            .warnings
            .iter()
            .any(|w| w.contains("no provider")));
        assert!(resp.table.findings.is_empty());
    }

    #[test]
    fn strength_classification_follows_channels() {
        let mk = |matched: &[&str]| EvidenceExcerpt {
            chunk_id: "c".into(),
            page_number: None,
            section_heading: None,
            text: "t".into(),
            matched_by: matched.iter().map(|s| s.to_string()).collect(),
            vec_distance: None,
            fts_rank: None,
        };
        assert_eq!(classify_strength(&[]), Strength::None);
        assert_eq!(classify_strength(&[mk(&["keyword"])]), Strength::Indirect);
        assert_eq!(classify_strength(&[mk(&["keyword"]), mk(&["vector"])]), Strength::Multiple);
        assert_eq!(classify_strength(&[mk(&["keyword", "vector"])]), Strength::Direct);
        assert_eq!(
            classify_strength(&[mk(&["keyword"]), mk(&["keyword", "vector"])]),
            Strength::Direct
        );
    }

    #[test]
    fn table_roundtrips_through_persistence() {
        let (_dir, db, project) = db_with_project("ev-persist");
        add_document(&db, &project, "a.txt", &["River deltas and sediment."]);
        let resp = build_table(&db, None, &request(&project, "delta", vec![], false)).unwrap();

        let row = crate::db::EvidenceTableRow {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project.clone(),
            question: resp.table.question.clone(),
            scope_json: "[]".into(),
            table_json: serde_json::to_string(&resp.table).unwrap(),
            trace_json: Some(serde_json::to_string(&resp.trace).unwrap()),
            model_id: None,
            prompt_version: EVIDENCE_PROMPT_VERSION.into(),
            created_at: crate::db::now_iso_pub(),
        };
        db.insert_evidence_table(&row).unwrap();

        let loaded = db.get_evidence_table(&row.id).unwrap();
        let table: EvidenceTable = serde_json::from_str(&loaded.table_json).unwrap();
        assert_eq!(table.question, "delta");
        assert_eq!(table.rows.len(), resp.table.rows.len());
        let trace: EvidenceTrace = serde_json::from_str(loaded.trace_json.as_deref().unwrap()).unwrap();
        assert_eq!(trace.documents_matched, resp.trace.documents_matched);
    }
}
