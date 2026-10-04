//! Master Voice provider: F5-TTS voice cloning rendered by the one-shot
//! Python script `services/voice-engine/render.py` (PEP 723 inline deps,
//! run through `uv` — the app still ships no Python of its own).
//!
//! Why this shape:
//! - F5-TTS (MIT) clones a user-supplied reference recording, so "your own
//!   voice" narrations run fully offline after the model is cached. The
//!   model (~1.3 GB) downloads on first render; the `uv` environment is
//!   created once and reused (fixed script path → stable uv cache key).
//! - One render = one subprocess for the **whole** script. F5 splits long
//!   text into batches internally, so per-chunk processes would only add
//!   ~90 s of interpreter/model start-up per chunk. One retry covers
//!   transient failures; there is no parts cache because losing a run
//!   re-does at most one render (same contract as the Piper/say single-shot
//!   path).
//! - Device defaults to CPU in the script: the MPS (Metal) path aborts
//!   intermittently on macOS 26 (`command encoder is already encoding`),
//!   which a subprocess cannot catch — CPU costs speed for reliability.
//!
//! The reference transcript is cached by the script in a
//! `<ref>.ref.txt` sidecar so later renders skip whisper transcription.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{AppError, AppResult};

use super::tts::{TextToSpeechProvider, TtsAudio, TtsStatus};

/// Render attempts: first the fast path (GPU when the script resolves one),
/// then the slow-but-certain CPU path. The MPS/GPU route on macOS 26
/// intermittently aborts with a Metal command-buffer assertion, which only
/// the parent process can observe — hence the explicit second device.
const RENDER_DEVICES: &[&str] = &["auto", "cpu"];

pub struct MasterVoiceProvider {
    /// Reference recording cloned for every render (`.wav`/`.mp3`/…).
    pub ref_path: PathBuf,
    /// Absolute path to `render.py` (empty = not found; check() reports it).
    script_path: PathBuf,
    /// Runner binary (tests override with a fake script; default "uv").
    uv_path: PathBuf,
    /// Speech rate multiplier passed through to F5-TTS `speed`.
    pub speed: f32,
    /// Transcode WAV → MP3 via ffmpeg when available.
    pub mp3_enabled: bool,
    /// Scratch/output directory for rendered files.
    out_dir: PathBuf,
}

impl MasterVoiceProvider {
    pub fn new(
        ref_path: impl Into<PathBuf>,
        script_path: impl Into<PathBuf>,
        speed: f32,
        mp3_enabled: bool,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ref_path: ref_path.into(),
            script_path: script_path.into(),
            uv_path: PathBuf::from("uv"),
            speed: speed.clamp(0.5, 2.0),
            mp3_enabled,
            out_dir: out_dir.into(),
        }
    }

    pub fn from_settings(
        s: &crate::db::TtsSettings,
        resource_dir: Option<&Path>,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::new(
            s.master_ref_path.clone(),
            resolve_render_script(resource_dir).unwrap_or_default(),
            s.speed,
            s.mp3_enabled,
            out_dir,
        )
    }

    #[cfg(test)]
    fn with_uv_binary(mut self, uv: impl Into<PathBuf>) -> Self {
        self.uv_path = uv.into();
        self
    }

    /// Locate `render.py`: bundled resource first (installed app), then the
    /// repo layout at compile time (dev runs and locally built apps).
    fn resolve(&self) -> Option<&Path> {
        (!self.script_path.as_os_str().is_empty() && self.script_path.is_file())
            .then_some(&self.script_path)
    }

    /// Locate the `uv` binary. GUI-launched apps get a minimal PATH
    /// (`/usr/bin:/bin:/usr/sbin:/sbin`), so homebrew installs are invisible
    /// to a plain PATH search — probe the usual install locations first.
    fn resolve_uv(&self) -> Option<PathBuf> {
        // Tests inject an explicit runner path.
        if self.uv_path != Path::new("uv") {
            return Some(self.uv_path.clone());
        }
        const KNOWN: &[&str] = &[
            "/opt/homebrew/bin/uv",
            "/usr/local/bin/uv",
            "/root/.local/bin/uv",
        ];
        for cand in KNOWN {
            let p = Path::new(cand);
            if p.is_file() {
                return Some(p.to_path_buf());
            }
        }
        if let Some(home) = dirs_home() {
            let p = home.join(".local/bin/uv");
            if p.is_file() {
                return Some(p);
            }
        }
        // Fall back to whatever PATH the process actually has.
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(if cfg!(windows) { "uv.exe" } else { "uv" }))
            .find(|p| p.is_file())
    }

    fn uv_available(&self) -> bool {
        let Some(uv) = self.resolve_uv() else {
            return false;
        };
        Command::new(uv)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|st| st.success())
            .unwrap_or(false)
    }

    /// Run the renderer once on `device`; Ok(()) means the WAV exists.
    fn run_once(&self, text_file: &Path, wav: &Path, device: &str) -> AppResult<()> {
        let script = self
            .resolve()
            .ok_or_else(|| AppError::msg(MISSING_SCRIPT_MSG))?;
        let uv = self.resolve_uv().ok_or_else(|| AppError::msg(UV_MSG))?;
        let output = Command::new(uv)
            .arg("run")
            .arg("--quiet")
            .arg(script)
            .arg("--device")
            .arg(device)
            .arg("--ref")
            .arg(&self.ref_path)
            .arg("--text-file")
            .arg(text_file)
            .arg("--out")
            .arg(wav)
            .arg("--speed")
            .arg(format!("{:.2}", self.speed))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        if output.status.success() && wav.is_file() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().filter(|l| l.starts_with("[render]")) {
                log::info!(target: "researchai::master-voice", "{line}");
            }
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // The RENDER_ERROR line is written by the script and carries the
        // actionable message; fall back to the log tail.
        let detail = stdout
            .lines()
            .rev()
            .find(|l| l.starts_with("RENDER_ERROR:"))
            .map(|l| l.trim_start_matches("RENDER_ERROR:").trim().to_string())
            .unwrap_or_else(|| tail(&stderr, 600));
        Err(AppError::msg(format!(
            "Master Voice render failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            if detail.is_empty() { "no error output".into() } else { detail }
        )))
    }
}

