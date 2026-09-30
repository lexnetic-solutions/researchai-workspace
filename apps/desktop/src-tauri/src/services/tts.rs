//! Text-to-speech (Phase 8, spec §21, §47.7) behind the neutral
//! `TextToSpeechProvider` interface.
//!
//! Piper (spec default) renders WAV files locally from a user-installed
//! binary + `.onnx` voice model; the macOS `say` provider is a cfg-gated
//! convenience fallback. Same philosophy as whisper.cpp (Phase 7): the app
//! never bundles or downloads binaries/models, and every provider runs as a
//! one-shot subprocess — nothing to load or unload.
//!
//! Long scripts (full-document read-aloud, 20-minute summaries) are
//! synthesized in **chunks**: the text is packed into sentence-boundary
//! pieces of at most [`MAX_CHUNK_WORDS`] words and each piece runs through
//! its own one-shot subprocess; the part WAVs are then concatenated into a
//! single exact-size RIFF file. Piper holds the whole utterance in memory
//! and slows down badly on very long inputs, and chunking localizes
//! failures ("chunk 2 of 3" instead of all-or-nothing) while keeping peak
//! memory flat. Short scripts take the single-invocation fast path.
//!
//! Optional MP3 export (`-b:a 64k` mono) goes through ffmpeg when enabled and
//! available; the returned [`TtsAudio`] reports exactly what was produced.

use std::io::Write as _;
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

impl TtsAudio {
    /// Describe a rendered WAV file from its exact RIFF header.
    pub fn from_wav(path: &Path) -> AppResult<Self> {
        let bytes = std::fs::metadata(path)?.len();
        let dur = wav_duration_ms_from_header(path)?.unwrap_or_else(|| wav_duration_ms(bytes));
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            format: "wav".into(),
            bytes,
            duration_ms: dur,
        })
    }
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

/// Maximum words per synthesis chunk (~1 minute of speech at normal rate).
pub(crate) const MAX_CHUNK_WORDS: usize = 220;

/// Provider-agnostic speech synthesis interface (spec §47.7). Implemented by
/// [`PiperProvider`] (spec default) and, on macOS, [`MacOsSayProvider`].
pub trait TextToSpeechProvider: Send + Sync {
    /// Render `text` (already narration-shaped) to an audio file.
    /// Blocking; call from a worker thread.
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio>;
    /// Cheap availability probe for the Settings UI.
    fn check(&self) -> TtsStatus;
    /// True when the last failure came from [`render_chunked`] (part files
    /// remain on disk for debugging); false for single-shot failures, where
    /// the partial output is removed before returning.
    fn is_chunk_error(&self) -> bool {
        false
    }
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
    /// Set when a chunked run failed partway (part files remain on disk).
    /// `Arc` keeps the derived `Clone` (clones share the flag, which matches
    /// the observable behaviour of a cloned provider chain).
    chunked_failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            chunked_failure: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    /// Synthesize one chunk through a one-shot piper process, then
    /// optionally transcode to MP3.
    fn render_internal(&self, text: &str, name_hint: &str, chunk_index: usize, chunk_total: usize) -> AppResult<TtsAudio> {
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

        let suffix = if chunk_total > 1 { format!("-part{chunk_index:02}") } else { String::new() };
        let wav = self.out_dir.join(format!("tts-{name_hint}{suffix}.wav"));
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
                "Piper failed on chunk {chunk_index} of {chunk_total} (exit status {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { "no error output" } else { stderr }
            )));
        }

        let wav_bytes = std::fs::metadata(&wav)?.len();
        if chunk_total == 1 && self.mp3_enabled && crate::services::transcription::ffmpeg_available() {
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
            duration_ms: wav_duration_ms_from_header(&wav)?.unwrap_or_else(|| wav_duration_ms(wav_bytes)),
        })
    }
}

impl TextToSpeechProvider for PiperProvider {
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        run_chunked(self, text, name_hint)
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

