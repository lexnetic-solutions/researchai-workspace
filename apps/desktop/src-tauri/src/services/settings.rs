//! Application settings persisted in the local SQLite settings store.
//! Local-first by construction — no network-capable fields exist (spec §34).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

impl Theme {
    pub fn as_str(&self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

/// Crosses the IPC boundary — the TS contract (packages/shared-types) uses
/// camelCase keys (dataDirectory, defaultImportMode, …).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub data_directory: String,
    pub default_import_mode: String, // "managed-copy" | "link-original"
    pub ocr_enabled: bool,
    pub max_concurrent_jobs: u32,
    pub ai_enabled: bool,
}

impl Settings {
    pub fn defaults(data_directory: &str) -> Self {
        Self {
            theme: Theme::System,
            data_directory: data_directory.to_string(),
            default_import_mode: "managed-copy".to_string(),
            // Conservative default for 8 GB machines (spec §43); diagnostics
            // may raise it on higher-RAM hardware in a later phase.
            ocr_enabled: true,
            max_concurrent_jobs: 1,
            ai_enabled: false,
        }
    }

    pub fn from_map(map: &HashMap<String, String>) -> Self {
        let mut s = Self::defaults("");
        if let Some(t) = map.get("theme") {
            s.theme = Theme::from_str(t);
        }
        if let Some(m) = map.get("default_import_mode") {
            s.default_import_mode = m.clone();
        }
        if let Some(o) = map.get("ocr_enabled") {
            s.ocr_enabled = o == "true";
        }
        if let Some(j) = map.get("max_concurrent_jobs") {
            s.max_concurrent_jobs = j.parse().unwrap_or(1);
        }
        if let Some(a) = map.get("ai_enabled") {
            s.ai_enabled = a == "true";
        }
        s
    }

    pub fn to_map(&self) -> Vec<(&'static str, String)> {
        vec![
            ("theme", self.theme.as_str().to_string()),
            (
                "default_import_mode",
                self.default_import_mode.clone(),
            ),
            ("ocr_enabled", self.ocr_enabled.to_string()),
            (
                "max_concurrent_jobs",
                self.max_concurrent_jobs.to_string(),
            ),
            ("ai_enabled", self.ai_enabled.to_string()),
        ]
    }
}
