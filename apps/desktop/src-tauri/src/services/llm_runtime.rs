//! LLM runtime supervisor (spec §17, §43).
//!
//! Owns the `llama-server` child process: spawn with model + context
//! settings, health-poll until the weights are loaded, auto-unload after the
//! configured idle timeout, and shut down cleanly when the app exits. The
//! supervisor thread is the only owner of the [`std::process::Child`];
//! everyone else talks to shared state, so process handles never leak across
//! threads.
//!
//! The core never links llama.cpp — the runtime is a separate, crash-isolated
//! process. A crash of `llama-server` surfaces as a `Failed` state, never as
//! a crash of the app. At startup any leftover `llama-server` from a previous
//! run that references the managed models directory is terminated (spec §43:
//! the LLM unloads when idle; a fresh run must not inherit a stale server).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::db::AiSettings;
use crate::error::AppResult;

/// State machine observed by the UI and the analysis pipeline. Serialised
/// internally tagged: {"state":"ready","port":5001} / {"state":"failed",
/// "detail":"…"} so the frontend can discriminate on `state`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum LoadState {
    /// No model requested yet.
    Idle,
    /// Spawned, weights not loaded yet.
    Loading,
    /// Serving on the given loopback port.
    Ready { port: u16 },
    /// Process exited or never started; human-readable reason.
    Failed { detail: String },
    /// Explicitly unloaded (manual or idle timeout).
    Unloaded,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub state: LoadState,
    pub model_file: Option<String>,
    pub pid: Option<u32>,
    /// Seconds since the last completion request (None = no request yet).
    pub idle_seconds: Option<u64>,
}

/// Everything the worker needs to spawn the server.
#[derive(Debug, Clone)]
struct LoadCommand {
    binary: PathBuf,
    model_path: PathBuf,
    model_file: String,
    settings: AiSettings,
    log_path: PathBuf,
    models_dir: PathBuf,
    /// Flipped by the worker on Unload/Shutdown so a waiting
    /// `ensure_loaded` returns promptly instead of blocking on health.
    shutdown: Arc<AtomicBool>,
}

enum Command {
    Load(LoadCommand),
    Unload,
    Shutdown,
}

pub struct LlmRuntime {
    tx: Sender<Command>,
    state: Arc<Mutex<RuntimeState>>,
    last_activity: Arc<Mutex<Option<Instant>>>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Set on drop so `Drop` never blocks behind a long health wait: the
    /// worker checks it in its 500 ms select loop.
    worker_done: Arc<AtomicBool>,
    /// Cancel flag of the load attempt currently in flight, shared so a
    /// dropping runtime can abort a spawn/health wait from outside.
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    data_dir: PathBuf,
}

struct RuntimeState {
    state: LoadState,
    model_file: Option<String>,
    pid: Option<u32>,
}

