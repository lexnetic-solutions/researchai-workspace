//! Self-update surface: check → install → restart.
//!
//! The frontend never talks to `tauri-plugin-updater` directly; these
//! commands keep the pending [`Update`] server-side behind a small, mockable
//! command set (the browser preview has no updater). Endpoints and the
//! signature public key are baked into `tauri.conf.json`.
//!
//! Debug builds honour `RESEARCHAI_UPDATER_ENDPOINT` so a local manifest can
//! be served for end-to-end checks. The override only changes where the
//! manifest is fetched from: every download is still verified against the
//! embedded public key before install, so it cannot introduce unsigned code.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// An update announced by [`updater_check`] and consumed by [`updater_install`].
#[derive(Default)]
pub struct PendingUpdate(pub Mutex<Option<Update>>);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Updater(#[from] tauri_plugin_updater::Error),
    #[error("no update is pending install")]
    NoPendingUpdate,
    #[error("invalid updater endpoint override: {0}")]
    Endpoint(String),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub content_length: Option<u64>,
}

/// Progress/finish events streamed to the frontend while downloading.
pub const PROGRESS_EVENT: &str = "updater://progress";

/// Emit at most this often so a 600 MB download does not spam the log.
const PROGRESS_STEP: u64 = 64 * 1024 * 1024;

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater> {
    let mut builder = app.updater_builder();
    if cfg!(debug_assertions) {
        if let Ok(endpoint) = std::env::var("RESEARCHAI_UPDATER_ENDPOINT") {
            let url = url::Url::parse(&endpoint)
                .map_err(|e| Error::Endpoint(format!("{endpoint}: {e}")))?;
            log::info!(
                target: "researchai",
                "update check: debug endpoint override active ({endpoint})"
            );
            builder = builder.endpoints(vec![url])?;
        }
    }
    Ok(builder.build()?)
}

/// Check the configured endpoint for a newer release. Stores the pending
/// update on success so [`updater_install`] can apply exactly what was
/// announced here. Returns `None` when the running version is current.
#[tauri::command]
pub async fn updater_check(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<Option<UpdateMetadata>> {
    let update = build_updater(&app)?.check().await?;
    let metadata = update.as_ref().map(|u| UpdateMetadata {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    match &metadata {
        Some(m) => log::info!(
            target: "researchai",
            "update check: {} available (current {}, target {})",
            m.version,
            m.current_version,
            update.as_ref().map(|u| u.target.as_str()).unwrap_or("?"),
        ),
        None => log::info!(
            target: "researchai",
            "update check: no update (current {})",
            app.package_info().version
        ),
    }
    *pending.0.lock().unwrap() = update;
    Ok(metadata)
}

/// Download the pending update, verify its signature against the embedded
/// public key, and install it. Emits [`PROGRESS_EVENT`] every
/// [`PROGRESS_STEP`] bytes. On success the app still runs the old binary —
/// the frontend calls [`updater_restart`] to relaunch into the new one
/// (Windows exits by itself during install).
#[tauri::command]
pub async fn updater_install(app: AppHandle, pending: State<'_, PendingUpdate>) -> Result<()> {
    let update = pending
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or(Error::NoPendingUpdate)?;
    log::info!(
        target: "researchai",
        "update download: starting {} (from {})",
        update.version,
        update.download_url
    );
    // Shared atomics: the chunk callback (FnMut) and the finish callback
    // (FnOnce) are passed to download_and_install together, so they may not
    // borrow the same counter with different mutability.
    let downloaded = AtomicU64::new(0);
    let next_emit = AtomicU64::new(0);
    let progress_app = app.clone();
    update
        .download_and_install(
            |chunk_length, content_length| {
                let total = downloaded.fetch_add(chunk_length as u64, Ordering::Relaxed)
                    + chunk_length as u64;
                if total >= next_emit.load(Ordering::Relaxed) {
                    next_emit.store(total + PROGRESS_STEP, Ordering::Relaxed);
                    let _ = progress_app.emit(
                        PROGRESS_EVENT,
                        DownloadProgress {
                            downloaded: total,
                            content_length,
                        },
                    );
                }
            },
            || {
                log::info!(
                    target: "researchai",
                    "update download: complete ({} bytes); verifying signature and installing",
                    downloaded.load(Ordering::Relaxed)
                );
            },
        )
        .await?;
    log::info!(
        target: "researchai",
        "update install: {} applied — restart to run it",
        update.version
    );
    Ok(())
}

/// Relaunch the app to run the freshly installed version. Diverges: the
/// process is replaced.
#[tauri::command]
pub fn updater_restart(app: AppHandle) {
    log::info!(target: "researchai", "update: restarting to apply the new version");
    app.restart();
}
