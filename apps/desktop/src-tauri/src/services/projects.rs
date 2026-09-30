//! Project records crossing the IPC boundary.

use serde::{Deserialize, Serialize};

/// Crosses the IPC boundary — the TS contract (packages/shared-types) uses
/// camelCase keys (createdAt, updatedAt).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
