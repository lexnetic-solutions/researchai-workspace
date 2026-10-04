//! Lecture transcription (Phase 7, spec §20, §47.7) behind the neutral
//! `SpeechToTextProvider` interface.
//!
//! [`WhisperCppProvider`] runs a user-installed whisper.cpp `whisper-cli`
//! binary as a one-shot subprocess per job — batch mode, no resident server,
//! nothing to unload. Binary and model paths come from settings (`stt.*`);
//! the app never bundles or downloads models (offline-first, user-owned
//! tooling, spec §4).
//!
//! Audio handling is honest, not best-effort:
//!
//! - 16 kHz mono PCM WAV is accepted directly (whisper.cpp's native format).
//! - Anything else goes through `ffmpeg` (must already be on PATH) to produce
//!   a 16 kHz mono `pcm_s16le` WAV; with conversion disabled the user gets an
//!   actionable error instead of silent failure.
//!
//! Output is the whisper `-oj` JSON (`transcription[]` with millisecond
//! offsets), parsed into [`TranscriptionSegment`]s and grouped into
//! timestamped `[mm:ss]` chunks by the document pipeline (`library.rs`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// One timestamped utterance from the recognizer (milliseconds from stream
/// start, matching whisper.cpp's `offsets.from` / `offsets.to`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Full transcript of one audio file.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub segments: Vec<TranscriptionSegment>,
    /// Language whisper detected (ISO code) when it was not pinned.
    pub language: Option<String>,
    /// Total transcribed duration in ms (end of the last segment).
    pub duration_ms: u64,
}

/// Availability of the external pieces the provider needs. Rendered in
/// Settings → Speech so the user can fix paths before running a job.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SttStatus {
    pub cli_found: bool,
    pub model_found: bool,
    pub ffmpeg_found: bool,
}

/// Provider-agnostic speech-to-text interface (spec §47.7). Implemented today
/// by the local whisper.cpp CLI; a future provider (e.g. a whisper.cpp server
/// or another engine) can slot in without touching the UI or pipeline.
pub trait SpeechToTextProvider: Send + Sync {
    /// Transcribe one audio file. Blocking; call from a worker thread.
    fn transcribe(&self, audio: &Path) -> AppResult<TranscriptionResult>;
    /// Cheap availability probe for the Settings UI.
    fn check(&self) -> SttStatus;
}

// ---------------------------------------------------------------------------
// whisper.cpp provider
// ---------------------------------------------------------------------------

/// Local whisper.cpp CLI transcription (batch mode: one subprocess per job).
#[derive(Debug, Clone)]
pub struct WhisperCppProvider {
    /// Path to the `whisper-cli` (or legacy `main`) binary.
    pub cli_path: PathBuf,
    /// Path to a whisper.cpp GGML model (e.g. `ggml-base.bin`).
    pub model_path: PathBuf,
    /// Language hint: `"auto"` (detect) or an ISO code like `"en"`.
    pub language: String,
    /// Convert non-16k-mono inputs via ffmpeg when available.
    pub convert_with_ffmpeg: bool,
    /// Scratch directory for converted audio and JSON output.
    pub work_dir: PathBuf,
}

