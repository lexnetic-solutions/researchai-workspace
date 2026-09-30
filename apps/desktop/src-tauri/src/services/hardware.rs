//! Hardware detection for diagnostics and future model sizing (spec §36).

use serde::Serialize;
use sysinfo::System;

use crate::error::AppResult;

/// Crosses the IPC boundary — the TS contract (packages/shared-types) nests
/// the CPU as `cpu: { name, cores }`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub arch: String,
    pub cpu: CpuInfo,
    /// Total physical RAM in megabytes.
    pub total_memory_mb: u64,
    /// Available physical RAM in megabytes.
    pub available_memory_mb: u64,
    /// Derived hardware profile: light (<12 GB), standard (<28 GB), advanced.
    pub profile: String,
}

pub fn detect() -> AppResult<SystemInfo> {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();

    let total = sys.total_memory() / (1024 * 1024);
    let available = sys.available_memory() / (1024 * 1024);

    let cpu_name = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());
    let cpu = CpuInfo {
        name: cpu_name,
        cores: sys.cpus().len(),
    };

    let profile = if total < 12 * 1024 {
        "light"
    } else if total < 28 * 1024 {
        "standard"
    } else {
        "advanced"
    };

    Ok(SystemInfo {
        os_name: System::name().unwrap_or_else(|| "Unknown OS".to_string()),
        os_version: System::os_version().unwrap_or_default(),
        arch: std::env::consts::ARCH.to_string(),
        cpu,
        total_memory_mb: total,
        available_memory_mb: available,
        profile: profile.to_string(),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    pub name: String,
    pub cores: usize,
}