impl LlmRuntime {
    /// Start the supervisor thread. Kills leftover servers from previous
    /// runs first (see module docs).
    pub fn start(data_dir: &std::path::Path, models_dir: &std::path::Path) -> Self {
        kill_stale_llama_servers(models_dir);

        let (tx, rx) = mpsc::channel::<Command>();
        let state = Arc::new(Mutex::new(RuntimeState {
            state: LoadState::Idle,
            model_file: None,
            pid: None,
        }));
        let last_activity: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

        let worker_state = Arc::clone(&state);
        let worker_activity = Arc::clone(&last_activity);
        let worker_done = Arc::new(AtomicBool::new(false));
        let worker_done_inner = Arc::clone(&worker_done);
        let active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::new(Mutex::new(None));
        let worker_cancel = Arc::clone(&active_cancel);
        let worker = std::thread::Builder::new()
            .name("llm-runtime".into())
            .spawn(move || {
                worker_loop(rx, worker_state, worker_activity, worker_done_inner, worker_cancel);
            })
            .expect("spawn llm runtime worker");

        Self {
            tx,
            state,
            last_activity,
            worker: Some(worker),
            worker_done,
            active_cancel,
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Ask for the model to be loaded; blocks until Ready or Failed.
    /// `deadline` bounds how long weight-loading may take.
    pub fn ensure_loaded(
        &self,
        binary: &std::path::Path,
        model_path: &std::path::Path,
        settings: &AiSettings,
        deadline: Duration,
    ) -> AppResult<LoadState> {
        let model_file = model_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "model.gguf".into());

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let cmd = LoadCommand {
            binary: binary.to_path_buf(),
            model_path: model_path.to_path_buf(),
            model_file: model_file.clone(),
            settings: settings.clone(),
            log_path: crate::logging::log_dir(&self.data_dir).join("llama-server.log"),
            models_dir: model_path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
            shutdown: Arc::clone(&cancel_flag),
        };
        self.tx
            .send(Command::Load(cmd))
            .map_err(|_| crate::error::AppError::msg("AI runtime stopped."))?;

        let started = Instant::now();
        loop {
            if started.elapsed() > deadline {
                return Ok(LoadState::Failed {
                    detail: "Model loading timed out.".into(),
                });
            }
            if cancel_flag.load(Ordering::SeqCst) {
                return Ok(LoadState::Failed {
                    detail: "Model loading was cancelled.".into(),
                });
            }
            {
                let guard = self.state.lock().expect("llm state lock");
                match &guard.state {
                    LoadState::Ready { .. } => {
                        self.touch();
                        return Ok(guard.state.clone());
                    }
                    LoadState::Failed { detail } => {
                        return Ok(LoadState::Failed {
                            detail: detail.clone(),
                        })
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Terminate the current server, if any.
    pub fn unload(&self) {
        let _ = self.tx.send(Command::Unload);
        // Give the worker a moment so callers observe Unloaded promptly.
        for _ in 0..20 {
            if matches!(self.snapshot().state, LoadState::Unloaded | LoadState::Idle) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Record activity now (prevents idle-unload mid-conversation).
    pub fn touch(&self) {
        *self.last_activity.lock().expect("activity lock") = Some(Instant::now());
    }

    pub fn snapshot(&self) -> RuntimeStatus {
        let guard = self.state.lock().expect("llm state lock");
        let idle = self
            .last_activity
            .lock()
            .expect("activity lock")
            .map(|t| t.elapsed().as_secs());
        RuntimeStatus {
            state: guard.state.clone(),
            model_file: guard.model_file.clone(),
            pid: guard.pid,
            idle_seconds: idle,
        }
    }
}

impl Drop for LlmRuntime {
    fn drop(&mut self) {
        // Abort any in-flight spawn/health wait first, then request shutdown.
        if let Some(flag) = self.active_cancel.lock().expect("cancel lock").take() {
            flag.store(true, Ordering::SeqCst);
        }
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.worker.take() {
            // Wait briefly; the worker checks worker_done in its select loop
            // and the cancel flag above unblocks any health wait. If a slow
            // kill still holds it, detach rather than block app exit — the
            // worker's Shutdown path still terminates the child process.
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.worker_done.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if self.worker_done.load(Ordering::SeqCst) {
                let _ = h.join();
            } else {
                std::mem::forget(h);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Worker — sole owner of the child process
// ---------------------------------------------------------------------------

fn worker_loop(
    rx: mpsc::Receiver<Command>,
    state: Arc<Mutex<RuntimeState>>,
    last_activity: Arc<Mutex<Option<Instant>>>,
    worker_done: Arc<AtomicBool>,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
) {
    let mut current: Option<ServerProc> = None;
    // Flag of the load attempt currently in flight (spawn/health phase).
    let mut active_flag: Option<Arc<AtomicBool>> = None;

    loop {
        // Cooperative shutdown + idle-unload polling with a short select-less
        // timeout (std mpsc has no select; recv_timeout covers both needs).
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Command::Load(cmd)) => {
                // Unload whatever is running, then spawn the new server.
                if let Some(proc) = current.take() {
                    proc.kill_and_wait();
                    set_state(&state, LoadState::Unloaded, None, None);
                }
                set_state(
                    &state,
                    LoadState::Loading,
                    Some(cmd.model_file.clone()),
                    None,
                );
                active_flag = Some(Arc::clone(&cmd.shutdown));
                *active_cancel.lock().expect("cancel lock") = active_flag.clone();
                match spawn_and_wait_ready(&cmd, &state) {
                    Ok(proc) => {
                        active_flag = None;
                        *active_cancel.lock().expect("cancel lock") = None;
                        set_state(
                            &state,
                            LoadState::Ready { port: proc.port },
                            Some(cmd.model_file.clone()),
                            Some(proc.pid),
                        );
                        current = Some(proc);
                        *last_activity.lock().expect("activity lock") = Some(Instant::now());
                    }
                    Err(detail) => {
                        active_flag = None;
                        *active_cancel.lock().expect("cancel lock") = None;
                        if cmd.shutdown.load(Ordering::SeqCst) {
                            // Cancelled mid-spawn (Unload/Shutdown won).
                            set_state(
                                &state,
                                LoadState::Unloaded,
                                Some(cmd.model_file.clone()),
                                None,
                            );
                        } else {
                            set_state(
                                &state,
                                LoadState::Failed { detail },
                                Some(cmd.model_file.clone()),
                                None,
                            );
                        }
                    }
                }
            }
            Ok(Command::Unload) => {
                if let Some(flag) = active_flag.take() {
                    flag.store(true, Ordering::SeqCst);
                }
                if let Some(proc) = current.take() {
                    proc.kill_and_wait();
                }
                set_state(&state, LoadState::Unloaded, None, None);
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(flag) = active_flag.take() {
                    flag.store(true, Ordering::SeqCst);
                }
                if let Some(proc) = current.take() {
                    proc.kill_and_wait();
                }
                worker_done.store(true, Ordering::SeqCst);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Idle auto-unload (spec §43).
                let idle_minutes = current
                    .as_ref()
                    .map(|p| p.idle_unload_minutes)
                    .unwrap_or(0);
                if idle_minutes > 0 {
                    if let Some(t) = *last_activity.lock().expect("activity lock") {
                        if t.elapsed() > Duration::from_secs(u64::from(idle_minutes) * 60) {
                            if let Some(proc) = current.take() {
                                log::info!(target: "researchai::llm", "idle timeout — unloading model");
                                proc.kill_and_wait();
                                set_state(&state, LoadState::Unloaded, None, None);
                            }
                        }
                    }
                }
                // Detect silent crashes: if the child died without us asking.
                if let Some(proc) = current.as_mut() {
                    if let Ok(Some(status)) = proc.child.try_wait() {
                        log::warn!(target: "researchai::llm", "llama-server exited unexpectedly: {status}");
                        set_state(
                            &state,
                            LoadState::Failed {
                                detail: format!("The local model server exited ({status})."),
                            },
                            None,
                            None,
                        );
                        current = None;
                    }
                }
            }
        }
    }
}

struct ServerProc {
    child: std::process::Child,
    port: u16,
    pid: u32,
    pid_file: PathBuf,
    idle_unload_minutes: u32,
}

impl ServerProc {
    fn kill_and_wait(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

/// Liveness that treats zombies (killed but not yet reaped by the parent) as
/// dead — plain `kill -0` reports zombies as alive forever on macOS.
#[cfg(test)]
fn process_gone(pid: u32) -> bool {
    match std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
    {
        Ok(out) if out.status.success() => {
            let stat = String::from_utf8_lossy(&out.stdout);
            !stat.chars().any(|c| c == 'Z')
        }
        _ => true,
    }
}

fn set_state(
    state: &Arc<Mutex<RuntimeState>>,
    state_v: LoadState,
    model_file: Option<String>,
    pid: Option<u32>,
) {
    let mut guard = state.lock().expect("llm state lock");
    if model_file.is_some() || !matches!(state_v, LoadState::Idle) {
        // Keep the last model name visible in Failed/Unloaded states for UX.
        if model_file.is_some() {
            guard.model_file = model_file;
        }
    }
    if pid.is_some() || matches!(state_v, LoadState::Unloaded | LoadState::Idle | LoadState::Failed { .. }) {
        guard.pid = pid;
    }
    guard.state = state_v;
}

/// Spawn llama-server, stream its output to the log file and poll /health
/// until the model is ready (or the process exits / health times out).
fn spawn_and_wait_ready(
    cmd: &LoadCommand,
    state: &Arc<Mutex<RuntimeState>>,
) -> Result<ServerProc, String> {
    if !cmd.binary.is_file() {
        return Err(format!(
            "llama-server not found at \"{}\" — set the path in Settings → Local AI.",
            cmd.binary.display()
        ));
    }
    if !cmd.model_path.is_file() {
        return Err(format!(
            "The model file is missing on disk: \"{}\". Re-scan or re-import the model.",
            cmd.model_path.display()
        ));
    }

    let port = find_free_port().ok_or_else(|| "No free TCP port available.".to_string())?;
    let s = &cmd.settings;

    let mut args: Vec<String> = vec![
        "-m".into(),
        cmd.model_path.to_string_lossy().to_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--ctx-size".into(),
        s.context_size.to_string(),
        "-fa".into(),
        "off".into(),
        // Prompt-prefix KV cache: keep context chunks of repeated prefixes
        // so asks that share evidence re-process only the tail (pairs with
        // the evidence-first prompt order in analysis.rs).
        "--cache-reuse".into(),
        "256".into(),
    ];
    if s.threads > 0 {
        args.extend(["-t".into(), s.threads.to_string()]);
    }
    if s.gpu_layers > 0 {
        args.extend(["-ngl".into(), s.gpu_layers.to_string()]);
    }
    for extra in s.llama_server_args.split_whitespace() {
        args.push(extra.to_string());
    }

    log::info!(target: "researchai::llm", "spawning llama-server on port {port}: {} {}", cmd.binary.display(), args.join(" "));

    if let Some(parent) = std::path::Path::new(&cmd.log_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cmd.log_path)
        .map_err(|e| format!("Could not open llama-server log: {e}"))?;
    let log_err = log_file
        .try_clone()
        .map_err(|e| format!("Could not open llama-server log: {e}"))?;

    let mut child = std::process::Command::new(&cmd.binary)
        .args(&args)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_err))
        .spawn()
        .map_err(|e| format!("Could not start llama-server: {e}"))?;

    let pid = child.id();
    set_state_pub(state, pid);

    // Persist the pid so the next app run can sweep this server if the app
    // crashed without cleanup (see kill_stale_llama_servers).
    let pid_file = cmd.models_dir.join("llama-server.pid");
    let _ = std::fs::write(&pid_file, pid.to_string());

    // Poll health; fail fast if the process dies (port conflict, bad flag…).
    let agent = {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build();
        ureq::Agent::new_with_config(config)
    };
    let health_url = format!("http://127.0.0.1:{port}/health");
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        // An Unload/Shutdown during the health wait must end this promptly.
        if cmd.shutdown.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Model loading was cancelled.".into());
        }
        if let Ok(Some(status)) = child.try_wait() {
            let tail = std::fs::read_to_string(&cmd.log_path)
                .unwrap_or_default()
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(format!(
                "llama-server exited during startup ({status}). Log tail: {tail}"
            ));
        }
        if Instant::now() > deadline {
            return Err("llama-server did not become healthy in time.".into());
        }
        if let Ok(resp) = agent.get(&health_url).call() {
            if resp.status().is_success() {
                return Ok(ServerProc {
                    child,
                    port,
                    pid,
                    pid_file,
                    idle_unload_minutes: cmd.settings.idle_unload_minutes,
                });
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn set_state_pub(state: &Arc<Mutex<RuntimeState>>, pid: u32) {
    state.lock().expect("llm state lock").pid = Some(pid);
}

/// Bind :0 on loopback to let the OS pick a free port, then release it.
pub fn find_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok().map(|a| a.port())
}

/// Terminate `llama-server` processes left over from a previous app run.
///
/// Primary signal: the pid file the supervisor writes at spawn time
/// (`<models_dir>/llama-server.pid`) — deterministic and cross-platform.
/// Secondary signal: any process whose command line references both
/// "llama-server" and the managed models directory (covers crashed runs
/// that died before writing the pid file).
pub fn kill_stale_llama_servers(models_dir: &std::path::Path) {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    // 1. Pid file left behind by the supervisor.
    let pid_file = models_dir.join("llama-server.pid");
    if let Ok(text) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            let sys_pid = sysinfo::Pid::from_u32(pid);
            if let Some(process) = sys.process(sys_pid) {
                log::info!(
                    target: "researchai::llm",
                    "terminating stale llama-server from pid file (pid {pid})"
                );
                process.kill();
            }
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    // 2. Command-line sweep for servers without a pid file.
    let models_dir_str = models_dir.to_string_lossy();
    for (_pid, process) in sys.processes() {
        let cmd_joined = process
            .cmd()
            .iter()
            .map(|c| c.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if cmd_joined.contains("llama-server") && cmd_joined.contains(models_dir_str.as_ref()) {
            log::info!(target: "researchai::llm", "terminating stale llama-server (pid {})", process.pid());
            process.kill();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — end-to-end supervisor behaviour against a scripted fake server.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::{temp_dir_for, TempDir};
    use std::io::Write as IoWrite;

    /// Fake llama-server: a real loopback HTTP server answering 200 on
    /// /health, parsing the same CLI args the supervisor passes (--port, -m)
    /// and reporting its pid via $FAKE_PID_FILE when that env var is set.
    const FAKE_SERVER_PY: &str = r#"#!/usr/bin/env python3
import http.server, os, socketserver, sys

model_path = sys.argv[sys.argv.index("-m") + 1]
port = int(sys.argv[sys.argv.index("--port") + 1])
assert os.path.isfile(model_path), f"model missing: {model_path}"
pid_file = os.environ.get("FAKE_PID_FILE", "")
if pid_file:
    with open(pid_file, "w") as f:
        f.write(str(os.getpid()))

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'{"status":"ok"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.do_GET()

    def log_message(self, *args):
        pass

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
    httpd.serve_forever()
"#;

    fn setup_fake_server() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir_for("llm-runtime");
        let script = dir.path().join("llama-server-fake.py");
        let mut f = std::fs::File::create(&script).unwrap();
        f.write_all(FAKE_SERVER_PY.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let model = dir.path().join("models").join("fake-model.gguf");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"GGUF-fake").unwrap();
        let pid_file = dir.path().join("models").join("fake.pid");
        (dir, script, model, pid_file)
    }

    fn test_settings() -> AiSettings {
        AiSettings {
            idle_unload_minutes: 0, // no idle unload inside tests
            ..AiSettings::default()
        }
    }

    fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut check: F) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn starts_becomes_ready_and_unloads_cleanly() {
        let (_dir, script, model, pid_file) = setup_fake_server();
        std::env::set_var("FAKE_PID_FILE", &pid_file);
        let runtime = LlmRuntime::start(_dir.path(), model.parent().unwrap());

        let state = runtime
            .ensure_loaded(&script, &model, &test_settings(), Duration::from_secs(30))
            .expect("ensure_loaded");
        let LoadState::Ready { port } = &state else {
            panic!("expected Ready, got {state:?}");
        };
        assert!(*port > 0);

        runtime.unload();
        assert!(matches!(runtime.snapshot().state, LoadState::Unloaded));

        // The fake server process is really gone (zombie-aware check).
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(wait_for(Duration::from_secs(5), || process_gone(pid)));
        let _ = std::env::remove_var("FAKE_PID_FILE");
    }

    #[test]
    fn reports_missing_binary_as_failed() {
        let (_dir, _script, model, _pid) = setup_fake_server();
        let runtime = LlmRuntime::start(_dir.path(), model.parent().unwrap());
        let bogus = _dir.path().join("no-such-llama-server");
        let state = runtime
            .ensure_loaded(&bogus, &model, &test_settings(), Duration::from_secs(15))
            .unwrap();
        let LoadState::Failed { detail } = state else {
            panic!("expected Failed, got {state:?}");
        };
        assert!(detail.contains("llama-server not found"), "{detail}");
    }

    #[test]
    fn missing_model_file_fails_with_readable_message() {
        let (_dir, script, model, _pid) = setup_fake_server();
        std::fs::remove_file(&model).unwrap();
        let runtime = LlmRuntime::start(_dir.path(), _dir.path());
        let state = runtime
            .ensure_loaded(&script, &model, &test_settings(), Duration::from_secs(15))
            .unwrap();
        assert!(matches!(state, LoadState::Failed { .. }));
    }

    #[test]
    fn terminates_stale_server_from_previous_run() {
        let (_dir, script, model, pid_file) = setup_fake_server();

        // Simulate a leftover server from a previous app run: same CLI shape
        // as the real thing (-m model, --port) and its pid recorded in the
        // supervisor's pid file, exactly like a crashed prior run would leave.
        let mut stale = std::process::Command::new(&script)
            .arg("-m")
            .arg(&model)
            .arg("--port")
            .arg("0")
            .env("FAKE_PID_FILE", &pid_file)
            .spawn()
            .unwrap();
        assert!(wait_for(Duration::from_secs(5), || pid_file.exists()));
        let stale_pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        // The supervisor's own pid file (what the sweep reads first).
        let models_dir = model.parent().unwrap();
        std::fs::write(models_dir.join("llama-server.pid"), stale_pid.to_string()).unwrap();

        // Starting a new runtime must kill it during the startup sweep. We
        // reap via try_wait — after a SIGKILL the process is a zombie of this
        // test until reaped, which is exactly the success signal here.
        let runtime = LlmRuntime::start(_dir.path(), models_dir);
        let reaped = wait_for(Duration::from_secs(10), || {
            matches!(stale.try_wait(), Ok(Some(_)) | Err(_))
        });
        assert!(reaped, "stale llama-server was not terminated by the sweep");

        runtime.unload();
        let _ = std::env::remove_var("FAKE_PID_FILE");
    }
}