impl WhisperCppProvider {
    pub fn new(
        cli_path: impl Into<PathBuf>,
        model_path: impl Into<PathBuf>,
        language: String,
        convert_with_ffmpeg: bool,
        work_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            cli_path: cli_path.into(),
            model_path: model_path.into(),
            language,
            convert_with_ffmpeg,
            work_dir: work_dir.into(),
        }
    }

    /// Build from saved settings; `work_dir` is the app's scratch space.
    pub fn from_settings(
        s: &crate::db::SttSettings,
        work_dir: impl Into<PathBuf>,
    ) -> Self {
        Self::new(
            s.whisper_cli_path.clone(),
            s.whisper_model_path.clone(),
            s.language.clone(),
            s.convert_with_ffmpeg,
            work_dir,
        )
    }

    /// Ensure the input is a whisper-friendly WAV, converting via ffmpeg when
    /// allowed. A WAV with the wrong rate/channels is reported honestly
    /// rather than silently re-encoded when conversion is off.
    fn prepare_audio(&self, audio: &Path) -> AppResult<PathBuf> {
        match sniff_wav(audio) {
            Ok(_) => Ok(audio.to_path_buf()),
            Err(reason) => {
                if !self.convert_with_ffmpeg {
                    return Err(AppError::UnsupportedFileType(format!(
                        "{reason}. Enable “Convert with ffmpeg” in Settings → Speech, \
                         or provide a 16 kHz mono WAV."
                    )));
                }
                if !ffmpeg_available() {
                    return Err(AppError::msg(
                        "ffmpeg was not found on PATH — install ffmpeg (e.g. `brew install \
                         ffmpeg`) or provide a 16 kHz mono WAV."
                    ));
                }
                convert_with_ffmpeg(audio, &self.work_dir)
            }
        }
    }

    /// Run whisper-cli on `wav` and return the path of the produced JSON.
    fn run_cli(&self, wav: &Path) -> AppResult<PathBuf> {
        if !self.cli_path.exists() {
            return Err(AppError::msg(format!(
                "whisper-cli was not found at “{}” — set the binary path in Settings → Speech.",
                self.cli_path.display()
            )));
        }
        if !self.model_path.exists() {
            return Err(AppError::msg(format!(
                "Whisper model was not found at “{}” — set the model path in Settings → Speech \
                 (e.g. ggml-base.bin from whisper.cpp).",
                self.model_path.display()
            )));
        }
        std::fs::create_dir_all(&self.work_dir)?;
        let base = self.work_dir.join("transcript");
        let json_path = base.with_extension("json");
        let _ = std::fs::remove_file(&json_path);

        let mut args: Vec<String> = vec![
            "-m".into(),
            self.model_path.to_string_lossy().into_owned(),
            "-f".into(),
            wav.to_string_lossy().into_owned(),
            "-oj".into(),
            "-of".into(),
            base.to_string_lossy().into_owned(),
        ];
        let lang = self.language.trim();
        if !lang.is_empty() && !lang.eq_ignore_ascii_case("auto") {
            args.push("-l".into());
            args.push(lang.to_ascii_lowercase());
        }

        let output = Command::new(&self.cli_path)
            .args(&args)
            .current_dir(&self.work_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            let stderr = if stderr.len() > 600 { &stderr[..600] } else { stderr };
            return Err(AppError::msg(format!(
                "whisper-cli failed (exit status {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { "no error output" } else { stderr }
            )));
        }
        if !json_path.exists() {
            return Err(AppError::msg(
                "whisper-cli produced no JSON output — rebuild whisper.cpp with JSON support \
                 (the -oj flag) or check the binary path.",
            ));
        }
        Ok(json_path)
    }
}

impl SpeechToTextProvider for WhisperCppProvider {
    fn transcribe(&self, audio: &Path) -> AppResult<TranscriptionResult> {
        let wav = self.prepare_audio(audio)?;
        let json_path = self.run_cli(&wav)?;
        parse_whisper_json(&std::fs::read_to_string(&json_path)?)
    }

    fn check(&self) -> SttStatus {
        SttStatus {
            cli_found: self.cli_path.exists(),
            model_found: self.model_path.exists(),
            ffmpeg_found: ffmpeg_available(),
        }
    }
}

// ---------------------------------------------------------------------------
// Audio probing + ffmpeg conversion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct WavInfo {
    sample_rate: u32,
    channels: u16,
}

