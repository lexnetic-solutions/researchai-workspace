//! Document-engine sidecar lifecycle (Phase 9, spec §37/§46).
//!
//! In dev the engine is started externally (`pnpm engine:run`, screen
//! session); the app just probes 127.0.0.1:8737. When bundled, the sidecar
//! ships inside the `.app` under `Contents/Resources/sidecar/` and this
//! supervisor owns its lifetime: spawn at startup (unless one is already
//! answering), wait for `/health`, and kill the child on app exit.
//!
//! Layout contract: `<resource-dir>/sidecar/researchai-engine*` plus an
//! optional `engine.env` (`KEY=value` lines, e.g. `PORT=8737`) as an ops
//! escape hatch. Missing sidecar files are a normal dev condition — the
//! supervisor stays dormant and keeps probing, so an externally started
//! engine is picked up automatically.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::engine_client;

/// Where the bundled sidecar lives inside the app resources.
pub const SIDECAR_DIR_NAME: &str = "sidecar";

/// How the document engine is currently provided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineMode {
    /// Bundled sidecar spawned by this supervisor.
    Bundled,
    /// An engine is already answering on the port (dev / previous run).
    External,
    /// Nothing bundled and nothing answering yet; probing in background.
    Dormant,
}

/// Handles for the running supervisor. Dropping kills the owned child.
pub struct EngineRuntime {
    owns_child: Arc<AtomicBool>,
    kill: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    mode: Arc<Mutex<EngineMode>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for EngineRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineRuntime")
            .field("mode", &self.mode())
            .field("child_pid", &self.child_pid())
            .finish()
    }
}

impl EngineRuntime {
    /// Start the supervisor thread. Returns immediately; the thread probes
    /// and spawns asynchronously so startup never blocks on Python.
    pub fn start(resource_dir: PathBuf, models_dir: PathBuf) -> Self {
        let kill = Arc::new(AtomicBool::new(false));
        let owns_child = Arc::new(AtomicBool::new(false));
        let child_pid = Arc::new(Mutex::new(None));
        let mode = Arc::new(Mutex::new(EngineMode::Dormant));

        let (kill_t, owns_t, pid_t, mode_t) = (
            Arc::clone(&kill),
            Arc::clone(&owns_child),
            Arc::clone(&child_pid),
            Arc::clone(&mode),
        );
        let worker = std::thread::Builder::new()
            .name("engine-supervisor".into())
            .spawn(move || {
                supervisor_loop(resource_dir, models_dir, kill_t, owns_t, pid_t, mode_t);
            })
            .expect("spawn engine supervisor");

        Self {
            owns_child,
            kill,
            child_pid,
            mode,
            worker: Some(worker),
        }
    }

    /// Current provisioning mode (diagnostics/UI).
    pub fn mode(&self) -> EngineMode {
        self.mode.lock().expect("engine mode lock").clone()
    }

    /// PID of the spawned sidecar, when this runtime owns one.
    pub fn child_pid(&self) -> Option<u32> {
        *self.child_pid.lock().expect("engine pid lock")
    }
}

impl Drop for EngineRuntime {
    fn drop(&mut self) {
        self.kill.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        if self.owns_child.load(Ordering::SeqCst) {
            if let Some(pid) = *self.child_pid.lock().expect("engine pid lock") {
                kill_pid(pid);
            }
        }
    }
}

fn supervisor_loop(
    resource_dir: PathBuf,
    models_dir: PathBuf,
    kill: Arc<AtomicBool>,
    owns_child: Arc<AtomicBool>,
    child_pid: Arc<Mutex<Option<u32>>>,
    mode: Arc<Mutex<EngineMode>>,
) {
    // Already answering? (dev screen session, leftover from previous run)
    if engine_client::health_ok() {
        *mode.lock().expect("mode lock") = EngineMode::External;
        return;
    }

    let sidecar = match find_sidecar_binary(&resource_dir) {
        Some(p) => p,
        None => {
            // Nothing bundled: stay dormant, keep probing for an external
            // engine (dev flow) until shutdown.
            *mode.lock().expect("mode lock") = EngineMode::Dormant;
            while !kill.load(Ordering::SeqCst) {
                if engine_client::health_ok() {
                    *mode.lock().expect("mode lock") = EngineMode::External;
                    return;
                }
                std::thread::sleep(Duration::from_millis(2500));
            }
            return;
        }
    };

    let mut cmd = std::process::Command::new(&sidecar);
    cmd.current_dir(sidecar.parent().unwrap_or(Path::new(".")))
        .env("PORT", engine_client::PORT.to_string())
        .env("RESEARCHAI_MODELS_DIR", &models_dir);

    // engine.env beside the binary may override env (ops escape hatch).
    if let Some(env_file) = sidecar.parent().map(|p| p.join("engine.env")) {
        if let Ok(content) = std::fs::read_to_string(&env_file) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    cmd.env(k.trim(), v.trim());
                }
            }
        }
    }

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            *child_pid.lock().expect("pid lock") = Some(pid);
            owns_child.store(true, Ordering::SeqCst);
            *mode.lock().expect("mode lock") = EngineMode::Bundled;

            // Wait up to 30 s for /health; log-only on failure (the queue
            // retries parse jobs independently).
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let mut healthy = false;
            let mut exited: Option<std::process::ExitStatus> = None;
            while std::time::Instant::now() < deadline {
                if engine_client::health_ok() {
                    healthy = true;
                    break;
                }
                // Surface an instant death (missing `_internal/` runtime,
                // bad exec, …) instead of silently burning the timeout —
                // this is exactly how a broken sidecar shipped once.
                match child.try_wait() {
                    Ok(Some(status)) => {
                        exited = Some(status);
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
                if kill.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            if healthy {
                log::info!(target: "researchai::engine", "bundled sidecar healthy (pid {pid})");
            } else if let Some(status) = exited {
                log::error!(
                    target: "researchai::engine",
                    "bundled sidecar exited immediately ({status}) — frozen runtime missing? expected `_internal/` beside the sidecar binary"
                );
            } else if !kill.load(Ordering::SeqCst) {
                log::warn!(
                    target: "researchai::engine",
                    "bundled sidecar not healthy after 30s (pid {pid}) — engine still warming up or failed silently; check its output"
                );
            }

            // Hold the supervisor open so Drop signalling stays simple.
            while !kill.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1000));
            }

            // Kill and reap on the way out. Without wait() the killed child
            // lingers as a zombie of this process, so liveness probes like
            // kill -0 keep reporting it alive after shutdown.
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(e) => {
            log::error!(target: "researchai::engine", "could not spawn bundled sidecar: {e}");
            *mode.lock().expect("mode lock") = EngineMode::Dormant;
        }
    }
}

