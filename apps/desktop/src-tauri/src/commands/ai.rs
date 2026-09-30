//! AI commands (Phase 3, spec §17): model library, runtime control and the
//! grounded ask pipeline.
//!
//! Heavy operations (model import/download, llama-server load, generation)
//! run on blocking threads via `spawn_blocking` — the main thread never
//! stalls. Blocking workers that need the database open their own
//! connection (WAL allows multiple readers), exactly like the ingestion
//! queue worker does.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::{AiSettings, LocalModelRow};
use crate::error::{AppError, AppResult};
use crate::services::analysis::{self, AnalysisMode, AnalysisResponse};
use crate::services::llm_runtime::{LoadState, RuntimeStatus};
use crate::services::model_manager::ModelManager;
use crate::state::AppState;

/// How long a model load (weight loading into RAM/VRAM) may take.
const LOAD_DEADLINE: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// Model library
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn ai_list_models(state: State<'_, AppState>) -> AppResult<Vec<LocalModelRow>> {
    // Reconcile with the filesystem so statuses are honest after users move
    // or delete files behind the app's back.
    let mgr = ModelManager::new(&state.data_dir);
    mgr.scan_for_missing(&state.db)?;
    state.db.list_local_models()
}

#[tauri::command]
pub async fn ai_add_model(app: AppHandle, path: String) -> AppResult<LocalModelRow> {
    if path.trim().is_empty() {
        return Err(AppError::msg("No file was selected."));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let mgr = ModelManager::new(&state.data_dir);
        // Local disk copy — fast; no cancellation UI needed.
        mgr.import_from_path(
            &state.db,
            std::path::Path::new(&path),
            &Arc::new(AtomicBool::new(false)),
        )
    })
    .await
    .map_err(|e| AppError::msg(format!("Model import failed to run: {e}")))?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadEvent {
    pub job_id: u64,
    pub file_name: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// "running" | "done" | "failed" | "cancelled"
    pub state: String,
    pub error: Option<String>,
    pub model: Option<LocalModelRow>,
}

static DOWNLOAD_SEQ: AtomicU64 = AtomicU64::new(1);

fn download_jobs() -> &'static Mutex<HashMap<u64, Arc<AtomicBool>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<AtomicBool>>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start a model download. Returns a job id immediately; progress arrives as
/// `ai://model-download` events (see [`ModelDownloadEvent`]).
#[tauri::command]
pub async fn ai_download_model(app: AppHandle, url: String) -> AppResult<u64> {
    if url.trim().is_empty() {
        return Err(AppError::msg("No download URL was given."));
    }
    let job_id = DOWNLOAD_SEQ.fetch_add(1, Ordering::SeqCst);
    let cancel = Arc::new(AtomicBool::new(false));
    download_jobs()
        .lock()
        .expect("download jobs lock")
        .insert(job_id, Arc::clone(&cancel));

    let cancel_thread = Arc::clone(&cancel);
    tauri::async_runtime::spawn(async move {
        let data_dir = app.state::<AppState>().data_dir.clone();
        let result = blocking_download(&app, data_dir, url, cancel_thread, job_id);
        download_jobs()
            .lock()
            .expect("download jobs lock")
            .remove(&job_id);
        if let Err(e) = result {
            log::warn!(target: "researchai::ai", "download job {job_id} errored: {e}");
        }
    });
    Ok(job_id)
}

