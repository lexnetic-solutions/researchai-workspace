//! Text-to-speech (Phase 8, spec §21, §47.7) behind the neutral
//! `TextToSpeechProvider` interface.
//!
//! Piper (spec default) renders WAV files locally from a user-installed
//! binary + `.onnx` voice model; the macOS `say` provider is a cfg-gated
//! convenience fallback. Same philosophy as whisper.cpp (Phase 7): the app
//! never bundles or downloads binaries/models, and every provider runs as a
//! one-shot subprocess — nothing to load or unload.
//!
//! Optional MP3 export (`-b:a 64k` mono) goes through ffmpeg when enabled and
//! available; the returned [`TtsAudio`] reports exactly what was produced.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{AppError, AppResult};

// ---------------------------------------------------------------------------
// Data model + provider trait
// ---------------------------------------------------------------------------

/// One rendered audio file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsAudio {
    /// Absolute path of the rendered file (WAV, or MP3 when transcoded).
    pub path: String,
    /// "wav" or "mp3".
    pub format: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Rough spoken duration in ms (bytes / [sr·ch·2] for WAV).
    pub duration_ms: u64,
}

/// Availability of the external pieces a voice provider needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsStatus {
    pub binary_found: bool,
    pub model_found: bool,
    /// True when the provider can render right now.
    pub ready: bool,
    /// Where the output will land (informational, surfaced in the UI).
    pub output_dir: String,
}

/// Provider-agnostic speech synthesis interface (spec §47.7). Implemented by
/// [`PiperProvider`] (spec default) and, on macOS, [`MacOsSayProvider`].
pub trait TextToSpeechProvider: Send + Sync {
    /// Render `text` (already narration-shaped) to an audio file.
    /// Blocking; call from a worker thread.
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio>;
    /// Cheap availability probe for the Settings UI.
    fn check(&self) -> TtsStatus;
}

// ---------------------------------------------------------------------------
// Piper provider
// ---------------------------------------------------------------------------

/// Local Piper synthesis: `piper --model <onnx> --output_file <wav>` with
/// text on stdin. Speed is `--length-scale = 1 / speed`.
#[derive(Debug, Clone)]
pub struct PiperProvider {
    /// Path to the `piper` executable.
    pub piper_path: PathBuf,
    /// Path to a Piper voice model (`.onnx`); its `.json` sits beside it.
    pub voice_model_path: PathBuf,
    /// Speech rate multiplier (0.5–2.0; 1.0 = normal).
    pub speed: f32,
    /// Transcode WAV → MP3 via ffmpeg when available.
    pub mp3_enabled: bool,
    /// Scratch/output directory for rendered files.
    pub out_dir: PathBuf,
}