    fn is_chunk_error(&self) -> bool {
        self.chunked_failure
            .load(std::sync::atomic::Ordering::Relaxed)
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
    /// Set when a chunked run failed partway (part files remain on disk).
    /// `Arc` keeps the derived `Clone` (clones share the flag).
    chunked_failure: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            chunked_failure: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    /// Synthesize one chunk through a one-shot `say` process.
    fn render_internal(&self, text: &str, name_hint: &str, chunk_index: usize, chunk_total: usize) -> AppResult<TtsAudio> {
        std::fs::create_dir_all(&self.out_dir)?;
        let suffix = if chunk_total > 1 { format!("-part{chunk_index:02}") } else { String::new() };
        let wav = self.out_dir.join(format!("tts-{name_hint}{suffix}.wav"));
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
                "macOS say failed on chunk {chunk_index} of {chunk_total} (exit status {}): {}",
                output.status.code().unwrap_or(-1),
                if stderr.is_empty() { "no error output" } else { stderr }
            )));
        }

        let bytes = std::fs::metadata(&wav)?.len();
        if chunk_total == 1 && self.mp3_enabled && crate::services::transcription::ffmpeg_available() {
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
            duration_ms: wav_duration_ms_from_header(&wav)?.unwrap_or_else(|| wav_duration_ms(bytes)),
        })
    }
}

impl TextToSpeechProvider for MacOsSayProvider {
    fn render(&self, text: &str, name_hint: &str) -> AppResult<TtsAudio> {
        run_chunked(self, text, name_hint)
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

    fn is_chunk_error(&self) -> bool {
        self.chunked_failure
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// A `fmt ` chunk, parsed enough to concatenate safely.
struct WavFormat {
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

/// Read the RIFF header far enough to confirm both files are same-format
/// PCM WAV. Returns `Ok(None)` for anything non-strict (fuzzy duration
/// estimation stays available downstream).
fn read_wav_format(path: &Path) -> std::io::Result<Option<WavFormat>> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path)?;
    let u16le = |r: &mut std::fs::File| -> std::io::Result<u16> {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        Ok(u16::from_le_bytes(b))
    };
    let u32le = |r: &mut std::fs::File| -> std::io::Result<u32> {
        let mut b = [0u8; 4];
        r.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    };
    let mut tag = [0u8; 4];
    f.read_exact(&mut tag)?;
    if &tag != b"RIFF" {
        return Ok(None);
    }
    let _riff_len = u32le(&mut f)?;
    f.read_exact(&mut tag)?;
    if &tag != b"WAVE" {
        return Ok(None);
    }
    loop {
        f.read_exact(&mut tag)?;
        let len = u32le(&mut f)?;
        match &tag {
            b"fmt " => {
                let audio_format = u16le(&mut f)?;
                let channels = u16le(&mut f)?;
                let sample_rate = u32le(&mut f)?;
                let _byte_rate = u32le(&mut f)?;
                let _align = u16le(&mut f)?;
                let bits = u16le(&mut f)?;
                // 1 = integer PCM, 3 = IEEE float (macOS `say --data-format=LEF32`
                // writes tag 3); both concatenate losslessly sample-wise.
                if (audio_format != 1 && audio_format != 3)
                    || channels == 0
                    || sample_rate == 0
                    || bits == 0
                {
                    return Ok(None);
                }
                let skip = (len as usize).saturating_sub(16);
                if skip > 0 {
                    let mut junk = vec![0u8; skip];
                    f.read_exact(&mut junk)?;
                }
                return Ok(Some(WavFormat {
                    channels,
                    sample_rate,
                    bits_per_sample: bits,
                }));
            }
            _ => {
                // Skip any chunk that appears before `fmt ` (odd sizes are
                // word-aligned in RIFF).
                let skip = i64::from(len) + i64::from(len & 1);
                f.seek(std::io::SeekFrom::Current(skip))?;
            }
        }
    }
}

/// Estimate spoken duration from WAV payload size (16-bit mono @ 22.05 kHz).
/// Good enough for a UI label; exact timing comes from the RIFF header when
/// [`wav_duration_ms_from_header`] can parse it.
fn wav_duration_ms(bytes: u64) -> u64 {
    const BYTES_PER_MS: u64 = 22_050 * 2 / 1000; // sr · 16-bit · 1ch
    bytes.saturating_div(BYTES_PER_MS)
}

/// Exact spoken duration from the RIFF header (`data` chunk size divided by
/// the byte rate). Returns `None` when the header cannot be parsed strictly.
pub(crate) fn wav_duration_ms_from_header(path: &Path) -> AppResult<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let mut tag = [0u8; 4];
    f.read_exact(&mut tag)?;
    if &tag != b"RIFF" {
        return Ok(None);
    }
    let mut b4 = [0u8; 4];
    f.read_exact(&mut b4)?;
    f.read_exact(&mut tag)?;
    if &tag != b"WAVE" {
        return Ok(None);
    }
    let mut byte_rate = 0u32;
    loop {
        if f.read_exact(&mut tag).is_err() {
            return Ok(None); // truncated header: fall back to the estimate
        }
        if f.read_exact(&mut b4).is_err() {
            return Ok(None);
        }
        let len = u32::from_le_bytes(b4);
        match &tag {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                if len < 16 || f.read_exact(&mut fmt).is_err() {
                    return Ok(None);
                }
                byte_rate = u32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]);
                let skip = (len as usize - 16).min(4096);
                let mut junk = vec![0u8; skip];
                if f.read_exact(&mut junk).is_err() {
                    return Ok(None);
                }
                if len as usize - 16 > skip {
                    return Ok(None);
                }
            }
            b"data" => {
                if byte_rate > 0 && len > 0 {
                    return Ok(Some(u64::from(len) * 1000 / u64::from(byte_rate)));
                }
                break; // data before fmt: keep scanning for fmt
            }
            _ => {
                let skip = i64::from(len) + i64::from(len & 1);
                if f.seek(SeekFrom::Current(skip)).is_err() {
                    return Ok(None);
                }
            }
        }
    }
    Ok(None)
}

