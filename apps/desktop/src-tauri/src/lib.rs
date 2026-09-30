//! ResearchAI Workspace — native backend library.
//!
//! The Rust core owns the SQLite database, file storage, settings,
//! diagnostics and logging. Frontend features talk to it exclusively
//! through the commands registered in [`commands`].
//!
//! Cross-platform rule (spec §2.3): no OS-specific paths or shell commands
//! in this crate; storage layout is resolved in `services::storage`.

pub mod commands;
pub mod db;
pub mod error;
pub mod logging;
pub mod services;
pub mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = state::resolve_app_data_dir(&handle)?;
            logging::init(&data_dir)?;
            log::info!(target: "researchai", "starting; data dir = {}", data_dir.display());

            let app_state = state::AppState::initialize(data_dir)?;
            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::projects::list_projects,
            commands::projects::create_project,
            commands::projects::delete_project,
            commands::documents::list_documents,
            commands::documents::get_document_text,
            commands::documents::import_documents,
            commands::documents::retry_document,
            commands::documents::delete_document,
            commands::documents::search_library,
            commands::documents::retrieval_status,
            commands::ai::ai_list_models,
            commands::ai::ai_add_model,
            commands::ai::ai_download_model,
            commands::ai::ai_cancel_download,
            commands::ai::ai_delete_model,
            commands::ai::ai_get_settings,
            commands::ai::ai_save_settings,
            commands::ai::ai_runtime_status,
            commands::ai::ai_load_model,
            commands::ai::ai_unload_model,
            commands::ai::ai_ask,
            commands::ai::ai_list_analyses,
            commands::ai::ai_get_analysis,
            commands::evidence::evidence_build,
            commands::evidence::evidence_save,
            commands::evidence::evidence_list,
            commands::evidence::evidence_get,
            commands::evidence::evidence_delete,
            commands::system::get_settings,
            commands::system::set_theme,
            commands::system::set_ai_enabled,
            commands::system::ai_pick_model_file,
            commands::system::get_system_info,
            commands::system::run_diagnostics,
            commands::system::probe_document_engine,
            commands::system::pick_folder,
            commands::system::pick_documents,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ResearchAI Workspace");
}