/// Sniff RIFF/WAVE and require whisper.cpp's native input: 16 kHz mono PCM.
/// Errors carry the concrete mismatch so the user can act on it.
fn sniff_wav(path: &Path) -> Result<WavInfo, String> {
    use std::io::{Read, Seek};

    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("Could not open audio file: {e}"))?;
    let mut header = [0u8; 12];
    f.read_exact(&mut header)
        .map_err(|_| "File is too small to be audio.".to_string())?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Not a WAV file (missing RIFF/WAVE header).".into());
    }

    // Walk chunks to the fmt chunk (files may carry extraneous metadata).
    let mut pos = 12u64;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    while pos + 8 <= meta.len() {
        f.seek(std::io::SeekFrom::Start(pos))
            .map_err(|e| e.to_string())?;
        let mut chunk = [0u8; 8];
        if f.read_exact(&mut chunk).is_err() {
            break;
        }
        let id = &chunk[0..4];
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        if id == b"fmt " && size >= 16 {
            let mut fmt = vec![0u8; size as usize];
            f.read_exact(&mut fmt)
                .map_err(|_| "Corrupt WAV fmt chunk.".to_string())?;
            let audio_format = u16::from_le_bytes([fmt[0], fmt[1]]);
            let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
            let sample_rate =
                u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
            if audio_format != 1 {
                return Err(format!(
                    "WAV is not 16-bit PCM (format tag {audio_format}); whisper.cpp needs \
                     plain PCM input."
                ));
            }
            let info = WavInfo { sample_rate, channels };
            return match (info.sample_rate, info.channels) {
                (16_000, 1) => Ok(info),
                (rate, 1) => Err(format!(
                    "WAV is {rate} Hz; whisper.cpp expects 16 kHz."
                )),
                (16_000, ch) => Err(format!(
                    "WAV has {ch} channels; whisper.cpp expects mono."
                )),
                (rate, ch) => Err(format!(
                    "WAV is {rate} Hz / {ch} channels; whisper.cpp expects 16 kHz mono."
                )),
            };
        }
        // Chunks are word-aligned.
        pos += 8 + size + (size & 1);
    }
    Err("WAV file has no fmt chunk.".into())
}

/// Well-known install prefixes probed after PATH. A GUI-launched app gets
/// launchd's minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), so Homebrew's
/// `ffmpeg` is invisible to a bare `Command::new("ffmpeg")` even though it
/// is installed — the same reason `resolve_uv` probes explicitly.
const FFMPEG_KNOWN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/bin"];

/// Absolute path of the `ffmpeg` executable: every directory on the process
/// PATH first (the user's own installs win), then the well-known prefixes.
/// `None` when no candidate exists — callers surface install guidance.
pub fn resolve_ffmpeg() -> Option<PathBuf> {
    resolve_ffmpeg_in(&std::env::var_os("PATH")?, FFMPEG_KNOWN_DIRS)
}