impl PiperProvider {
    pub fn new(
        piper_path: impl Into<PathBuf>,
        voice_model_path: impl Into<PathBuf>,
        speed: f32,
        mp3_enabled: bool,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            piper_path: piper_path.into(),
            voice_model_path: voice_model_path.into(),
            speed: speed.clamp(0.5, 2.0),
            mp3_enabled,
            out_dir: out_dir.into(),
        }
    }

    pub fn from_settings(
        s: &crate::db::TtsSettings,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::new(
            s.piper_path.clone(),
            s.voice_model_path.clone(),
            s.speed,
            s.mp3_enabled,
            out_dir,
        )
    }

    #[cfg(test)]
    fn with_piper_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.piper_path = binary.into();
        self
    }

    /// Validate tooling, run piper, then optionally transcode to MP3.
    fn render_internal(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        if !self.piper_path.exists() {
            return Err(AppError::msg(format!(
                "Piper was not found at “{}” — install piper (e.g. `brew install piper`) \
                 or set its path in Settings → Speech.",
                self.piper_path.display()
            )));
        }
        if !self.voice_model_path.exists() {
            return Err(AppError::msg(format!(
                "The Piper voice model was not found at “{}” — download a voice from the \
                 Piper samples page (an .onnx file) and set it in Settings → Speech.",
                self.voice_model_path.display()
            )));
        }
        std::fs::create_dir_all(&self.out_dir)?;

        let wav = self.out_dir.join(format!("tts-{name_hint}.wav"));
        let _ = std::fs::remove_file(&wav); // start clean

        let mut cmd = Command::new(&self.piper_path);
        cmd.arg("--model")
            .arg(&self.voice_model_path)
            .arg("--length-scale")
            .arg((1.0 / self.speed).to_string())
            .arg("--output_file")
            .arg(&wav);
        #[cfg(target_os = "macos")]
        {
            // Piper inherits CoreAudio access; headless CI needs no device.
            cmd.env("COREAUDIO", "1");
        }

        use std::io::Write;
        let mut child = cmd.stdin(std::process::Stdio::piped()).spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(text.as_bytes())?;
        }
        let output = child.wait_with_output()?;

        if !output.status.success() || !wav.exists() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            let stderr = if stderr.len() > 600 { &stderr[..600] } else { stderr };
            return Err(AppError::msg(format!(
                "Piper failed (exit status {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { "no error output" } else { stderr }
            )));
        }

        let wav_bytes = std::fs::metadata(&wav)?.len();
        if self.mp3_enabled && crate::services::transcription::ffmpeg_available() {
            match transcode_to_mp3(&wav, &self.out_dir) {
                Ok(mp3) => {
                    let bytes = std::fs::metadata(&mp3)?.len();
                    let _ = std::fs::remove_file(&wav);
                    return Ok(TtsAudio {
                        path: mp3.to_string_lossy().into_owned(),
                        format: "mp3".into(),
                        bytes,
                        duration_ms: wav_duration_ms(wav_bytes),
                    });
                }
                Err(e) => {
                    log::warn!(target: "researchai::tts", "mp3 transcode failed, keeping WAV: {e}");
                }
            }
        }

        Ok(TtsAudio {
            path: wav.to_string_lossy().into_owned(),
            format: "wav".into(),
            bytes: wav_bytes,
            duration_ms: wav_duration_ms(wav_bytes),
        })
    }
}

impl TextToSpeechProvider for PiperProvider {
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        self.render_internal(text, name_hint)
    }

    fn check(&self) -> TtsStatus {
        let binary_found = self.piper_path.exists();
        let model_found = self.voice_model_path.exists();
        TtsStatus {
            binary_found,
            model_found,
            ready: binary_found && model_found,
            output_dir: self.out_dir.to_string_lossy().into_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// macOS `say` fallback (cfg-gated; also used by tests via a fake binary)
// ---------------------------------------------------------------------------

/// macOS built-in speech synthesis. Ships with the OS — zero-install, but
/// voice quality is system-dependent, so Piper stays the spec default.
#[derive(Debug, Clone)]
pub struct MacOsSayProvider {
    /// Voice name for `say -v` (empty = system default).
    pub voice: String,
    /// Speech rate for `say -r` (words per minute; 1.0 ≈ 175 wpm).
    pub speed: f32,
    /// Transcode AIFF/AU → MP3 via ffmpeg when available (`say -o` writes
    /// AIFF for `.aiff`, WAVE for `.wav`).
    pub mp3_enabled: bool,
    /// Scratch/output directory.
    pub out_dir: PathBuf,
    /// Binary to invoke (overridable in tests with a fake shell script).
    binary: PathBuf,
}

impl MacOsSayProvider {
    #[cfg(target_os = "macos")]
    pub fn from_settings(s: &crate::db::TtsSettings, out_dir: impl Into<PathBuf>) -> Self {
        Self::new(s.macos_voice.clone(), s.speed, s.mp3_enabled, out_dir)
    }

    pub fn new(
        voice: String,
        speed: f32,
        mp3_enabled: bool,
        out_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            voice,
            speed: speed.clamp(0.5, 2.0),
            mp3_enabled,
            out_dir: out_dir.into(),
            binary: PathBuf::from("say"),
        }
    }

    #[cfg(test)]
    fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    fn render_internal(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        std::fs::create_dir_all(&self.out_dir)?;
        let wav = self.out_dir.join(format!("tts-{name_hint}.wav"));
        let _ = std::fs::remove_file(&wav);

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-o").arg(&wav).arg("--data-format=LEF32@22050");
        if !self.voice.trim().is_empty() {
            cmd.arg("-v").arg(self.voice.trim());
        }
        if (self.speed - 1.0).abs() > f32::EPSILON {
            cmd.arg("-r").arg(format!("{}", (175.0 * self.speed) as u32));
        }
        cmd.arg(text);

        let output = cmd.output()?;
        if !output.status.success() || !wav.exists() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            let stderr = if stderr.len() > 400 { &stderr[..400] } else { stderr };
            return Err(AppError::msg(format!(
                "macOS say failed (exit status {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { "no error output" } else { stderr }
            )));
        }

        let bytes = std::fs::metadata(&wav)?.len();
        if self.mp3_enabled && crate::services::transcription::ffmpeg_available() {
            if let Ok(mp3) = transcode_to_mp3(&wav, &self.out_dir) {
                let mp3_bytes = std::fs::metadata(&mp3)?.len();
                let _ = std::fs::remove_file(&wav);
                return Ok(TtsAudio {
                    path: mp3.to_string_lossy().into_owned(),
                    format: "mp3".into(),
                    bytes: mp3_bytes,
                    // 22.05 kHz f32 stereo from --data-format would need the
                    // real header to be exact; keep the WAV estimate.
                    duration_ms: wav_duration_ms(bytes),
                });
            }
        }

        Ok(TtsAudio {
            path: wav.to_string_lossy().into_owned(),
            format: "wav".into(),
            bytes,
            duration_ms: wav_duration_ms(bytes),
        })
    }
}