/// Runs the blocking download on this thread (already inside a spawned
/// async task) and emits progress/end events.
fn blocking_download(
    app: &AppHandle,
    data_dir: std::path::PathBuf,
    url: String,
    cancel: Arc<AtomicBool>,
    job_id: u64,
) -> AppResult<()> {
    let mgr = ModelManager::new(&data_dir);
    let emit = |ev: ModelDownloadEvent| {
        let _ = app.emit("ai://model-download", ev);
    };

    let file_name = url.split(['?', '#']).next().and_then(|p| p.rsplit('/').next())
        .unwrap_or("model.gguf").to_string();
    let result = mgr.download(
        // Own connection for this thread (queue-worker pattern).
        &crate::db::Db::open(&data_dir)?,
        &url,
        &cancel,
        &mut |done, total| {
            emit(ModelDownloadEvent {
                job_id,
                file_name: file_name.clone(),
                downloaded_bytes: done,
                total_bytes: total,
                state: "running".into(),
                error: None,
                model: None,
            });
        },
    );

    match result {
        Ok(row) => {
            emit(ModelDownloadEvent {
                job_id,
                file_name: row.file_name.clone(),
                downloaded_bytes: row.size_bytes as u64,
                total_bytes: row.size_bytes as u64,
                state: "done".into(),
                error: None,
                model: Some(row),
            });
            Ok(())
        }
        Err(e) => {
            let cancelled = e.to_string().contains("cancelled");
            emit(ModelDownloadEvent {
                job_id,
                file_name,
                downloaded_bytes: 0,
                total_bytes: 0,
                state: if cancelled { "cancelled" } else { "failed" }.into(),
                error: Some(e.to_string()),
                model: None,
            });
            Err(e)
        }
    }
}

#[tauri::command]
pub fn ai_cancel_download(job_id: u64) -> bool {
    if let Some(flag) = download_jobs()
        .lock()
        .expect("download jobs lock")
        .get(&job_id)
    {
        flag.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

#[tauri::command]
pub async fn ai_delete_model(
    app: AppHandle,
    model_id: String,
    remove_file: bool,
) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        ModelManager::new(&state.data_dir).delete_model(&state.db, &model_id, remove_file)
    })
    .await
    .map_err(|e| AppError::msg(format!("Model deletion failed to run: {e}")))?
}

// ---------------------------------------------------------------------------
// AI settings & runtime control
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn ai_get_settings(state: State<'_, AppState>) -> AppResult<AiSettings> {
    state.db.get_ai_settings()
}

#[tauri::command]
pub fn ai_save_settings(state: State<'_, AppState>, settings: AiSettings) -> AppResult<AiSettings> {
    let mut s = settings;
    s.context_size = s.context_size.clamp(512, 131_072);
    s.max_tokens = s.max_tokens.clamp(16, 8_192);
    s.temperature = s.temperature.clamp(0.0, 2.0);
    state.db.save_ai_settings(&s)?;
    Ok(s)
}

#[tauri::command]
pub fn ai_runtime_status(state: State<'_, AppState>) -> RuntimeStatus {
    state.llm_runtime.snapshot()
}

/// Resolve which model should run: explicit id, else the active setting.
fn resolve_model(db: &crate::db::Db, settings: &AiSettings) -> AppResult<LocalModelRow> {
    let id = settings
        .active_model_id
        .as_deref()
        .ok_or_else(|| AppError::msg(
            "No local model is selected. Add a GGUF model in Settings → Local AI and choose one.",
        ))?;
    let model = db.get_local_model(id)?;
    if model.status != "available" {
        return Err(AppError::msg(format!(
            "The selected model file is not available on disk ({}). Re-scan or re-add it.",
            model.status
        )));
    }
    Ok(model)
}

/// Load the configured model (blocks until Ready or Failed).
#[tauri::command]
pub async fn ai_load_model(app: AppHandle) -> AppResult<RuntimeStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = state.db.get_ai_settings()?;
        let model = resolve_model(&state.db, &settings)?;
        let binary = std::path::PathBuf::from(&settings.llama_server_path);
        match state
            .llm_runtime
            .ensure_loaded(&binary, std::path::Path::new(&model.file_path), &settings, LOAD_DEADLINE)?
        {
            LoadState::Ready { .. } => Ok(state.llm_runtime.snapshot()),
            LoadState::Failed { detail } => Err(AppError::msg(detail)),
            _ => Ok(state.llm_runtime.snapshot()),
        }
    })
    .await
    .map_err(|e| AppError::msg(format!("Model load failed to run: {e}")))?
}

#[tauri::command]
pub fn ai_unload_model(state: State<'_, AppState>) -> RuntimeStatus {
    state.llm_runtime.unload();
    state.llm_runtime.snapshot()
}

