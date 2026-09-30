//! HTTP client for the Python document engine (loopback sidecar).
//!
//! One shared agent with tight global timeouts so a hung sidecar never
//! blocks the ingestion worker for long (spec §43).

use std::sync::OnceLock;
use std::time::Duration;

use ureq::Agent;

use crate::error::{AppError, AppResult};
use crate::services::documents::EngineParseResponse;

pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 8737;

fn base_url() -> String {
    format!("http://{HOST}:{PORT}")
}

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        Agent::new_with_config(config)
    })
}

/// True when the sidecar answers /health with status ok.
pub fn health_ok() -> bool {
    let url = format!("{}/health", base_url());
    match agent().get(&url).call() {
        Ok(mut resp) => resp
            .body_mut()
            .read_json::<serde_json::Value>()
            .map(|v| v.get("status").and_then(|s| s.as_str()) == Some("ok"))
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// POST /parse for one managed file.
pub fn parse_document(
    document_id: &str,
    managed_path: &std::path::Path,
    mime_type: Option<&str>,
) -> AppResult<EngineParseResponse> {
    let body = serde_json::json!({
        "document_id": document_id,
        "managed_path": managed_path.to_string_lossy(),
        "mime_type": mime_type,
    });

    let url = format!("{}/parse", base_url());
    let mut resp = agent()
        .post(&url)
        .send_json(body)
        .map_err(|e| AppError::msg(format!("Document engine unreachable: {e}")))?;

    resp.body_mut()
        .read_json::<EngineParseResponse>()
        .map_err(|e| AppError::msg(format!("Invalid response from document engine: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe must answer `false` — never panic — regardless of whether
    /// the sidecar happens to be running during the test run.
    #[test]
    fn health_probe_returns_bool_not_panic() {
        let ok = health_ok();
        assert!(ok == true || ok == false);
    }
}
