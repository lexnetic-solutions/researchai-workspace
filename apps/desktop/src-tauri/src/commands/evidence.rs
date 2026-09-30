//! Evidence commands (Phase 4, spec §18): build, save and re-open
//! cross-document evidence matrices.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::db::EvidenceTableRow;
use crate::error::{AppError, AppResult};
use crate::services::evidence::{self, EvidenceResponse, EvidenceTable, EvidenceTrace};
use crate::services::llm_runtime::LoadState;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSummary {
    pub id: String,
    pub question: String,
    pub created_at: String,
    /// true when the table includes AI findings/synthesis.
    pub has_ai: bool,
    pub model_id: Option<String>,
}

/// Build (but do not persist) an evidence matrix for a question.
#[tauri::command]
pub async fn evidence_build(
    app: AppHandle,
    project_id: String,
    document_ids: Vec<String>,
    question: String,
    with_ai: bool,
) -> AppResult<EvidenceResponse> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();

        // AI pass requires the No-AI gate to be open (spec §35). Without it
        // the table is still built — deterministically.
        let ai_allowed = state.settings.lock().expect("settings lock").ai_enabled;

        let mut model_id: Option<String> = None;
        let provider: Option<Box<dyn crate::services::ai::AiProvider>> = if with_ai && ai_allowed {
            let ai = state.db.get_ai_settings()?;
            let model = resolve_available_model(&state, &ai)?;
            model_id = Some(model.id.clone());
            let binary = std::path::PathBuf::from(&ai.llama_server_path);
            if !matches!(state.llm_runtime.snapshot().state, LoadState::Ready { .. }) {
                state.llm_runtime.ensure_loaded(
                    &binary,
                    std::path::Path::new(&model.file_path),
                    &ai,
                    std::time::Duration::from_secs(600),
                )?;
            }
            state.llm_runtime.touch();
            let port = match state.llm_runtime.snapshot().state {
                LoadState::Ready { port } => port,
                LoadState::Failed { detail } => return Err(AppError::msg(detail)),
                _ => return Err(AppError::msg("The local model is not ready yet.")),
            };
            Some(Box::new(crate::services::ai::LlamaCppProvider::new(
                &format!("http://127.0.0.1:{port}"),
                &model.file_name,
                None,
            )) as Box<dyn crate::services::ai::AiProvider>)
        } else {
            None
        };

        let req = evidence::EvidenceRequest {
            project_id: project_id.clone(),
            document_ids: document_ids.clone(),
            question: question.clone(),
            with_ai,
            model_id: if provider.is_some() { model_id } else { None },
        };
        evidence::build_table(&state.db, provider.as_deref(), &req)
    })
    .await
    .map_err(|e| AppError::msg(format!("Evidence build failed to run: {e}")))?
}

fn resolve_available_model(
    state: &State<'_, AppState>,
    ai: &crate::db::AiSettings,
) -> AppResult<crate::db::LocalModelRow> {
    let id = ai.active_model_id.as_deref().ok_or_else(|| {
        AppError::msg("No local model is selected. Add a GGUF model in Settings → Local AI.")
    })?;
    let model = state.db.get_local_model(id)?;
    if model.status != "available" {
        return Err(AppError::msg(format!(
            "The selected model file is not available on disk ({}).",
            model.status
        )));
    }
    Ok(model)
}

/// Persist a built table (Save button) and return its id.
#[tauri::command]
pub fn evidence_save(
    state: State<'_, AppState>,
    project_id: String,
    question: String,
    scope_document_ids: Vec<String>,
    table: EvidenceTable,
    trace: EvidenceTrace,
    model_id: Option<String>,
) -> AppResult<String> {
    let row = EvidenceTableRow {
        id: uuid::Uuid::new_v4().to_string(),
        project_id,
        question,
        scope_json: serde_json::to_string(&scope_document_ids)
            .map_err(|e| AppError::msg(format!("Could not serialise scope: {e}")))?,
        table_json: serde_json::to_string(&table)
            .map_err(|e| AppError::msg(format!("Could not serialise table: {e}")))?,
        trace_json: Some(
            serde_json::to_string(&trace)
                .map_err(|e| AppError::msg(format!("Could not serialise trace: {e}")))?,
        ),
        model_id,
        prompt_version: evidence::EVIDENCE_PROMPT_VERSION.into(),
        created_at: crate::db::now_iso_pub(),
    };
    state.db.insert_evidence_table(&row)?;
    Ok(row.id)
}

#[tauri::command]
pub fn evidence_list(
    state: State<'_, AppState>,
    project_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<EvidenceSummary>> {
    Ok(state
        .db
        .list_evidence_tables(&project_id, limit.unwrap_or(50).clamp(1, 200))?
        .into_iter()
        .map(|row| EvidenceSummary {
            has_ai: row.model_id.is_some(),
            id: row.id,
            question: row.question,
            created_at: row.created_at,
            model_id: row.model_id,
        })
        .collect())
}

/// Re-open a saved table with its full layout and trace.
#[tauri::command]
pub fn evidence_get(
    state: State<'_, AppState>,
    table_id: String,
) -> AppResult<EvidenceResponse> {
    let row = state.db.get_evidence_table(&table_id)?;
    let table: EvidenceTable = serde_json::from_str(&row.table_json)
        .map_err(|e| AppError::msg(format!("Saved table is unreadable: {e}")))?;
    let trace: EvidenceTrace = row
        .trace_json
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| EvidenceTrace {
            mode: "unknown".into(),
            engine: "unknown".into(),
            model: None,
            scope_documents: 0,
            documents_matched: 0,
            total_excerpts: 0,
            strength_counts: Default::default(),
            retrieval_warnings: Vec::new(),
            warnings: Vec::new(),
            duration_ms: 0,
        });
    Ok(EvidenceResponse {
        table_id: Some(row.id),
        model_id: row.model_id,
        table,
        trace,
    })
}

#[tauri::command]
pub fn evidence_delete(state: State<'_, AppState>, table_id: String) -> AppResult<()> {
    state.db.delete_evidence_table(&table_id)
}
