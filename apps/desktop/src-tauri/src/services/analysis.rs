//! Grounded AI analysis pipeline (spec §16, §17).
//!
//! Every answer is assembled from numbered evidence excerpts — hybrid
//! retrieval for project/library scope, ordered chunks for single-document
//! scope — and the system prompt forbids claims without `[n]` citations
//! pointing at that evidence. Answers, evidence and a debug trace are
//! persisted to `analyses` (migration 4) so results are auditable later.
//!
//! The provider is injected ([`AiProvider`]); this module owns grounding,
//! prompt assembly and persistence, never transport.

use serde::Serialize;

use crate::db::{AnalysisRow, Db};
use crate::error::{AppError, AppResult};
use crate::services::ai::{AiProvider, CompletionOutput, CompletionRequest};
use crate::services::retrieval;

/// Bump when prompt wording changes so old analyses stay interpretable.
pub const PROMPT_VERSION: &str = "v1";

const DEFAULT_MAX_EVIDENCE: usize = 12;

// ---------------------------------------------------------------------------
// Modes (spec §17.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMode {
    Chat,
    Research,
    QuickRead,
    DeepAnalysis,
    Critical,
}

impl AnalysisMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnalysisMode::Chat => "chat",
            AnalysisMode::Research => "research",
            AnalysisMode::QuickRead => "quick_read",
            AnalysisMode::DeepAnalysis => "deep_analysis",
            AnalysisMode::Critical => "critical",
        }
    }

    pub fn from_str(s: &str) -> AppResult<Self> {
        Ok(match s {
            "chat" => AnalysisMode::Chat,
            "research" => AnalysisMode::Research,
            "quick_read" => AnalysisMode::QuickRead,
            "deep_analysis" => AnalysisMode::DeepAnalysis,
            "critical" => AnalysisMode::Critical,
            other => {
                return Err(AppError::msg(format!(
                    "Unknown analysis mode \"{other}\"."
                )))
            }
        })
    }

    fn system_prompt(&self) -> &'static str {
        match self {
            AnalysisMode::Chat => {
                "Mode: chat. Answer conversationally but precisely, grounded in the evidence. Cite the evidence for each claim."
            }
            AnalysisMode::Research => {
                "Mode: research. Be strict: separate what the evidence supports from what it merely suggests. Flag disagreements between sources and state uncertainty explicitly. Every claim must be cited."
            }
            AnalysisMode::QuickRead => {
                "Mode: quick read. Produce exactly these sections, each starting on its own line: \"Title:\", \"Question:\", \"Five findings:\" (numbered 1-5, each cited), \"Method:\", \"Conclusion:\", \"Limitations:\". Keep each finding to one sentence."
            }
            AnalysisMode::DeepAnalysis => {
                "Mode: deep analysis. Produce exactly these sections in order, each starting on its own line: \"WHAT:\", \"WHY:\", \"WHO:\", \"HOW:\", \"EVIDENCE:\", \"STRENGTHS:\", \"LIMITATIONS:\", \"GAPS:\". Ground every non-empty line in the evidence with citations."
            }
            AnalysisMode::Critical => {
                "Mode: critical review. Challenge the paper's methodology: identify hidden assumptions, methodological weaknesses, threats to validity and overclaims, each grounded in cited evidence. End with the three sharpest questions a reviewer should ask, numbered."
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Requests / responses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AskRequest {
    pub project_id: String,
    /// Empty = whole project; one id = single-document mode (ordered chunks).
    pub document_ids: Vec<String>,
    pub mode: AnalysisMode,
    pub question: String,
    pub max_evidence: usize,
    pub model_id: Option<String>,
    /// Generation limits from the user's AI settings (spec §34).
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisEvidence {
    pub chunk_id: String,
    pub document_id: String,
    pub document_name: String,
    pub page_number: Option<i64>,
    pub section_heading: Option<String>,
    pub text: String,
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisTrace {
    pub mode: String,
    pub engine: String,
    pub model: Option<String>,
    pub scope_documents: usize,
    pub evidence_count: usize,
    pub evidence_chars: usize,
    pub citations_used: Vec<usize>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub finish_reason: Option<String>,
    pub duration_ms: u64,
    pub embedding_coverage: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResponse {
    pub analysis_id: Option<String>,
    pub answer: String,
    pub evidence: Vec<AnalysisEvidence>,
    pub trace: AnalysisTrace,
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Everything needed to run the completion for one ask, prepared without
/// touching the provider (retrieval + prompt assembly).
pub(crate) struct PreparedAsk {
    pub evidence: Vec<AnalysisEvidence>,
    pub warnings: Vec<String>,
    pub system_prompt: String,
    pub user_prompt: String,
}

/// Outcome of ask preparation: either a ready-to-run completion, or an
/// already-complete response (no evidence matched — nothing to generate).
pub(crate) enum PreparedOutcome {
    NoMatches(AnalysisResponse),
    Ready(PreparedAsk),
}

/// Retrieval + prompt assembly shared by the blocking and streaming ask
/// paths. `provider_name` only feeds the no-match trace's engine field.
pub(crate) fn prepare_ask(
    db: &Db,
    provider_name: &str,
    req: &AskRequest,
) -> AppResult<PreparedOutcome> {
    let question = req.question.trim();
    if question.is_empty() {
        return Err(AppError::msg("The question must not be empty."));
    }

    let (evidence, retrieval_warnings) = gather_evidence(db, req)?;
    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(retrieval_warnings);

    if evidence.is_empty() {
        warnings.push(
            "No indexed sources matched this question — try Search first, or widen the scope."
                .into(),
        );
        let trace = AnalysisTrace {
            mode: req.mode.as_str().into(),
            engine: provider_name.into(),
            model: None,
            scope_documents: req.document_ids.len(),
            evidence_count: 0,
            evidence_chars: 0,
            citations_used: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
            finish_reason: None,
            duration_ms: 0,
            embedding_coverage: coverage(db)?,
            warnings,
        };
        return Ok(PreparedOutcome::NoMatches(AnalysisResponse {
            analysis_id: None,
            answer: "No indexed sources matched this question. Try Search to find the right "
                .to_string()
                + "documents, then ask again with a wider scope.",
            evidence: Vec::new(),
            trace,
        }));
    }

    let max_evidence = req
        .max_evidence
        .clamp(1, 20)
        .min(evidence.len());
    let evidence = evidence[..max_evidence].to_vec();

    let (system_prompt, user_prompt) = build_prompt(req.mode, question, &evidence);
    Ok(PreparedOutcome::Ready(PreparedAsk {
        evidence,
        warnings,
        system_prompt,
        user_prompt,
    }))
}

/// Trace + persistence shared by both ask paths, after the completion is in.
pub(crate) fn finish_ask(
    db: &Db,
    req: &AskRequest,
    evidence: &[AnalysisEvidence],
    mut warnings: Vec<String>,
    completion: CompletionOutput,
) -> AppResult<AnalysisResponse> {
    let question = req.question.trim();

    // Prompt-cache telemetry (see build_prompt): repeated asks over the same
    // evidence should show a much lower ms/token than the first ask, because
    // the shared evidence prefix comes from llama-server's KV cache.
    log::info!(
        target: "researchai::ai",
        "ask complete: mode={} evidence={} prompt_tokens={:?} completion_tokens={:?} duration_ms={} ms_per_token={:.1}",
        req.mode.as_str(),
        evidence.len(),
        completion.prompt_tokens,
        completion.completion_tokens,
        completion.duration_ms,
        if completion.completion_tokens.unwrap_or(0) > 0 {
            completion.duration_ms as f64
                / completion.completion_tokens.unwrap() as f64
        } else {
            0.0
        },
    );

    let citations_used = used_citations(&completion.text, evidence.len());
    if citations_used.is_empty() {
        warnings.push(
            "The answer cited no numbered evidence — treat it with caution (ungrounded).".into(),
        );
    }

    let trace = AnalysisTrace {
        mode: req.mode.as_str().into(),
        engine: completion.engine.clone(),
        model: completion.model.clone(),
        scope_documents: req.document_ids.len(),
        evidence_count: evidence.len(),
        evidence_chars: evidence.iter().map(|e| e.text.len()).sum(),
        citations_used,
        prompt_tokens: completion.prompt_tokens,
        completion_tokens: completion.completion_tokens,
        finish_reason: completion.finish_reason.clone(),
        duration_ms: completion.duration_ms,
        embedding_coverage: coverage(db)?,
        warnings,
    };

    // Persist (spec §17: analyses are recorded and re-openable).
    let row = AnalysisRow {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: req.project_id.clone(),
        document_id: if req.document_ids.len() == 1 {
            Some(req.document_ids[0].clone())
        } else {
            None
        },
        analysis_type: req.mode.as_str().into(),
        model_id: req.model_id.clone(),
        prompt_version: PROMPT_VERSION.into(),
        question: Some(question.into()),
        answer_text: completion.text.clone(),
        evidence_json: serde_json::to_string(&evidence)
            .map_err(|e| AppError::msg(format!("Could not serialise evidence: {e}")))?,
        trace_json: Some(
            serde_json::to_string(&trace)
                .map_err(|e| AppError::msg(format!("Could not serialise trace: {e}")))?,
        ),
        created_at: crate::db::now_iso_pub(),
    };
    db.insert_analysis(&row)?;

    Ok(AnalysisResponse {
        analysis_id: Some(row.id),
        answer: completion.text,
        evidence: evidence.to_vec(),
        trace,
    })
}

/// Run one grounded ask (blocking completion). The provider is a parameter so
/// tests (and future cloud providers) can inject any implementation.
pub fn ask(db: &Db, provider: &dyn AiProvider, req: &AskRequest) -> AppResult<AnalysisResponse> {
    let prepared = match prepare_ask(db, provider.name(), req)? {
        PreparedOutcome::NoMatches(resp) => return Ok(resp),
        PreparedOutcome::Ready(p) => p,
    };

    let completion = provider.complete(&CompletionRequest {
        system_prompt: prepared.system_prompt,
        user_prompt: prepared.user_prompt,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
    })?;

    finish_ask(db, req, &prepared.evidence, prepared.warnings, completion)
}

/// Streaming variant: `on_delta` receives incremental answer text as it is
/// generated; the returned response is identical to [`ask`] (full text
/// persisted, same trace). Nothing is persisted until the stream ends, so an
/// interrupted stream leaves no partial analysis behind.
pub fn ask_streaming(
    db: &Db,
    provider: &dyn AiProvider,
    req: &AskRequest,
    on_delta: &mut dyn FnMut(&str),
) -> AppResult<AnalysisResponse> {
    let prepared = match prepare_ask(db, provider.name(), req)? {
        PreparedOutcome::NoMatches(resp) => return Ok(resp),
        PreparedOutcome::Ready(p) => p,
    };

    let completion = provider.complete_stream(
        &CompletionRequest {
            system_prompt: prepared.system_prompt,
            user_prompt: prepared.user_prompt,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
        },
        on_delta,
    )?;

    finish_ask(db, req, &prepared.evidence, prepared.warnings, completion)
}

// ---------------------------------------------------------------------------
// Evidence gathering
// ---------------------------------------------------------------------------

/// Single-document scope → chunks in reading order (deterministic); otherwise
/// hybrid retrieval over the given scope (empty = whole project). Returns the
/// evidence plus the retrieval trace's warnings (engine offline, coverage…)
/// so the analysis debug panel shows why the evidence looks the way it does.
fn gather_evidence(
    db: &Db,
    req: &AskRequest,
) -> AppResult<(Vec<AnalysisEvidence>, Vec<String>)> {
    if req.document_ids.len() == 1 {
        let chunks = chunks_for_document(db, &req.document_ids[0], req.max_evidence.clamp(1, 20))?;
        return Ok((chunks, Vec::new()));
    }
    let response =
        retrieval::search(db, req.question.trim(), &req.document_ids, DEFAULT_MAX_EVIDENCE)?;
    let evidence = response
        .hits
        .into_iter()
        .map(|h| AnalysisEvidence {
            chunk_id: h.chunk_id,
            document_id: h.document_id,
            document_name: h.document_name,
            page_number: h.page_number,
            section_heading: h.section_heading,
            text: h.text,
            start_offset: h.start_offset,
            end_offset: h.end_offset,
        })
        .collect();
    Ok((evidence, response.trace.warnings))
}

fn chunks_for_document(
    db: &Db,
    document_id: &str,
    limit: usize,
) -> AppResult<Vec<AnalysisEvidence>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT c.id, c.document_id, d.file_name, c.page_number, s.heading,
                c.text, c.start_offset, c.end_offset
         FROM chunks c
         JOIN documents d ON d.id = c.document_id
         LEFT JOIN document_sections s ON s.id = c.section_id
         WHERE c.document_id = ?1
         ORDER BY c.chunk_index
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![document_id, limit as i64], |row| {
            Ok(AnalysisEvidence {
                chunk_id: row.get(0)?,
                document_id: row.get(1)?,
                document_name: row.get(2)?,
                page_number: row.get(3)?,
                section_heading: row.get(4)?,
                text: row.get(5)?,
                start_offset: row.get(6)?,
                end_offset: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Prompt assembly
// ---------------------------------------------------------------------------

const SYSTEM_PREAMBLE: &str = "You are ResearchAI, a rigorous research assistant working with a private, local document library. You answer ONLY from the numbered evidence excerpts provided. Rules:\n\
1. Every factual claim must end with a citation marker like [1] or [2][3] referring to the evidence it comes from.\n\
2. Never cite a number outside the provided evidence range; never invent page numbers, authors or findings.\n\
3. If the evidence is insufficient, say so plainly and suggest what the user should look for instead of speculating.\n\
4. Quote sparingly and exactly. Write clearly for a busy researcher.";

fn build_prompt(
    mode: AnalysisMode,
    question: &str,
    evidence: &[AnalysisEvidence],
) -> (String, String) {
    let system = format!("{SYSTEM_PREAMBLE}\n\n{}", mode.system_prompt());

    // Evidence first, question last: llama-server's prompt-prefix KV cache
    // reuses everything up to the first differing token, so keeping the
    // (large) evidence block before the (small, per-ask) question lets
    // repeated asks over the same evidence — regenerate, follow-up asks
    // with a new question, retries — skip re-processing the whole context.
    // (A mode switch still misses: it changes the system prompt.)
    let mut user = String::from("Evidence:\n");
    for (i, e) in evidence.iter().enumerate() {
        let page = e
            .page_number
            .map(|p| format!(", p. {p}"))
            .unwrap_or_default();
        user.push_str(&format!("[{}] ({}{}) {}\n\n", i + 1, e.document_name, page, e.text));
    }
    user.push_str(&format!("Question: {question}\n"));
    (system, user)
}

/// Extract the valid citation numbers used in an answer, in first-use order.
/// Citations beyond the evidence count are ignored (the model must not invent
/// sources, and silently dropping them keeps the UI honest).
pub(crate) fn used_citations(answer: &str, max: usize) -> Vec<usize> {
    let mut used = Vec::new();
    let bytes = answer.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close) = answer[i + 1..].find(']') {
                let inner = &answer[i + 1..i + 1 + close];
                if let Ok(n) = inner.trim().parse::<usize>() {
                    if n >= 1 && n <= max && !used.contains(&n) {
                        used.push(n);
                    }
                    i += close + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    used
}

fn coverage(db: &Db) -> AppResult<String> {
    let store = retrieval::VectorStore::new(db)?;
    let (total, embedded) = store.stats()?;
    Ok(format!("{embedded}/{total} chunks embedded"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::{temp_dir_for, TempDir};
    use crate::services::ai::{CompletionOutput, EchoProvider};
    use crate::services::documents::{EngineBlock, EngineParseResponse};

    fn db_with_project(label: &str) -> (TempDir, Db, String) {
        let dir = temp_dir_for(label);
        let db = Db::open(dir.path()).unwrap();
        let project = db.create_project("Ask tests", None).unwrap();
        (dir, db, project.id)
    }

    fn add_document(db: &Db, project_id: &str, name: &str, texts: &[&str]) -> String {
        let doc_id = uuid::Uuid::new_v4().to_string();
        db.insert_document(crate::services::documents::DocumentRow {
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
            page_count: Some(3),
            language: None,
            imported_at: crate::db::now_iso_pub(),
            chunk_count: 0,
        })
        .unwrap();

        let parsed = EngineParseResponse {
            document_id: doc_id.clone(),
            ok: true,
            page_count: Some(3),
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
                    page: Some((i as i64 % 3) + 1),
                    start_offset: Some(i as i64 * 100),
                    end_offset: Some(i as i64 * 100 + t.len() as i64),
                })
                .collect(),
            error: None,
            doi: None,
            year: None,
        };
        db.replace_sections_and_chunks(&doc_id, &parsed).unwrap();
        doc_id
    }

    fn ask_request(project_id: &str, mode: AnalysisMode, question: &str, docs: Vec<String>) -> AskRequest {
        AskRequest {
            project_id: project_id.into(),
            document_ids: docs,
            mode,
            question: question.into(),
            max_evidence: 12,
            model_id: None,
            max_tokens: 256,
            temperature: 0.2,
        }
    }

    #[test]
    fn single_document_mode_uses_ordered_chunks_and_persists() {
        let (_dir, db, project) = db_with_project("analysis-single");
        let doc = add_document(
            &db,
            &project,
            "deltas.txt",
            &[
                "Coastal deltas retreat faster than inland shorelines under equal sea-level rise.",
                "Sediment starvation amplifies the retreat of river deltas.",
                "Unrelated paragraph about memory consolidation in mice.",
            ],
        );

        let req = ask_request(&project, AnalysisMode::Research, "Why do deltas retreat?", vec![doc]);
        let resp = ask(&db, &EchoProvider, &req).unwrap();

        assert_eq!(resp.evidence.len(), 3, "ordered chunks become evidence");
        assert_eq!(resp.evidence[0].document_name, "deltas.txt");
        assert!(resp.answer.contains("[1]"), "echo echoes the evidence list: {}", resp.answer);
        assert!(!resp.trace.citations_used.is_empty());
        assert_eq!(resp.trace.mode, "research");
        assert_eq!(resp.trace.engine, "echo");

        // Persisted for later review.
        let rows = db.list_analyses(&project, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].analysis_type, "research");
        assert_eq!(rows[0].document_id.as_deref(), Some(resp.evidence[0].document_id.as_str()));
        assert!(rows[0].trace_json.is_some());
    }

    #[test]
    fn project_scope_gathers_hits_across_documents() {
        let (_dir, db, project) = db_with_project("analysis-scope");
        let doc_a = add_document(
            &db,
            &project,
            "a-deltas.txt",
            &["River deltas shrink when sediment supply falls below sea-level rise."],
        );
        let doc_b = add_document(
            &db,
            &project,
            "b-deltas.txt",
            &["Deltas worldwide face compound flooding and subsidence pressures."],
        );

        let req = ask_request(&project, AnalysisMode::Chat, "deltas", vec![]);
        let resp = ask(&db, &EchoProvider, &req).unwrap();

        assert!(!resp.evidence.is_empty());
        let docs: std::collections::HashSet<&str> =
            resp.evidence.iter().map(|e| e.document_id.as_str()).collect();
        assert!(docs.contains(doc_a.as_str()), "doc A evidence missing");
        assert!(docs.contains(doc_b.as_str()), "doc B evidence missing");
        // Environment-agnostic assertions (the sidecar may or may not be up):
        // with no embeddings stored, coverage is always zero and the vector
        // channel can only ever return zero candidates.
        assert!(resp.trace.embedding_coverage.starts_with("0/"));
        if !crate::services::engine_client::health_ok() {
            assert!(resp
                .trace
                .warnings
                .iter()
                .any(|w| w.contains("engine offline") || w.contains("Semantic")));
        }
    }

    #[test]
    fn no_matches_short_circuits_without_persisting() {
        let (_dir, db, project) = db_with_project("analysis-empty");
        add_document(&db, &project, "a.txt", &["Coastal adaptation content."]);

        let req = ask_request(&project, AnalysisMode::Chat, "zzzqqq gibberish", vec![]);
        let resp = ask(&db, &EchoProvider, &req).unwrap();

        assert!(resp.evidence.is_empty());
        assert!(resp.analysis_id.is_none());
        assert!(resp.answer.contains("No indexed sources matched"));
        assert!(db.list_analyses(&project, 10).unwrap().is_empty());
    }

    #[test]
    fn empty_question_is_rejected() {
        let (_dir, db, project) = db_with_project("analysis-blank");
        let err = ask(&db, &EchoProvider, &ask_request(&project, AnalysisMode::Chat, "   ", vec![]))
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn mode_prompts_are_distinct_and_strict() {
        let evidence = vec![AnalysisEvidence {
            chunk_id: "c1".into(),
            document_id: "d1".into(),
            document_name: "paper.txt".into(),
            page_number: Some(2),
            section_heading: None,
            text: "Some claim.".into(),
            start_offset: None,
            end_offset: None,
        }];

        let (chat_sys, chat_user) = build_prompt(AnalysisMode::Chat, "Q?", &evidence);
        let (deep_sys, _) = build_prompt(AnalysisMode::DeepAnalysis, "Q?", &evidence);
        let (quick_sys, _) = build_prompt(AnalysisMode::QuickRead, "Q?", &evidence);

        assert!(chat_sys.contains("ONLY from the numbered evidence"));
        assert!(chat_sys.contains("Mode: chat."));
        assert!(deep_sys.contains("GAPS:"));
        assert!(quick_sys.contains("Five findings:"));
        assert_ne!(chat_sys, deep_sys);

        assert!(chat_user.starts_with("Evidence:\n"));
        assert!(chat_user.ends_with("Question: Q?\n"));
        assert!(chat_user.contains("[1] (paper.txt, p. 2) Some claim."));
    }

    #[test]
    fn citation_parser_accepts_valid_and_ignores_out_of_range() {
        assert_eq!(used_citations("[1] then [4] and [2]", 2), vec![1, 2]);
        assert_eq!(used_citations("no citations here", 3), Vec::<usize>::new());
        assert_eq!(used_citations("[3] repeated [3] [1]", 3), vec![3, 1]);
        // Non-citation brackets are ignored.
        assert_eq!(used_citations("see [appendix] and [ 2 ]", 3), vec![2]);
    }

    /// A provider that returns a fixed answer, for the ungrounded-answer path.
    struct FixedProvider(&'static str);
    impl AiProvider for FixedProvider {
        fn name(&self) -> &'static str {
            "fixed"
        }
        fn complete(&self, _req: &CompletionRequest) -> AppResult<CompletionOutput> {
            Ok(CompletionOutput {
                text: self.0.into(),
                engine: "fixed".into(),
                model: None,
                prompt_tokens: None,
                completion_tokens: None,
                duration_ms: 1,
                finish_reason: Some("stop".into()),
            })
        }
        fn complete_stream(
            &self,
            _req: &CompletionRequest,
            _on_delta: &mut dyn FnMut(&str),
        ) -> AppResult<CompletionOutput> {
            self.complete(_req)
        }
    }

    #[test]
    fn ungrounded_answers_are_flagged_but_returned() {
        let (_dir, db, project) = db_with_project("analysis-ungrounded");
        add_document(&db, &project, "a.txt", &["Coastal deltas retreat."]);

        let req = ask_request(&project, AnalysisMode::Chat, "deltas", vec![]);
        let resp = ask(&db, &FixedProvider("I believe deltas are interesting."), &req).unwrap();

        assert!(resp.trace.citations_used.is_empty());
        assert!(resp
            .trace
            .warnings
            .iter()
            .any(|w| w.contains("cited no numbered evidence")));
        assert_eq!(resp.answer, "I believe deltas are interesting.");
        // Still persisted — the audit trail matters more than the answer.
        assert_eq!(db.list_analyses(&project, 10).unwrap().len(), 1);
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(AnalysisMode::from_str("poetry").is_err());
        assert_eq!(AnalysisMode::from_str("quick_read").unwrap(), AnalysisMode::QuickRead);
    }
}