/// Locate the bundled sidecar binary. Checked layouts:
/// 1. `<resources>/sidecar/researchai-engine*` (remapped bundle resources)
/// 2. `<resources>/packaging/resources/sidecar/researchai-engine*`
///    (Tauri preserving the source tree prefix)
fn find_sidecar_binary(resource_dir: &Path) -> Option<PathBuf> {
    let mut dirs = vec![resource_dir.join(SIDECAR_DIR_NAME)];
    dirs.push(
        resource_dir
            .join("packaging")
            .join("resources")
            .join(SIDECAR_DIR_NAME),
    );
    for dir in dirs {
        if let Some(hit) = first_engine_binary(&dir) {
            return Some(hit);
        }
    }
    None
}

fn first_engine_binary(dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("researchai-engine"))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// Cross-process kill (we only keep the PID, not the Child handle).
fn kill_pid(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("kill").arg(pid.to_string()).output();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// Sidecar discovery is a pure path probe — test it on every platform (CI
// runs these on Windows too); only process-spawning behaviour needs unix.
#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::db::tests::TempDir;

    #[test]
    fn finds_sidecar_binary_in_resources() {
        let dir = TempDir::new_with_label("eng");
        let sidecar_dir = dir.path().join(SIDECAR_DIR_NAME);
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(sidecar_dir.join("engine.env"), b"# c\nFOO=1\n").unwrap();
        // Windows releases ship researchai-engine.exe; test that name too
        // where it can actually be produced.
        #[cfg(windows)]
        let binary = sidecar_dir.join("researchai-engine.exe");
        #[cfg(not(windows))]
        let binary = sidecar_dir.join("researchai-engine-macos");
        std::fs::write(&binary, b"bin").unwrap();

        let found = find_sidecar_binary(dir.path()).expect("binary found");
        assert!(found
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("researchai-engine"));
    }

    #[test]
    fn falls_back_to_packaging_prefix_layout() {
        let dir = TempDir::new_with_label("eng2");
        let nested = dir
            .path()
            .join("packaging")
            .join("resources")
            .join(SIDECAR_DIR_NAME);
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("researchai-engine"), b"bin").unwrap();
        let found = find_sidecar_binary(dir.path()).expect("binary found");
        assert!(found.to_string_lossy().contains("sidecar"));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn dormant_without_sidecar_files() {
        let dir = TempDir::new_with_label("eng");
        let rt = EngineRuntime::start(dir.path().to_path_buf(), dir.path().to_path_buf());
        std::thread::sleep(Duration::from_millis(300));
        // Port 8737 is normally unoccupied during tests; classification must
        // never report Bundled without a bundled sidecar.
        assert!(!matches!(rt.mode(), EngineMode::Bundled));
        assert!(rt.child_pid().is_none());
    }

    #[test]
    fn spawns_bundled_fake_sidecar_and_drop_kills_it() {
        // Hermeticity note: the supervisor deliberately prefers an engine
        // that already answers on the shared port (dev screen session), so
        // on a machine with a live engine this test asserts exactly that
        // classification; on CI (no engine) it exercises the spawn+kill path.
        let engine_already_up = engine_client::health_ok();

        let dir = TempDir::new_with_label("eng");
        let sidecar_dir = dir.path().join(SIDECAR_DIR_NAME);
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        let fake = sidecar_dir.join("researchai-engine-fake");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let rt = EngineRuntime::start(dir.path().to_path_buf(), dir.path().to_path_buf());
        std::thread::sleep(Duration::from_millis(400));

        if engine_already_up {
            assert_eq!(rt.mode(), EngineMode::External);
            assert!(rt.child_pid().is_none());
            drop(rt);
            return;
        }

        assert_eq!(rt.mode(), EngineMode::Bundled);
        let pid = rt.child_pid().expect("child pid recorded");
        drop(rt); // must not hang; must kill the child

        // The kill is signal-based; give the process a moment to die.
        std::thread::sleep(Duration::from_millis(300));
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        assert!(!alive, "child should be dead after drop");
    }
}