/// Best-effort scratch-file removal (used by both providers + chunking).
fn remove_file_best_effort(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Pack `text` into chunk boundaries of at most [`MAX_CHUNK_WORDS`] words,
/// cutting only at sentence ends (`. `, `? `, `! ` or end of text). A single
/// pathological run without sentence ends is emitted as one (over-length)
/// chunk rather than split mid-sentence.
pub(crate) fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut words = 0usize;
    for sentence in split_sentences(text) {
        let w = sentence.split_whitespace().count().max(1);
        if !current.is_empty() && words + w > MAX_CHUNK_WORDS {
            chunks.push(std::mem::take(&mut current));
            words = 0;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
        words += w;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Split into sentences ending in `.`, `!` or `?` (delimiter kept), falling
/// back to the remaining text as one final sentence.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        if matches!(c, '.' | '!' | '?') {
            let after = text[i + c.len_utf8()..].chars().next();
            let end_of_sentence = match after {
                None => true,
                Some(' ') | Some('\t') | Some('\n') | Some('\r') => true,
                _ => false,
            };
            if end_of_sentence {
                let s = text[start..i + c.len_utf8()].trim();
                if !s.is_empty() {
                    sentences.push(s.to_string());
                }
                start = i + c.len_utf8();
            }
        }
    }
    if start < text.len() {
        let s = text[start..].trim();
        if !s.is_empty() {
            sentences.push(s.to_string());
        }
    }
    sentences
}