/// Testable core of [`resolve_ffmpeg`]: first `ffmpeg` file found in
/// `path_var`'s directories, then in `known` (empty PATH entries skipped).
pub(crate) fn resolve_ffmpeg_in(
    path_var: &std::ffi::OsStr,
    known: &[&str],
) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    for dir in std::env::split_paths(path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(exe);
        if cand.is_file() {
            return Some(cand);
        }
    }
    for dir in known {
        let cand = Path::new(dir).join(exe);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// True when a usable `ffmpeg` is installed (on PATH or a known prefix).
pub fn ffmpeg_available() -> bool {
    resolve_ffmpeg()
        .map(|ff| {
            Command::new(&ff)
                .arg("-version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Media duration via `ffprobe` (ships in the same install as ffmpeg),
/// in milliseconds. `None` when ffprobe is absent or the file is unreadable.
pub fn ffprobe_duration_ms(path: &Path) -> Option<u64> {
    let exe = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let probe = resolve_ffmpeg()?.with_file_name(exe);
    if !probe.is_file() {
        return None;
    }
    let out = Command::new(&probe)
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let secs: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    if secs.is_finite() && secs >= 0.0 {
        Some((secs * 1000.0).round() as u64)
    } else {
        None
    }
}

/// Convert any ffmpeg-readable input to 16 kHz mono `pcm_s16le` WAV.
pub fn convert_with_ffmpeg(src: &Path, work_dir: &Path) -> AppResult<PathBuf> {
    std::fs::create_dir_all(work_dir)?;
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    let out = work_dir.join(format!("converted-{stem}.wav"));
    // Start clean so a stale file can't masquerade as this job's output.
    let _ = std::fs::remove_file(&out);

    let ff = resolve_ffmpeg().ok_or_else(|| {
        AppError::msg(
            "ffmpeg was not found — install it (e.g. `brew install ffmpeg`) so \
             non-WAV audio can be converted.",
        )
    })?;
    let output = Command::new(&ff)
        .args(["-y", "-i"])
        .arg(src)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(&out)
        .output()?;

    if !output.status.success() || !out.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let stderr = if stderr.len() > 400 { &stderr[..400] } else { stderr };
        return Err(AppError::msg(format!(
            "ffmpeg conversion failed: {}",
            if stderr.is_empty() { "unknown error" } else { stderr }
        )));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// whisper -oj JSON parsing + timestamp helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WhisperJson {
    #[serde(default)]
    transcription: Vec<WhisperSegJson>,
    #[serde(default)]
    result: WhisperMetaJson,
}

#[derive(Debug, Deserialize)]
struct WhisperSegJson {
    offsets: WhisperOffsetsJson,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct WhisperOffsetsJson {
    from: u64,
    to: u64,
}

#[derive(Debug, Deserialize, Default)]
struct WhisperMetaJson {
    #[serde(default)]
    language: Option<String>,
}

/// Parse whisper.cpp `-oj` output into segments.
pub fn parse_whisper_json(json: &str) -> AppResult<TranscriptionResult> {
    let parsed: WhisperJson = serde_json::from_str(json).map_err(|e| {
        AppError::msg(format!(
            "Could not parse whisper JSON output: {e}"
        ))
    })?;
    let duration_ms = parsed
        .transcription
        .iter()
        .map(|s| s.offsets.to)
        .max()
        .unwrap_or(0);
    let segments = parsed
        .transcription
        .into_iter()
        .map(|s| TranscriptionSegment {
            start_ms: s.offsets.from,
            end_ms: s.offsets.to,
            text: s.text.trim().to_string(),
        })
        .collect();
    Ok(TranscriptionResult {
        segments,
        language: parsed.result.language,
        duration_ms,
    })
}

/// `[mm:ss]` marker used in transcript chunks (hours roll into minutes).
pub fn format_timestamp_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    format!("[{:02}:{:02}]", total_secs / 60, total_secs % 60)
}

/// Group consecutive segments into retrieval-sized chunks: each chunk carries
/// at most `max_chars` of text, and every segment keeps its `[mm:ss]` marker
/// so citations point back into the recording timeline.
pub fn group_segments(
    segments: &[TranscriptionSegment],
    max_chars: usize,
) -> Vec<TranscriptionSegment> {
    let mut chunks: Vec<TranscriptionSegment> = Vec::new();
    let mut buf_start = 0u64;
    let mut buf_end = 0u64;
    let mut buf = String::new();
    for seg in segments {
        let line = format!("{} {}", format_timestamp_ms(seg.start_ms), seg.text);
        if !buf.is_empty() && buf.len() + line.len() + 1 > max_chars {
            chunks.push(TranscriptionSegment {
                start_ms: buf_start,
                end_ms: buf_end,
                text: std::mem::take(&mut buf),
            });
        }
        if buf.is_empty() {
            buf_start = seg.start_ms;
        }
        buf_end = seg.end_ms;
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&line);
    }
    if !buf.is_empty() {
        chunks.push(TranscriptionSegment {
            start_ms: buf_start,
            end_ms: buf_end,
            text: buf,
        });
    }
    chunks
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::tests::TempDir;
    use std::os::unix::fs::PermissionsExt;

    /// A PATH directory containing a fake `ffmpeg` file wins over known dirs.
    #[test]
    fn resolve_ffmpeg_prefers_path_entries() {
        let dir = TempDir::new_with_label("ffmpeg-path");
        let exe = dir.path().join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        let found = resolve_ffmpeg_in(
            dir.path().as_os_str(),
            &["/definitely/not/here"],
        )
        .expect("PATH entry must be found");
        assert_eq!(found, exe);
    }

    /// Empty PATH entries are skipped (they would otherwise match CWD).
    #[test]
    fn resolve_ffmpeg_falls_back_to_known_dirs_and_skips_empty() {
        let dir = TempDir::new_with_label("ffmpeg-known");
        let exe = dir.path().join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        let known = dir.path().to_string_lossy().into_owned();
        let found = resolve_ffmpeg_in(std::ffi::OsStr::new(""), &[known.as_str()])
            .expect("known dir must be found");
        assert_eq!(found, exe);
    }

    /// Nothing anywhere → `None` (callers then show install guidance).
    #[test]
    fn resolve_ffmpeg_returns_none_when_absent() {
        let found = resolve_ffmpeg_in(
            std::ffi::OsStr::new("/definitely/not/here"),
            &["/also/not/here"],
        );
        assert!(found.is_none(), "unexpectedly found {found:?}");
    }

    /// Garbage input yields `None` whether ffprobe exists (parse fails) or
    /// not (tool absent) — never a panic and never a made-up duration.
    #[test]
    fn ffprobe_duration_is_none_for_garbage_input() {
        let dir = TempDir::new_with_label("ffprobe");
        let wav = dir.path().join("x.wav");
        std::fs::write(&wav, b"not audio").unwrap();
        assert!(ffprobe_duration_ms(&wav).is_none());
    }

    /// 44-byte canonical WAV header + a little PCM data.
    fn wav_bytes(sample_rate: u32, channels: u16) -> Vec<u8> {
        let bits = 16u16;
        let block_align = channels * (bits / 8);
        let byte_rate = sample_rate * block_align as u32;
        let data: [u8; 32] = [0; 32];
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data.len() as u32).to_le_bytes()));
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&sample_rate.to_le_bytes());
        v.extend_from_slice(&byte_rate.to_le_bytes());
        v.extend_from_slice(&block_align.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&data);
        v
    }

    fn write_wav(dir: &Path, name: &str, rate: u32, ch: u16) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, wav_bytes(rate, ch)).unwrap();
        p
    }

    /// Write an executable fake `whisper-cli` that emulates `-oj -of <base>`:
    /// it scans argv for the value after `-of` and writes `<base>.json`.
    fn write_fake_cli(
        dir: &Path,
        json_body: &str,
        exit_code: i32,
    ) -> PathBuf {
        let script = dir.join("fake-whisper-cli.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nbase=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-of\" ]; then base=\"$a\"; fi\n  prev=\"$a\"\ndone\nprintf '%s' '{json_body}' > \"$base.json\"\nexit {exit_code}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        script
    }

    fn provider(dir: &Path, cli: &Path, convert: bool) -> WhisperCppProvider {
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let model = dir.join("ggml-fake.bin");
        std::fs::write(&model, b"fake").unwrap();
        WhisperCppProvider::new(
            cli,
            model,
            "auto".into(),
            convert,
            work,
        )
    }

    const CANNED_JSON: &str = r#"{
        "transcription": [
            {"offsets": {"from": 0, "to": 4200}, "text": " Welcome to the seminar."},
            {"offsets": {"from": 4200, "to": 9100}, "text": " Today we discuss coastal erosion."}
        ],
        "result": {"language": "en"}
    }"#;

    #[test]
    fn wav_sniff_accepts_16k_mono() {
        let dir = TempDir::new_with_label("stt");
        let p = write_wav(dir.path(), "ok.wav", 16_000, 1);
        assert!(sniff_wav(&p).is_ok());
    }

    #[test]
    fn wav_sniff_reports_concrete_mismatches() {
        let dir = TempDir::new_with_label("stt");
        let rate = write_wav(dir.path(), "rate.wav", 44_100, 1);
        let err = sniff_wav(&rate).unwrap_err();
        assert!(err.contains("44100"), "unexpected: {err}");

        let chans = write_wav(dir.path(), "stereo.wav", 16_000, 2);
        let err = sniff_wav(&chans).unwrap_err();
        assert!(err.contains("channels"), "unexpected: {err}");

        let not_wav = dir.path().join("nope.mp3");
        std::fs::write(&not_wav, b"ID3 not audio at all").unwrap();
        let err = sniff_wav(&not_wav).unwrap_err();
        assert!(err.contains("RIFF"), "unexpected: {err}");
    }

    #[test]
    fn parses_whisper_json() {
        let r = parse_whisper_json(CANNED_JSON).unwrap();
        assert_eq!(r.segments.len(), 2);
        assert_eq!(r.segments[0].text, "Welcome to the seminar.");
        assert_eq!(r.segments[1].start_ms, 4200);
        assert_eq!(r.language.as_deref(), Some("en"));
        assert_eq!(r.duration_ms, 9100);
    }

    #[test]
    fn fake_whisper_end_to_end() {
        let dir = TempDir::new_with_label("stt");
        let cli = write_fake_cli(dir.path(), CANNED_JSON, 0);
        let prov = provider(dir.path(), &cli, false);
        let wav = write_wav(dir.path(), "lecture.wav", 16_000, 1);

        let r = prov.transcribe(&wav).unwrap();
        assert_eq!(r.segments.len(), 2);
        assert_eq!(r.duration_ms, 9100);
    }

    #[test]
    fn whisper_failure_is_actionable() {
        let dir = TempDir::new_with_label("stt");
        let cli = write_fake_cli(dir.path(), "{}", 3);
        // stdout/stderr: make the fake print an error first.
        std::fs::write(
            &cli,
            "#!/bin/sh\necho \"error: failed to open model\" >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let prov = provider(dir.path(), &cli, false);
        let wav = write_wav(dir.path(), "lecture.wav", 16_000, 1);

        let err = prov.transcribe(&wav).unwrap_err().to_string();
        assert!(err.contains("exit status 3"), "unexpected: {err}");
        assert!(err.contains("failed to open model"), "unexpected: {err}");
    }

    #[test]
    fn non_wav_without_conversion_is_rejected_with_guidance() {
        let dir = TempDir::new_with_label("stt");
        let cli = write_fake_cli(dir.path(), CANNED_JSON, 0);
        let prov = provider(dir.path(), &cli, false);
        let mp3 = dir.path().join("lecture.mp3");
        std::fs::write(&mp3, b"fake mpeg frames").unwrap();

        let err = prov.transcribe(&mp3).unwrap_err().to_string();
        assert!(err.contains("ffmpeg"), "unexpected: {err}");
    }

    #[test]
    fn missing_tools_reported_by_check() {
        let dir = TempDir::new_with_label("stt");
        let prov = WhisperCppProvider::new(
            dir.path().join("nope-cli"),
            dir.path().join("nope-model.bin"),
            "auto".into(),
            true,
            dir.path().join("work"),
        );
        let st = prov.check();
        assert!(!st.cli_found);
        assert!(!st.model_found);
        // ffmpeg may or may not exist on the dev machine; don't assert it.
    }

    #[test]
    fn timestamps_and_grouping() {
        assert_eq!(format_timestamp_ms(0), "[00:00]");
        assert_eq!(format_timestamp_ms(65_000), "[01:05]");
        assert_eq!(format_timestamp_ms(3_725_000), "[62:05]");

        let segs: Vec<TranscriptionSegment> = (0..40)
            .map(|i| TranscriptionSegment {
                start_ms: i * 1000,
                end_ms: i * 1000 + 900,
                text: "word ".repeat(60).trim().to_string(), // ~300 chars
            })
            .collect();
        let chunks = group_segments(&segs, 1000);
        // ~1000-char budget ⇒ roughly 3–4 segments per chunk, not 40.
        assert!(chunks.len() > 10 && chunks.len() < 20, "got {}", chunks.len());
        assert!(chunks[0].text.starts_with("[00:00] "));
        // Chunk spans are contiguous with their first/last segments.
        assert_eq!(chunks[0].start_ms, 0);
        assert_eq!(chunks.last().unwrap().end_ms, 39 * 1000 + 900);
    }
}