impl TextToSpeechProvider for MacOsSayProvider {
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        self.render_internal(text, name_hint)
    }

    fn check(&self) -> TtsStatus {
        let binary_found = Command::new(&self.binary)
            .arg("-v")
            .arg("?")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        TtsStatus {
            binary_found,
            model_found: true, // OS voices need no model file
            ready: binary_found,
            output_dir: self.out_dir.to_string_lossy().into_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Estimate spoken duration from WAV payload size (16-bit mono @ 22.05 kHz).
/// Good enough for a UI label; the real file carries exact timing.
fn wav_duration_ms(bytes: u64) -> u64 {
    const BYTES_PER_MS: u64 = 22_050 * 2 / 1000; // sr · 16-bit · 1ch
    bytes.saturating_div(BYTES_PER_MS)
}

/// Transcode a rendered WAV to mono MP3 (64 kbps) beside it via ffmpeg.
fn transcode_to_mp3(src: &Path, out_dir: &Path) -> AppResult<PathBuf> {
    let out = out_dir.join(format!(
        "{}.mp3",
        src.file_stem().and_then(|s| s.to_str()).unwrap_or("tts")
    ));
    let _ = std::fs::remove_file(&out);
    let output = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(src)
        .args(["-ac", "1", "-b:a", "64k"])
        .arg(&out)
        .output()?;
    if !output.status.success() || !out.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let stderr = if stderr.len() > 400 {
            &stderr[stderr.len() - 400..]
        } else {
            stderr
        };
        return Err(AppError::msg(format!(
            "ffmpeg MP3 transcode failed: {stderr}"
        )));
    }
    Ok(out)
}

/// Build the provider for the saved settings (Phase 8 selection).
pub fn provider_for(
    s: &crate::db::TtsSettings,
    out_dir: impl Into<PathBuf>,
) -> AppResult<Box<dyn TextToSpeechProvider>> {
    match s.provider.as_str() {
        "macos-say" => {
            #[cfg(target_os = "macos")]
            {
                Ok(Box::new(MacOsSayProvider::from_settings(s, out_dir)))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (s, out_dir);
                Err(AppError::msg(
                    "The macOS say provider is only available on macOS — switch to Piper in Settings → Speech.",
                ))
            }
        }
        _ => Ok(Box::new(PiperProvider::from_settings(s, out_dir))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable fake `piper` that reads stdin, writes a small
    /// WAV-shaped file and exits with the given code (POSIX sh loop for the
    /// `--output_file <path>` value — no bashisms).
    fn write_fake_piper(dir: &Path, exit_code: i32) -> PathBuf {
        let script = dir.join("fake-piper.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output_file\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\ncat > /dev/null\nprintf 'RIFF' > \"$out\"\nprintf '\\x29\\x09\\x00\\x00' >> \"$out\"\nprintf 'WAVEfmt ' >> \"$out\"\nhead -c 2213 /dev/zero >> \"$out\"\nexit {exit_code}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn piper(dir: &Path, mp3: bool) -> PiperProvider {
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let model = dir.join("voice.onnx");
        std::fs::write(&model, b"fake").unwrap();
        PiperProvider::new(dir.join("fake-piper.sh"), model, 1.0, mp3, out)
    }

    #[test]
    fn piper_renders_wav_via_fake_binary() {
        let dir = TempDir::new_with_label("tts");
        let cli = write_fake_piper(dir.path(), 0);
        let prov = piper(dir.path(), false).with_piper_binary(&cli);

        let audio = prov.render("Hello from the fake voice.", "test").unwrap();
        assert_eq!(audio.format, "wav");
        assert!(audio.path.ends_with("tts-test.wav"));
        let on_disk = std::fs::metadata(&audio.path).unwrap().len();
        assert_eq!(audio.bytes, on_disk);
        assert_eq!(audio.duration_ms, on_disk / 44); // 44 bytes per ms @ 22.05 kHz 16-bit
    }

    #[test]
    fn piper_missing_tools_are_actionable() {
        let dir = TempDir::new_with_label("tts");
        let cli = write_fake_piper(dir.path(), 0);
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let model = dir.path().join("voice.onnx");
        std::fs::write(&model, b"fake").unwrap();

        // Binary missing.
        let no_binary = PiperProvider::new(
            dir.path().join("nope-piper"),
            &model,
            1.0,
            false,
            &out,
        );
        let err = no_binary.render("hi", "x").unwrap_err().to_string();
        assert!(err.contains("Piper was not found"), "{err}");

        // Binary present but voice model missing.
        let no_model = PiperProvider::new(
            &cli,
            dir.path().join("nope.onnx"),
            1.0,
            false,
            &out,
        );
        let err = no_model.render("hi", "x").unwrap_err().to_string();
        assert!(err.contains("voice model was not found"), "{err}");
    }

    #[test]
    fn piper_failure_surfaces_stderr() {
        let dir = TempDir::new_with_label("tts");
        let cli = write_fake_piper(dir.path(), 2);
        let prov = piper(dir.path(), false).with_piper_binary(&cli);

        let err = prov.render("hi", "boom").unwrap_err().to_string();
        assert!(err.contains("exit status 2"), "{err}");
    }

    #[test]
    fn say_provider_renders_via_fake_binary() {
        let dir = TempDir::new_with_label("tts");
        let script = dir.path().join("fake-say.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n# emulate macOS say: -o <file>; speak text is last arg\nprev=\"\"\nout=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\nprintf 'RIFF' > \"$out\"\nprintf '\\x52\\x12\\x00\\x00' >> \"$out\"  # 4690 bytes ≈ 106 ms\nprintf 'WAVEfmt ' >> \"$out\"\nhead -c 4678 /dev/zero >> \"$out\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out_dir = dir.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let prov = MacOsSayProvider::new(String::new(), 1.0, false, &out_dir)
            .with_binary(&script);
        let audio = prov.render("Reading the abstract aloud.", "say").unwrap();
        assert_eq!(audio.format, "wav");
        let on_disk = std::fs::metadata(&audio.path).unwrap().len();
        assert_eq!(audio.duration_ms, on_disk / 44);
    }

    #[test]
    fn provider_for_selects_by_settings() {
        let dir = TempDir::new_with_label("tts");
        let out = dir.path().join("out");
        let piper_settings = crate::db::TtsSettings {
            provider: "piper".into(),
            ..crate::db::TtsSettings::default()
        };
        assert!(provider_for(&piper_settings, &out).is_ok());
        // Non-macOS cfg path returns the macOS error; on macOS it constructs.
        let say_settings = crate::db::TtsSettings {
            provider: "macos-say".into(),
            ..crate::db::TtsSettings::default()
        };
        let _ = provider_for(&say_settings, &out);
    }
}
