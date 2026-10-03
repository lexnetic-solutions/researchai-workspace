//! Project CRUD commands.

use tauri::State;

use crate::error::AppResult;
use crate::services::projects::ProjectRow;
use crate::state::AppState;

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> AppResult<Vec<ProjectRow>> {
    state.db.list_projects()
}

#[tauri::command]
pub fn create_project(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
) -> AppResult<ProjectRow> {
    let row = state.db.create_project(&name, description.as_deref())?;
    log::info!(target: "researchai::projects", "created project {} ({})", row.name, row.id);
    Ok(row)
}

#[tauri::command]
pub fn delete_project(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.db.delete_project(&id)?;
    // The DB cascade removes the document rows; the managed copies on disk
    // would otherwise be orphaned forever (spec §13 workspace is ours to
    // manage). Link-original files live outside the workspace and stay put.
    let managed = state.data_dir.join("documents").join(&id);
    if managed.is_dir() {
        if let Err(e) = std::fs::remove_dir_all(&managed) {
            log::warn!(
                target: "researchai::projects",
                "could not purge managed copies for {id}: {e}"
            );
        }
    }
    log::info!(target: "researchai::projects", "deleted project {id}");
    Ok(())
}
