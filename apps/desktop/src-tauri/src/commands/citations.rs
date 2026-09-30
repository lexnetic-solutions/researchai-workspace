//! Citation commands (Phase 5, spec §31): user-correctable metadata and the
//! formatted bibliography.

use tauri::State;

use crate::error::AppResult;
use crate::services::citations::{self, CitationStyle, FormattedReference};
use crate::services::documents::BibliographyUpdate;
use crate::state::AppState;

/// Apply user corrections to a document's bibliographic metadata.
/// Corrections are authoritative over extracted values (spec §31); the
/// returned row reflects the new state.
#[tauri::command]
pub fn update_document_bibliography(
    state: State<'_, AppState>,
    document_id: String,
    update: BibliographyUpdate,
) -> AppResult<()> {
    state
        .db
        .update_document_bibliography(&document_id, &update)
}

/// Formatted reference list for a project (alphabetised). `style` is one of
/// `apa` | `harvard` | `chicago`.
#[tauri::command]
pub fn bibliography_list(
    state: State<'_, AppState>,
    project_id: String,
    style: String,
) -> AppResult<Vec<FormattedReference>> {
    let style = CitationStyle::from_str(&style)?;
    citations::bibliography(&state.db, &project_id, style)
}