/// Chunked orchestration shared by both providers: split long scripts at
/// sentence boundaries, render each piece via one subprocess invocation,
/// then join the part WAVs into a single exact-size RIFF file. Short
/// scripts take the provider's single-invocation fast path.
fn run_chunked(
    p: &dyn ChunkedRenderInternal,
    text: &str,
    name_hint: &str,
) -> AppResult<TtsAudio> {
    let chunks = chunk_text(text);
    if chunks.len() <= 1 {
        return p.render_internal(
            chunks.first().map(String::as_str).unwrap_or(text),
            name_hint,
            1,
            1,
        );
    }

    let total = chunks.len();
    let mut parts: Vec<PathBuf> = Vec::with_capacity(total);
    let result = (1..=total).try_for_each(|i| {
        match p.render_internal(&chunks[i - 1], name_hint, i, total) {
            Ok(audio) => {
                parts.push(PathBuf::from(&audio.path));
                Ok(())
            }
            Err(e) => Err(e),
        }
    });

    if let Err(e) = result {
        log::warn!(
            target: "researchai::tts",
            "chunked synthesis stopped after {}/{} parts: {e}",
            parts.len(),
            total
        );
        p.set_chunk_failed();
        for part in &parts {
            remove_file_best_effort(part);
        }
        return Err(e);
    }

    // One ffmpeg transcode for the whole narration, when enabled + possible.
    if p.mp3_enabled() && crate::services::transcription::ffmpeg_available() {
        match concat_wavs(&parts, &p.out_dir(), name_hint) {
            Ok(wav) => match transcode_to_mp3(&wav, &p.out_dir()) {
                Ok(mp3) => {
                    let bytes = std::fs::metadata(&mp3)?.len();
                    let duration =
                        wav_duration_ms_from_header(&wav)?.unwrap_or_else(|| {
                            wav_duration_ms(std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0))
                        });
                    parts.iter().for_each(|f| remove_file_best_effort(f));
                    remove_file_best_effort(&wav);
                    return Ok(TtsAudio {
                        path: mp3.to_string_lossy().into_owned(),
                        format: "mp3".into(),
                        bytes,
                        duration_ms: duration,
                    });
                }
                Err(e) => {
                    log::warn!(target: "researchai::tts", "mp3 transcode failed, keeping WAV: {e}");
                }
            },
            Err(e) => {
                log::warn!(target: "researchai::tts", "WAV concat failed: {e}");
            }
        }
    }

    // MP3 unavailable or the transcode failed: keep the concatenated WAV.
    let final_wav = concat_wavs(&parts, &p.out_dir(), name_hint)?;
    parts.iter().for_each(|f| remove_file_best_effort(f));
    TtsAudio::from_wav(&final_wav)
}

/// Object-safe view of the per-provider bits [`run_chunked`] needs.
pub(crate) trait ChunkedRenderInternal {
    /// Flag a partway failure so [`TextToSpeechProvider::is_chunk_error`]
    /// can report it (part files remain on disk for debugging).
    fn set_chunk_failed(&self);
    fn render_internal(
        &self,
        text: &str,
        name_hint: &str,
        chunk_index: usize,
        chunk_total: usize,
    ) -> AppResult<TtsAudio>;
    fn mp3_enabled(&self) -> bool;
    fn out_dir(&self) -> &Path;
}

