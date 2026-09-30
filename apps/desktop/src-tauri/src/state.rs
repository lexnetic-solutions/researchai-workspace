//! Application state managed by Tauri: resolved data directory, SQLite
//! handle and cached settings.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::Manager;

use crate::db;
use crate::error::{AppError, AppResult};
use crate::services::llm_runtime::LlmRuntime;
use crate::services::queue::IngestionQueue;
use crate::services::settings::Settings;

pub struct AppState {
    pub data_dir: PathBuf,
    pub db: db::Db,
    pub settings: Mutex<Settings>,
    /// Owns the background ingestion worker; dropped on app exit.
    pub _queue: IngestionQueue,
    /// Owns the llama-server supervisor; kills the child on app exit.
    pub llm_runtime: LlmRuntime,
}

impl AppState {
    /// Open (creating if needed) the database, load settings and start the
    /// background ingestion worker (spec §14).
    pub fn initialize(data_dir: PathBuf) -> AppResult<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let db = db::Db::open(&data_dir)?;
        let settings = db.load_settings()?;
        let queue = IngestionQueue::start(data_dir.clone());
        // Sweep leftover llama-server processes from previous runs, then
        // start the supervisor thread (spec §43).
        let llm_runtime = LlmRuntime::start(&data_dir, &data_dir.join("models"));
        Ok(Self {
            data_dir,
            db,
            settings: Mutex::new(settings),
            _queue: queue,
            llm_runtime,
        })
    }
}

/// Resolve the managed workspace directory (spec §13):
/// `<OS app-data>/ResearchAI` — e.g. `~/Library/Application Support/ResearchAI`
/// on macOS. Overridable via `RESEARCHAI_DATA_DIR` for dev/testing.
pub fn resolve_app_data_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    if let Ok(custom) = std::env::var("RESEARCHAI_DATA_DIR") {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::msg(format!("Could not resolve app data directory: {e}")))?;
    Ok(base)
}