const MISSING_SCRIPT_MSG: &str =
    "The Master Voice render script was not found (services/voice-engine/render.py) — \
     reinstall the app or switch voice provider in Settings → Speech.";

const UV_MSG: &str =
    "The Master Voice needs the `uv` Python runner, which was not found on this machine \
     (install it with `brew install uv`, then restart the app) — or pick another voice in \
     Settings → Speech.";

/// Last `n` chars of `s`, trimmed of surrounding whitespace.
fn tail(s: &str, n: usize) -> String {
    let t = s.trim();
    if t.len() <= n {
        return t.to_string();
    }
    t[t.len() - n..].to_string()
}

/// Find the bundled or repo-local `render.py`.
pub fn resolve_render_script(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(res) = resource_dir {
        let bundled = res.join("voice-engine").join("render.py");
        if bundled.is_file() {
            return Some(bundled);
        }
        // Dev `resource_dir` is the src-tauri dir; the repo layout sits
        // three levels up from there (mirrors the bundled.rs dev probes).
        let dev = res.join("../../..").join("services");
        let dev = dev.join("voice-engine").join("render.py");
        if dev.is_file() {
            return Some(dev);
        }
    }
    // Compile-time repo layout: <repo>/apps/desktop/src-tauri → <repo>/services
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../services/voice-engine/render.py");
    repo.canonicalize().ok().filter(|p| p.is_file())
}

