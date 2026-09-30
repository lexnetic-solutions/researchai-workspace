//! Export commands (Phase 6, spec §32).

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::services::citations::CitationStyle;
use crate::services::exports::{
    capabilities, ExportFormat, ExportKind, ExportResult, ExportService,
};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCapabilitiesResponse {
    pub formats: Vec<String>,
    pub kinds: Vec<ExportKindDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportKindDto {
    pub kind: String,
    pub formats: Vec<String>,
}

#[tauri::command]
pub fn export_capabilities() -> ExportCapabilitiesResponse {
    ExportCapabilitiesResponse {
        formats: ["markdown", "docx", "pdf", "bibtex", "ris"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        kinds: capabilities()
            .into_iter()
            .map(|c| ExportKindDto {
                kind: c.kind,
                formats: c.formats,
            })
            .collect(),
    }
}

/// Export an analysis / evidence table / bibliography document.
/// `source_id` is the analysis or table id; bibliography uses project scope.
#[tauri::command]
pub async fn export_document(
    app: AppHandle,
    project_id: String,
    kind: String,
    source_id: Option<String>,
    format: String,
    style: Option<String>,
) -> AppResult<ExportResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let kind = ExportKind::from_str(&kind)?;
        let format = ExportFormat::from_str(&format)?;
        let style = CitationStyle::from_str(style.as_deref().unwrap_or("apa"))?;

        // DOCX/PDF need the sidecar; fail with an actionable message.
        if format.needs_engine() && !crate::services::engine_client::health_ok() {
            return Err(AppError::msg(
                "The document engine is offline — start it with `pnpm engine:run` to render DOCX/PDF exports.",
            ));
        }

        let svc = ExportService::new(&state.data_dir);
        svc.export_doc(
            &state.db,
            &project_id,
            kind,
            source_id.as_deref(),
            format,
            style,
        )
    })
    .await
    .map_err(|e| AppError::msg(format!("Export failed to run: {e}")))?
}

/// Export the whole project bibliography as BibTeX or RIS.
#[tauri::command]
pub async fn export_bibliography(
    app: AppHandle,
    project_id: String,
    format: String,
) -> AppResult<ExportResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let format = ExportFormat::from_str(&format)?;
        if !matches!(format, ExportFormat::BibTeX | ExportFormat::Ris) {
            return Err(AppError::msg(
                "Use export_document for formatted bibliography documents.",
            ));
        }
        let svc = ExportService::new(&state.data_dir);
        svc.export_bibliography_database(&state.db, &project_id, format)
    })
    .await
    .map_err(|e| AppError::msg(format!("Export failed to run: {e}")))?
}

/// Validate an exported file still exists (UI reveals it via the opener
/// plugin, which already has capability).
#[tauri::command]
pub fn reveal_path(path: String) -> AppResult<String> {
    let p = std::path::Path::new(&path);
    if !p.is_file() {
        return Err(AppError::msg("The export file no longer exists."));
    }
    Ok(path)
}

/// List recent exports for the UI (exports/ directory, newest first).
#[tauri::command]
pub fn list_exports(state: State<'_, AppState>) -> AppResult<Vec<ExportFileDto>> {
    let svc = ExportService::new(&state.data_dir);
    let dir = svc.exports_dir();
    let mut files: Vec<ExportFileDto> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let meta = entry.metadata().ok();
                files.push(ExportFileDto {
                    name: entry.file_name().to_string_lossy().to_string(),
                    path: path.to_string_lossy().to_string(),
                    size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                });
            }
        }
    }
    files.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(files)
}

/// Aggregate size of the exports directory (storage sweep, Phase 6 note).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportStatsDto {
    pub files: u64,
    pub total_bytes: u64,
}

#[tauri::command]
pub fn exports_stats(state: State<'_, AppState>) -> AppResult<ExportStatsDto> {
    let svc = ExportService::new(&state.data_dir);
    let dir = svc.exports_dir().to_path_buf();
    let mut stats = ExportStatsDto {
        files: 0,
        total_bytes: 0,
    };
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                stats.files += 1;
                stats.total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    Ok(stats)
}

/// Delete one export file. Refuses anything outside the managed exports
/// directory (spec §13: managed storage is the only writable area).
#[tauri::command]
pub fn delete_export(state: State<'_, AppState>, path: String) -> AppResult<ExportStatsDto> {
    let svc = ExportService::new(&state.data_dir);
    let dir = svc.exports_dir().to_path_buf();

    let target = std::path::Path::new(&path);
    if !target.is_file() {
        return Err(AppError::msg("The export file no longer exists."));
    }
    // Canonicalise both sides so `../` escapes cannot pass the prefix check.
    let dir_canon = dir.canonicalize()?;
    let target_canon = target.canonicalize()?;
    if !target_canon.starts_with(&dir_canon) {
        return Err(AppError::msg(
            "Refusing to delete: the file is not inside the managed exports directory.",
        ));
    }

    std::fs::remove_file(target_canon)?;
    exports_stats(state)
}

#[cfg(all(test, unix))]
mod tests {
    use crate::db::tests::TempDir;
    use std::path::Path;

    /// The delete command's core is the path-containment check; exercise it
    /// against a real temp dir with an outside file and an inside file.
    #[test]
    fn delete_refuses_paths_outside_exports_dir() {
        let dir = TempDir::new_with_label("exp-del");
        let exports = dir.path().join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let inside = exports.join("report.md");
        std::fs::write(&inside, b"x").unwrap();
        let outside = dir.path().join("precious.db");
        std::fs::write(&outside, b"keep me").unwrap();

        let dir_canon = exports.canonicalize().unwrap();
        // The command's guard, extracted: outside → refused, inside → allowed.
        let outside_allowed = outside
            .canonicalize()
            .map(|p| p.starts_with(&dir_canon))
            .unwrap_or(false);
        assert!(!outside_allowed);

        // Same logic for a traversal-looking path that resolves outside.
        let sneaky = exports.join("../precious.db");
        let sneaky_allowed = sneaky
            .canonicalize()
            .map(|p| p.starts_with(&dir_canon))
            .unwrap_or(false);
        assert!(!sneaky_allowed);

        let inside_allowed = inside
            .canonicalize()
            .map(|p| p.starts_with(&dir_canon))
            .unwrap_or(true);
        assert!(inside_allowed);
        assert!(Path::new(&inside).is_file());
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFileDto {
    pub name: String,
    pub path: String,
    #[serde(rename = "sizeBytes")]
    pub size_bytes: u64,
}
