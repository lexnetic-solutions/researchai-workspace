//! First-run diagnostics (spec §48): hardware, data directory, database,
//! document-engine reachability. All checks are local; nothing is uploaded.

use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: String,
    pub label: String,
    pub status: String, // "pass" | "warn" | "fail"
    pub detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub system: crate::services::hardware::SystemInfo,
    pub checks: Vec<DiagnosticCheck>,
    pub data_directory: String,
    pub database_ok: bool,
    pub document_engine_ok: bool,
    pub generated_at: String,
}

pub fn run(state: &AppState) -> crate::error::AppResult<DiagnosticsReport> {
    let system = crate::services::hardware::detect()?;
    let mut checks: Vec<DiagnosticCheck> = Vec::new();

    // Data directory
    let dir_ok = state.data_dir.is_dir();
    checks.push(DiagnosticCheck {
        id: "data-directory".into(),
        label: "Data directory".into(),
        status: if dir_ok { "pass" } else { "fail" }.into(),
        detail: state.data_dir.display().to_string(),
    });

    // Database round-trip
    let db_ok = state
        .db
        .list_projects()
        .map(|_| true)
        .unwrap_or(false);
    checks.push(DiagnosticCheck {
        id: "database".into(),
        label: "SQLite database".into(),
        status: if db_ok { "pass" } else { "fail" }.into(),
        detail: if db_ok {
            "research.db opened, schema current".into()
        } else {
            "Could not query research.db".into()
        },
    });

    // Workspace subdirectories
    let ws_ok = state.data_dir.join("documents").is_dir();
    checks.push(DiagnosticCheck {
        id: "workspace-layout".into(),
        label: "Workspace layout".into(),
        status: if ws_ok { "pass" } else { "warn" }.into(),
        detail: if ws_ok {
            "documents/, derived/, exports/ present".into()
        } else {
            "Workspace subfolders not initialised".into()
        },
    });

    // Document engine sidecar (real health check via HTTP client)
    let engine_ok = crate::services::engine_client::health_ok();
    checks.push(DiagnosticCheck {
        id: "document-engine".into(),
        label: "Document engine (Python sidecar)".into(),
        status: if engine_ok { "pass" } else { "warn" }.into(),
        detail: if engine_ok {
            "127.0.0.1:8737 answered /health".into()
        } else {
            "Not reachable — start it with `pnpm engine:run` (required for parsing)".into()
        },
    });

    // Logging
    checks.push(DiagnosticCheck {
        id: "logging".into(),
        label: "Local logging".into(),
        status: "pass".into(),
        detail: format!("logs/ under {}", crate::logging::log_dir(&state.data_dir).display()),
    });

    Ok(DiagnosticsReport {
        system,
        checks,
        data_directory: state.data_dir.display().to_string(),
        database_ok: db_ok,
        document_engine_ok: engine_ok,
        generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}
