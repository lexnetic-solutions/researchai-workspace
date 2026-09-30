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
    log::info!(target: "researchai::projects", "deleted project {id}");
    Ok(())
}