impl ChunkedRenderInternal for PiperProvider {
    fn render_internal(
        &self,
        text: &str,
        name_hint: &str,
        chunk_index: usize,
        chunk_total: usize,
    ) -> AppResult<TtsAudio> {
        PiperProvider::render_internal(self, text, name_hint, chunk_index, chunk_total)
    }
    fn set_chunk_failed(&self) {
        self.chunked_failure
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn mp3_enabled(&self) -> bool {
        self.mp3_enabled
    }
    fn out_dir(&self) -> &Path {
        &self.out_dir
    }
}

impl ChunkedRenderInternal for MacOsSayProvider {
    fn render_internal(
        &self,
        text: &str,
        name_hint: &str,
        chunk_index: usize,
        chunk_total: usize,
    ) -> AppResult<TtsAudio> {
        MacOsSayProvider::render_internal(self, text, name_hint, chunk_index, chunk_total)
    }
    fn set_chunk_failed(&self) {
        self.chunked_failure
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn mp3_enabled(&self) -> bool {
        self.mp3_enabled
    }
    fn out_dir(&self) -> &Path {
        &self.out_dir
    }
}

/// Concatenate same-format PCM WAV part files into one exact-size RIFF
/// (header rewritten with true final sizes; part headers stripped).
pub(crate) fn concat_wavs(parts: &[PathBuf], out_dir: &Path, name_hint: &str) -> AppResult<PathBuf> {
    let first = parts.first().ok_or_else(|| AppError::msg("No audio parts to concatenate."))?;
    let fmt = read_wav_format(first)
        .map_err(|e| AppError::msg(format!("Cannot read {}: {e}", first.display())))?
        .ok_or_else(|| AppError::msg(format!(
            "{} is not a strictly parseable PCM WAV file.",
            first.display()
        )))?;

    let bytes_per_frame = u64::from(fmt.channels) * u64::from(fmt.bits_per_sample / 8);
    let mut data_len: u64 = 0;
    let mut payload: Vec<(PathBuf, u64)> = Vec::with_capacity(parts.len());
    for part in parts {
        let size = std::fs::metadata(part)
            .map_err(|e| AppError::msg(format!("Cannot stat {}: {e}", part.display())))?
            .len();
        let header = wav_data_offset(part)
            .map_err(|e| AppError::msg(format!("Cannot read {}: {e}", part.display())))?
            .ok_or_else(|| AppError::msg(format!(
                "{} is not a strictly parseable PCM WAV file.",
                part.display()
            )))?;
        let plen = size.saturating_sub(header);
        if plen % bytes_per_frame != 0 {
            return Err(AppError::msg(format!(
                "{} has a partial PCM frame ({} bytes) — refusing to concatenate.",
                part.display(),
                plen % bytes_per_frame
            )));
        }
        data_len += plen;
        payload.push((part.clone(), header));
    }

    let byte_rate = u64::from(fmt.sample_rate) * bytes_per_frame;
    // RIFF stores little-endian u32 sizes; refuse beyond the 4 GiB WAV cap.
    let riff_len = u32::try_from(36 + data_len)
        .map_err(|_| AppError::msg("Concatenated audio exceeds the 4 GiB WAV limit."))?;
    let data_len32 = u32::try_from(data_len)
        .map_err(|_| AppError::msg("Concatenated audio exceeds the 4 GiB WAV limit."))?;
    let out = out_dir.join(format!("tts-{name_hint}.wav"));
    remove_file_best_effort(&out);
    let mut w = std::fs::File::create(&out)?;
    w.write_all(b"RIFF")?;
    w.write_all(&riff_len.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // integer PCM output
    w.write_all(&fmt.channels.to_le_bytes())?;
    w.write_all(&fmt.sample_rate.to_le_bytes())?;
    w.write_all(&(byte_rate as u32).to_le_bytes())?;
    w.write_all(&(bytes_per_frame as u16).to_le_bytes())?;
    w.write_all(&fmt.bits_per_sample.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len32.to_le_bytes())?;
    for (part, offset) in &payload {
        let mut r = std::fs::File::open(part)?;
        use std::io::Seek;
        r.seek(std::io::SeekFrom::Start(*offset))?;
        std::io::copy(&mut r, &mut w)?;
    }
    w.flush()?;
    Ok(out)
}

/// Byte offset of the `data` chunk payload (strict PCM parse; `None` when
/// the header cannot be walked deterministically).
fn wav_data_offset(path: &Path) -> std::io::Result<Option<u64>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let mut tag = [0u8; 4];
    f.read_exact(&mut tag)?;
    if &tag != b"RIFF" {
        return Ok(None);
    }
    let mut b4 = [0u8; 4];
    f.read_exact(&mut b4)?;
    f.read_exact(&mut tag)?;
    if &tag != b"WAVE" {
        return Ok(None);
    }
    loop {
        if f.read_exact(&mut tag).is_err() || f.read_exact(&mut b4).is_err() {
            return Ok(None);
        }
        let len = u32::from_le_bytes(b4);
        if &tag == b"data" {
            let pos = f.stream_position()?;
            return Ok(Some(pos));
        }
        let skip = i64::from(len) + i64::from(len & 1);
        if f.seek(SeekFrom::Current(skip)).is_err() {
            return Ok(None);
        }
    }
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

    /// Minimal valid PCM WAV (24 kHz, mono, 16-bit) with `samples` frames.
    fn write_minimal_wav(dir: &Path, name: &str, samples: usize) -> PathBuf {
        let data: Vec<u8> = vec![0u8; samples * 2];
        let byte_rate: u32 = 24_000 * 2; // mono 16-bit
        let mut bytes: Vec<u8> = b"RIFF".to_vec();
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    /// Write an executable fake `piper` that reads stdin, writes a small
    /// but *valid* 24 kHz mono 16-bit PCM WAV (400-byte payload) and exits
    /// with the given code (POSIX sh loop for the `--output_file <path>`
    /// value — no bashisms).
    fn write_fake_piper(dir: &Path, exit_code: i32) -> PathBuf {
        let script = dir.join("fake-piper.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output_file\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\ncat > /dev/null\nprintf 'RIFF' > \"$out\"\nprintf '\\x90\\x01\\x00\\x00' >> \"$out\"\nprintf 'WAVE' >> \"$out\"\nprintf 'fmt ' >> \"$out\"\nprintf '\\x10\\x00\\x00\\x00' >> \"$out\"\nprintf '\\x01\\x00' >> \"$out\"\nprintf '\\x01\\x00' >> \"$out\"\nprintf '\\xc0\\x5d\\x00\\x00' >> \"$out\"\nprintf '\\x80\\xbb\\x00\\x00' >> \"$out\"\nprintf '\\x02\\x00' >> \"$out\"\nprintf '\\x10\\x00' >> \"$out\"\nprintf 'data' >> \"$out\"\nprintf '\\x90\\x01\\x00\\x00' >> \"$out\"\nhead -c 400 /dev/zero >> \"$out\"\nexit {exit_code}\n"
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
        // Exact duration from the fake's RIFF header: 400-byte payload at
        // 48 000 B/s = 8 ms (the size-based estimate would say ~9 ms).
        assert_eq!(audio.duration_ms, 8);
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
    fn chunk_text_packs_sentences_under_the_limit() {
        let sentence = "The coastal review continues here. ";
        let text = sentence.repeat(200); // 5 words × 200 = 1000 words
        let chunks = chunk_text(&text);
        assert!(chunks.len() >= 4, "expected several chunks, got {}", chunks.len());
        for c in &chunks {
            let words = c.split_whitespace().count();
            assert!(words <= MAX_CHUNK_WORDS, "chunk has {words} words");
            assert!(c.ends_with(". ") || c.ends_with('.') || c.trim_end().ends_with('.'), "{c}");
        }
        // Nothing lost: same word count after re-chunking.
        let total: usize = chunks.iter().map(|c| c.split_whitespace().count()).sum();
        assert_eq!(total, text.split_whitespace().count());
    }

    #[test]
    fn short_text_stays_a_single_chunk() {
        assert_eq!(chunk_text("One sentence only.").len(), 1);
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn multi_chunk_render_joins_parts_into_one_wav() {
        let dir = TempDir::new_with_label("tts");
        let cli = write_fake_piper(dir.path(), 0);
        let prov = piper(dir.path(), false).with_piper_binary(&cli);

        // 3 words × 240 = 720 words > MAX_CHUNK_WORDS, forcing ≥4 piper runs.
        let sentence = "Chunked narration sentence. ";
        let text = sentence.repeat(240);
        let audio = prov.render(&text, "multi").unwrap();

        assert_eq!(audio.format, "wav");
        assert!(audio.path.ends_with("tts-multi.wav"), "{}", audio.path);
        // Exact-size RIFF: declared data length matches the file tail.
        let raw = std::fs::read(&audio.path).unwrap();
        let data_len = u32::from_le_bytes([raw[40], raw[41], raw[42], raw[43]]) as usize;
        assert_eq!(raw.len(), 44 + data_len, "RIFF sizes must be exact");
        // Per-part payloads summed = final payload (the fake piper writes a
        // 400-byte payload per invocation).
        let parts = (raw.len() - 44) / 400;
        assert!(parts >= 3, "expected ≥3 part payloads, got {parts}");
        // Part files are cleaned up.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("out"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("-part"))
            .collect();
        assert!(leftovers.is_empty(), "leftover parts: {leftovers:?}");
    }

    #[test]
    fn mid_chunk_failure_reports_chunk_position() {
        let dir = TempDir::new_with_label("tts");
        let cli = write_fake_piper(dir.path(), 3);
        let prov = piper(dir.path(), false).with_piper_binary(&cli);

        let text = "Fail on every chunk. ".repeat(120);
        let err = prov.render(&text, "boom").unwrap_err().to_string();
        assert!(err.contains("chunk 1 of"), "{err}");
        assert!(prov.is_chunk_error(), "chunked failure must be flagged");
    }

    #[test]
    fn concat_wavs_builds_exact_24k_header() {
        let dir = TempDir::new_with_label("tts");
        let a = write_minimal_wav(dir.path(), "a.wav", 240); // 10 ms each
        let b = write_minimal_wav(dir.path(), "b.wav", 480); // 20 ms
        let out_dir = dir.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        let out = concat_wavs(&[a.clone(), b], &out_dir, "cat").unwrap();
        let raw = std::fs::read(&out).unwrap();
        assert_eq!(&raw[0..4], b"RIFF");
        assert_eq!(&raw[8..12], b"WAVE");
        let data_len = u32::from_le_bytes([raw[40], raw[41], raw[42], raw[43]]) as usize;
        assert_eq!(data_len, 720 * 2);
        assert_eq!(raw.len(), 44 + data_len);
        // fmt fields: mono, 24 kHz, 48 kB/s, block align 2, 16 bits.
        assert_eq!(&raw[36..40], b"data");
        let channels = u16::from_le_bytes([raw[22], raw[23]]);
        let rate = u32::from_le_bytes([raw[24], raw[25], raw[26], raw[27]]);
        assert_eq!((channels, rate), (1, 24_000));
        // Duration comes out exactly from the header.
        let ms = wav_duration_ms_from_header(&out).unwrap().unwrap();
        assert_eq!(ms, 30);
    }

    #[test]
    fn wav_duration_falls_back_for_nonstrict_headers() {
        let dir = TempDir::new_with_label("tts");
        let path = dir.path().join("junk.wav");
        std::fs::write(&path, b"not a wav at all").unwrap();
        assert_eq!(wav_duration_ms_from_header(&path).unwrap(), None);
    }

    #[test]
    fn say_provider_renders_via_fake_binary() {
        let dir = TempDir::new_with_label("tts");
        let script = dir.path().join("fake-say.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n# emulate macOS say: -o <file>; speak text is last arg\nprev=\"\"\nout=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\nprintf 'RIFF' > \"$out\"\nprintf '\\x4c\\x12\\x00\\x00' >> \"$out\"\nprintf 'WAVE' >> \"$out\"\nprintf 'fmt ' >> \"$out\"\nprintf '\\x10\\x00\\x00\\x00' >> \"$out\"\nprintf '\\x01\\x00' >> \"$out\"\nprintf '\\x01\\x00' >> \"$out\"\nprintf '\\xc0\\x5d\\x00\\x00' >> \"$out\"\nprintf '\\x80\\xbb\\x00\\x00' >> \"$out\"\nprintf '\\x02\\x00' >> \"$out\"\nprintf '\\x10\\x00' >> \"$out\"\nprintf 'data' >> \"$out\"\nprintf '\\x80\\xbb\\x00\\x00' >> \"$out\"\nhead -c 4680 /dev/zero >> \"$out\"\nexit 0\n",
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
        assert_eq!(audio.bytes, on_disk);
        // Exact duration from the strict RIFF header the fake writes:
        // 48 000-byte payload at 48 000 B/s = 1000 ms.
        assert_eq!(audio.duration_ms, 1000);
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