// ---------------------------------------------------------------------------
// Ask pipeline
// ---------------------------------------------------------------------------

/// Light row for the analyses history list.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisSummary {
    pub id: String,
    pub analysis_type: String,
    pub question: Option<String>,
    pub created_at: String,
    pub model_id: Option<String>,
}

#[tauri::command]
pub async fn ai_ask(
    app: AppHandle,
    project_id: String,
    document_ids: Vec<String>,
    mode: String,
    question: String,
) -> AppResult<AnalysisResponse> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();

        // No-AI mode is a hard gate (spec §35).
        if !state
            .settings
            .lock()
            .expect("settings lock")
            .ai_enabled
        {
            return Err(AppError::msg(
                "AI is disabled (No-AI mode). Enable it in Settings to use Ask features.",
            ));
        }

        let mode = AnalysisMode::from_str(&mode)?;
        let ai = state.db.get_ai_settings()?;

        // Build the provider: the local llama.cpp runtime, loaded on demand.
        // First Ask triggers the model load automatically.
        let model = resolve_model(&state.db, &ai)?;
        let binary = std::path::PathBuf::from(&ai.llama_server_path);
        if !matches!(state.llm_runtime.snapshot().state, LoadState::Ready { .. }) {
            state.llm_runtime.ensure_loaded(
                &binary,
                std::path::Path::new(&model.file_path),
                &ai,
                LOAD_DEADLINE,
            )?;
        }
        state.llm_runtime.touch();
        let snapshot = state.llm_runtime.snapshot();
        let port = match snapshot.state {
            LoadState::Ready { port } => port,
            LoadState::Failed { detail } => return Err(AppError::msg(detail)),
            _ => return Err(AppError::msg("The local model is not ready yet.")),
        };

        let provider = crate::services::ai::LlamaCppProvider::new(
            &format!("http://127.0.0.1:{port}"),
            &model.file_name,
            None,
        );

        let req = analysis::AskRequest {
            project_id: project_id.clone(),
            document_ids: document_ids.clone(),
            mode,
            question: question.clone(),
            max_evidence: 12,
            model_id: Some(model.id.clone()),
            max_tokens: ai.max_tokens,
            temperature: ai.temperature,
        };
        analysis::ask(&state.db, &provider, &req)
    })
    .await
    .map_err(|e| AppError::msg(format!("AI request failed to run: {e}")))?
}

#[tauri::command]
pub fn ai_list_analyses(
    state: State<'_, AppState>,
    project_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<AnalysisSummary>> {
    Ok(state
        .db
        .list_analyses(&project_id, limit.unwrap_or(50).clamp(1, 200))?
        .into_iter()
        .map(|row| AnalysisSummary {
            id: row.id,
            analysis_type: row.analysis_type,
            question: row.question,
            created_at: row.created_at,
            model_id: row.model_id,
        })
        .collect())
}

/// Re-open a stored analysis with its evidence and trace.
#[tauri::command]
pub fn ai_get_analysis(
    state: State<'_, AppState>,
    analysis_id: String,
) -> AppResult<AnalysisResponse> {
    let row = state.db.get_analysis(&analysis_id)?;
    let evidence: Vec<analysis::AnalysisEvidence> = serde_json::from_str(&row.evidence_json)
        .unwrap_or_default();
    let trace: analysis::AnalysisTrace = row
        .trace_json
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(default_trace);
    Ok(AnalysisResponse {
        analysis_id: Some(row.id),
        answer: row.answer_text,
        evidence,
        trace,
    })
}

fn default_trace() -> analysis::AnalysisTrace {
    analysis::AnalysisTrace {
        mode: "unknown".into(),
        engine: "unknown".into(),
        model: None,
        scope_documents: 0,
        evidence_count: 0,
        evidence_chars: 0,
        citations_used: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
        finish_reason: None,
        duration_ms: 0,
        embedding_coverage: String::new(),
        warnings: Vec::new(),
    }
}