/// True when the F5-TTS checkpoint is already in the Hugging Face cache
/// (first render downloads ~1.3 GB when missing).
fn f5_model_cached() -> bool {
    let hub = match std::env::var_os("HF_HOME") {
        Some(h) => PathBuf::from(h),
        None => match dirs_home() {
            Some(h) => h.join(".cache").join("huggingface"),
            None => return false,
        },
    }
    .join("hub");
    let repo = hub.join("models--SWivid--F5-TTS").join("snapshots");
    let Ok(entries) = std::fs::read_dir(repo) else {
        return false;
    };
    entries.flatten().any(|snap| {
        snap.path()
            .join("F5TTS_v1_Base")
            .join("model_1250000.safetensors")
            .is_file()
    })
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

impl TextToSpeechProvider for MasterVoiceProvider {
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        // Deterministic preflight: fail fast with guidance, no retries.
        if !self.uv_available() {
            return Err(AppError::msg(UV_MSG));
        }
        self.resolve()
            .ok_or_else(|| AppError::msg(MISSING_SCRIPT_MSG))?;
        if !self.ref_path.is_file() {
            return Err(AppError::msg(format!(
                "The master voice recording was not found at “{}” — pick it again in \
                 Settings → Speech (Voice output → Master voice).",
                self.ref_path.display()
            )));
        }
        if text.trim().is_empty() {
            return Err(AppError::msg("There is nothing to speak."));
        }

        std::fs::create_dir_all(&self.out_dir)?;
        let wav = self.out_dir.join(format!("tts-{name_hint}.wav"));
        let _ = std::fs::remove_file(&wav);
        let text_file = self.out_dir.join(format!("tts-{name_hint}.text.txt"));
        std::fs::write(&text_file, text)?;

        let mut last_err: Option<AppError> = None;
        for (i, device) in RENDER_DEVICES.iter().enumerate() {
            if i > 0 {
                log::warn!(
                    target: "researchai::master-voice",
                    "render failed on device '{}' — retrying on '{}': {}",
                    RENDER_DEVICES[i - 1],
                    device,
                    last_err.as_ref().map(|e| e.to_string()).unwrap_or_default()
                );
                let _ = std::fs::remove_file(&wav);
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            match self.run_once(&text_file, &wav, device) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let _ = std::fs::remove_file(&text_file);
        if let Some(e) = last_err {
            // Never leave a partial render behind for the player to trip on.
            let _ = std::fs::remove_file(&wav);
            return Err(e);
        }

        if self.mp3_enabled && crate::services::transcription::ffmpeg_available() {
            match crate::services::tts::transcode_to_mp3(&wav, &self.out_dir) {
                Ok(mp3) => {
                    let bytes = std::fs::metadata(&mp3)?.len();
                    let duration_ms =
                        crate::services::tts::wav_duration_ms_from_header(&wav)?
                            .unwrap_or_default();
                    let _ = std::fs::remove_file(&wav);
                    return Ok(TtsAudio {
                        path: mp3.to_string_lossy().into_owned(),
                        format: "mp3".into(),
                        bytes,
                        duration_ms,
                    });
                }
                Err(e) => {
                    log::warn!(target: "researchai::master-voice", "mp3 transcode failed, keeping WAV: {e}");
                }
            }
        }
        TtsAudio::from_wav(&wav)
    }

    fn check(&self) -> TtsStatus {
        let binary_found = self.uv_available();
        let script_found = self.resolve().is_some();
        let ref_found = self.ref_path.is_file();
        TtsStatus {
            binary_found,
            // "model" in the UI chips: everything the render needs besides
            // the F5 checkpoint (which auto-downloads on first render).
            model_found: script_found && ref_found,
            ready: binary_found && script_found && ref_found,
            output_dir: self.out_dir.to_string_lossy().into_owned(),
        }
    }
}

/// Probe used by the status command: is the F5 checkpoint already cached
/// (vs. downloading on first render)? Surfaced as an informational chip.
pub fn f5_assets_cached() -> bool {
    f5_model_cached()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;
    use std::os::unix::fs::PermissionsExt as _;

    /// Minimal valid PCM WAV (24 kHz mono 16-bit), like the tts.rs fakes.
    fn write_wav(path: &Path, samples: usize) {
        let data = vec![0u8; samples * 2];
        let mut bytes: Vec<u8> = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        std::fs::write(path, bytes).unwrap();
    }

    /// Fake `uv` that ignores the script and copies a prepared WAV to the
    /// `--out` argument (proves the whole spawn contract: flags, text file,
    /// out path, exit status handling).
    fn fake_uv(dir: &Path, exit_code: i32) -> PathBuf {
        let payload = dir.join("payload.wav");
        write_wav(&payload, 480); // 20 ms @ 24 kHz
        let script = dir.join("fake-uv.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nout=\"\"; prev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--out\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\ncat '{p}' > \"$out\"\nexit {exit_code}\n",
                p = payload.display(),
                exit_code = exit_code
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn provider(dir: &Path, uv: &Path) -> MasterVoiceProvider {
        let ref_wav = dir.join("master.wav");
        if !ref_wav.exists() {
            write_wav(&ref_wav, 240);
        }
        let script = dir.join("render.py");
        if !script.exists() {
            std::fs::write(&script, "# test placeholder").unwrap();
        }
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        MasterVoiceProvider::new(&ref_wav, &script, 1.0, false, out).with_uv_binary(uv)
    }

    #[test]
    fn renders_via_the_uv_contract() {
        let dir = TempDir::new_with_label("master-voice");
        let uv = fake_uv(dir.path(), 0);
        let prov = provider(dir.path(), &uv);

        let audio = prov.render("Hello from the master voice.", "mv-test").unwrap();
        assert_eq!(audio.format, "wav");
        assert!(audio.path.ends_with("tts-mv-test.wav"), "{}", audio.path);
        assert_eq!(audio.duration_ms, 20); // exact header: 480 frames @ 24 kHz
        // The temporary text file is cleaned up.
        assert!(!dir.path().join("out").join("mv-test.text.txt").exists());
    }

    #[test]
    fn failing_renderer_reports_actionable_error_after_retry() {
        let dir = TempDir::new_with_label("master-voice-fail");
        let uv = fake_uv(dir.path(), 7);
        let prov = provider(dir.path(), &uv);
        let err = prov.render("Hello.", "mv-fail").unwrap_err().to_string();
        assert!(err.contains("Master Voice render failed"), "{err}");
        assert!(err.contains("exit 7"), "{err}");
        // No partial output or temp files left behind.
        assert!(!dir.path().join("out").join("tts-mv-fail.wav").exists());
        assert!(!dir.path().join("out").join("mv-fail.text.txt").exists());
    }

    #[test]
    fn check_needs_uv_script_and_reference() {
        let dir = TempDir::new_with_label("master-voice-check");
        let uv = fake_uv(dir.path(), 0);
        let prov = provider(dir.path(), &uv);
        let st = prov.check();
        assert!(st.binary_found, "fake uv must be found");
        assert!(st.model_found && st.ready, "script + ref present → ready");

        let missing_ref = MasterVoiceProvider::new(
            dir.path().join("nope.wav"),
            dir.path().join("render.py"),
            1.0,
            false,
            dir.path().join("out"),
        )
        .with_uv_binary(&uv);
        let st = missing_ref.check();
        assert!(!st.model_found && !st.ready);
    }

    #[test]
    fn render_without_uv_is_a_guidance_error_not_a_retry_loop() {
        let dir = TempDir::new_with_label("master-voice-nouv");
        let uv = fake_uv(dir.path(), 0);
        let prov = provider(dir.path(), &uv);
        let prov = prov.with_uv_binary(dir.path().join("definitely-missing-uv"));
        let err = prov.render("Hello.", "x").unwrap_err().to_string();
        assert!(err.contains("`uv`"), "{err}");
        assert!(err.contains("brew install uv"), "{err}");
    }

    #[test]
    fn resolve_render_script_finds_the_repo_script_in_dev() {
        // In dev builds CARGO_MANIFEST_DIR points into this repo.
        let found = resolve_render_script(None);
        assert!(found.is_some(), "repo-local render.py must resolve in dev");
        assert!(found.unwrap().ends_with("services/voice-engine/render.py"));
    }

    /// Live chain proof: real `uv`, real `render.py`, real F5-TTS model and
    /// a real reference recording staged into a temp dir. Renders a short
    /// sentence and checks the WAV. Slow (model load + synthesis, ~1–4 min)
    /// and needs the model/network, so it is ignored by default:
    ///
    /// ```sh
    /// cargo test live_master_voice -- --ignored --exact \
    ///   services::master_voice::tests::live_master_voice_renders_with_real_uv
    /// ```
    #[test]
    #[ignore = "live F5-TTS render; needs uv + model (~1-4 min)"]
    fn live_master_voice_renders_with_real_uv() {
        let dir = TempDir::new_with_label("master-voice-live");
        // Stage a real recording (the master voice sample on this machine).
        let src = Path::new(
            "/Users/odere/Downloads/take_I.wav",
        );
        if !src.is_file() {
            eprintln!("no reference recording at {} — skipping", src.display());
            return;
        }
        let ref_wav = dir.path().join("master-voice.wav");
        std::fs::copy(src, &ref_wav).unwrap();
        let out = dir.path().join("out");
        let script = resolve_render_script(None).expect("render.py resolves in dev");
        let prov =
            MasterVoiceProvider::new(&ref_wav, &script, 1.0, false, &out);
        if !prov.check().ready {
            eprintln!("uv not available — skipping");
            return;
        }
        let audio = prov
            .render(
                "The master voice chain works end to end, from the Rust provider to the \
                 Python renderer and back.",
                "live-master",
            )
            .unwrap();
        assert_eq!(audio.format, "wav");
        assert!(Path::new(&audio.path).is_file());
        assert!(audio.duration_ms > 1_000, "expected >1 s, got {} ms", audio.duration_ms);
        // First run transcribes the reference and caches the transcript
        // beside the staged file so later renders skip whisper.
        let sidecar = ref_wav.with_file_name(format!(
            "{}.ref.txt",
            ref_wav.file_name().unwrap().to_string_lossy()
        ));
        assert!(sidecar.is_file(), "transcript sidecar must be cached next to the reference");
    }
}
