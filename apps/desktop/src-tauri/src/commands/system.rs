//! System commands: settings, diagnostics, native pickers, import staging
//! and the document-engine probe.

use tauri::State;
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::services::settings::{Settings, Theme};
use crate::state::AppState;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    Ok(state.settings.lock().expect("settings lock").clone())
}

#[tauri::command]
pub fn set_theme(state: State<'_, AppState>, theme: String) -> AppResult<Settings> {
    let theme_value = match theme.as_str() {
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        "system" => Theme::System,
        other => {
            return Err(AppError::msg(format!(
                "Invalid theme value: {other}. Expected light, dark or system."
            )))
        }
    };
    {
        let mut s = state.settings.lock().expect("settings lock");
        s.theme = theme_value;
    }
    state.db.save_setting("theme", &theme)?;
    Ok(state.settings.lock().expect("settings lock").clone())
}

#[tauri::command]
pub fn get_system_info() -> AppResult<crate::services::hardware::SystemInfo> {
    crate::services::hardware::detect()
}

/// Toggle No-AI mode (spec §35). Returns the updated settings.
#[tauri::command]
pub fn set_ai_enabled(state: State<'_, AppState>, enabled: bool) -> AppResult<Settings> {
    {
        let mut s = state.settings.lock().expect("settings lock");
        s.ai_enabled = enabled;
    }
    state
        .db
        .save_setting("ai_enabled", if enabled { "true" } else { "false" })?;
    Ok(state.settings.lock().expect("settings lock").clone())
}

#[tauri::command]
pub fn run_diagnostics(
    state: State<'_, AppState>,
) -> AppResult<crate::services::diagnostics::DiagnosticsReport> {
    crate::services::diagnostics::run(&state)
}

#[tauri::command]
pub fn probe_document_engine() -> bool {
    crate::services::engine_client::health_ok()
}

/// Frontend diagnostics channel: the webview has no console of its own in a
/// packaged build, so media/IPC failures are reported here and land in the
/// app log where support can see them.
#[tauri::command]
pub fn log_frontend(message: String) {
    log::info!(target: "researchai::frontend", "{message}");
}


// ---------------------------------------------------------------------------
// Native file pickers (dialog plugin). The blocking variants must not run on
// the main thread, so they execute on a dedicated thread via spawn_blocking.
// ---------------------------------------------------------------------------

/// Native file picker for GGUF models (no extension filter — model files are
/// not in the document picker's allow-list).
#[tauri::command]
pub async fn ai_pick_model_file(app: tauri::AppHandle) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .ok()
    .flatten()
}

/// Native file picker for documents. The allow-list mirrors
/// `services::ingestion::SUPPORTED`, so a user can only select types the
/// document engine can actually parse.
#[tauri::command]
pub async fn pick_documents(app: tauri::AppHandle) -> Option<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Documents", crate::services::ingestion::SUPPORTED)
            .blocking_pick_files()
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(|p| p.clone().into_path().ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .collect()
            })
    })
    .await
    .ok()
    .flatten()
}


