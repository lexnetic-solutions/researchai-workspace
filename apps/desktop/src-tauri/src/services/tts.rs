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

/// Bump whenever chunk boundaries change (splitter or ceiling tweaks) so
/// cached part files rendered under an older chunker can never be spliced
/// into a newer one — manifest counts alone cannot prove matching
/// boundaries. Fed into [`parts_signature`].
const CHUNKER_VERSION: &str = "chunker-v2-clause-fallback";

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
            let stderr = crate::error::clip_bytes(stderr, 600);
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
            let stderr = crate::error::clip_bytes(stderr, 400);
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
    /// 1 = integer PCM, 3 = IEEE float.
    audio_format: u16,
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
                    audio_format,
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
/// cutting at sentence ends (`. `, `? `, `! ` or end of text). A single
/// pathological sentence longer than the ceiling is passed to
/// [`split_overlong_sentence`] — cut at clause boundaries or whole words,
/// never mid-word — so the ceiling holds for every input.
pub(crate) fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut words = 0usize;
    for sentence in split_sentences(text) {
        let w = sentence.split_whitespace().count().max(1);
        if w > MAX_CHUNK_WORDS {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
                words = 0;
            }
            chunks.extend(split_overlong_sentence(&sentence, MAX_CHUNK_WORDS));
            continue;
        }
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

/// Split a single sentence longer than `max_words` into pieces of at most
/// `max_words`, preferring the last clause boundary (`,`, `;`, `:`, `—`,
/// `–`) in the back half of each window, then falling back to a whole-word
/// cut. Never splits inside a word. (Clause-fallback idea adapted from
/// Voicebox's `split_text_into_chunks`; MIT.)
fn split_overlong_sentence(sentence: &str, max_words: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut words: Vec<&str> = Vec::new();
    let mut clause_at: Option<usize> = None;
    for word in sentence.split_whitespace() {
        words.push(word);
        if word.ends_with(',') || word.ends_with(';') || word.ends_with(':')
            || word.ends_with('—') || word.ends_with('–')
        {
            clause_at = Some(words.len());
        }
        if words.len() == max_words {
            let cut = clause_at
                .filter(|&c| c > max_words / 2)
                .unwrap_or(words.len());
            pieces.push(words[..cut].join(" "));
            words = words.split_off(cut);
            // Re-locate the last clause marker in the carried-over tail.
            clause_at = words
                .iter()
                .rposition(|w| {
                    w.ends_with(',') || w.ends_with(';') || w.ends_with(':')
                        || w.ends_with('—') || w.ends_with('–')
                })
                .map(|i| i + 1);
        }
    }
    if !words.is_empty() {
        pieces.push(words.join(" "));
    }
    pieces
}

/// Split into sentences ending in `.`, `!` or `?` (delimiter kept), falling
/// back to the remaining text as one final sentence. Periods that follow a
/// known abbreviation (`Dr.`, `e.g.`, `p.m.`, …), a single-letter initial
/// (`J. Smith`) or a bare number (`3.`, list markers) are not boundaries —
/// abbreviation handling adapted from Voicebox's splitter (MIT). Over-
/// merging is safe here: the splitter only packs chunks, it never drops
/// text, and [`split_overlong_sentence`] enforces the word ceiling.
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
            if end_of_sentence && c == '.' && !period_ends_sentence(text, i) {
                continue;
            }
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

/// Abbreviations (lowercase, inner dots kept) whose trailing period never
/// ends a sentence. Includes the academic favourites — `et al.` matches via
/// its `al` tail, `e.g.`/`i.e.` via their dotted forms.
const SENTENCE_ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "etc",
    "inc", "ltd", "corp", "dept", "est", "approx", "no", "fig", "al",
    "cf", "ed", "vol", "pp", "eq", "ref", "ch", "sec", "op", "ibid",
    "e.g", "i.e", "a.m", "p.m", "u.s", "ph.d",
];

/// Why the `.` at byte `i` is (not) a sentence boundary: false for known
/// abbreviations, single-letter initials (`J. Smith`) and bare numbers
/// (list markers like `1.`), true otherwise. Called only for periods that
/// already sit before whitespace or end of text.
fn period_ends_sentence(text: &str, i: usize) -> bool {
    // Walk back over the alphanumeric run (plus inner dots, so `e.g.` and
    // `U.S.` surface whole); any other byte stops the word.
    let bytes = text.as_bytes();
    let mut start = i;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'.' {
            start -= 1;
        } else {
            break;
        }
    }
    let word = &text[start..i];
    if word.is_empty() {
        return true; // leading `.` — treat as a boundary, never a crash
    }
    // `J. Smith` — a lone letter is an initial, not a sentence.
    if word.len() == 1 && word.as_bytes()[0].is_ascii_alphabetic() {
        return false;
    }
    // `1. First item` / `3.5` (before whitespace) — digits don't end prose.
    if word.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    !SENTENCE_ABBREVIATIONS.contains(&word.to_ascii_lowercase().as_str())
}

/// Small on-disk manifest (`tts-parts.json`, written next to the part
/// files) that lets a re-run reuse already-rendered chunks instead of
/// re-synthesizing the whole narration. The signature guards every entry:
/// any edit to the script (or a provider switch) changes it and all cached
/// parts are ignored.
#[derive(serde::Serialize, serde::Deserialize)]
struct PartsManifest {
    /// Hex SHA-256 over script + provider identity.
    signature: String,
    /// Absolute part paths, in chunk order.
    parts: Vec<String>,
}

/// Cache signature: what is spoken, by whom, and under which chunker
/// version. Speech rate is deliberately excluded — a speed change alters
/// each subprocess run, so the cache must not be trusted across it;
/// [`CHUNKER_VERSION`] covers boundary changes the text alone cannot show.
fn parts_signature(provider_id: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(CHUNKER_VERSION.as_bytes());
    h.update([0u8]);
    h.update(provider_id.as_bytes());
    h.update([0u8]);
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

/// Load still-valid cached part paths for `sig`, dropping entries whose
/// files vanished and treating a missing/corrupt manifest as empty cache.
fn load_cached_parts(out_dir: &Path, sig: &str) -> Vec<PathBuf> {
    let manifest_path = out_dir.join("tts-parts.json");
    let Ok(raw) = std::fs::read(&manifest_path) else {
        return Vec::new();
    };
    let m: PartsManifest = match serde_json::from_slice(&raw) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    if m.signature != sig {
        return Vec::new();
    }
    m.parts
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect()
}

/// Persist the manifest (best-effort: losing the cache is never fatal).
fn save_cached_parts(out_dir: &Path, sig: &str, parts: &[PathBuf]) {
    if parts.is_empty() {
        return;
    }
    let manifest_path = out_dir.join("tts-parts.json");
    let m = PartsManifest {
        signature: sig.to_string(),
        parts: parts
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    };
    match serde_json::to_vec_pretty(&m) {
        Ok(json) => {
            if std::fs::write(&manifest_path, json).is_err() {
                log::warn!(
                    target: "researchai::tts",
                    "could not write the TTS parts manifest; resume will re-render"
                );
            }
        }
        Err(_) => {
            log::warn!(
                target: "researchai::tts",
                "could not serialize the TTS parts manifest; resume will re-render"
            );
        }
    }
}

/// Seconds between chunk retries (fixed schedule; total added delay ≈ 3 s).
const CHUNK_RETRY_DELAYS_SECS: &[u64] = &[1, 2];

/// Render one chunk with a small fixed retry schedule for transient
/// subprocess failures. The final error keeps the chunk position context.
fn render_chunk_with_retry(
    p: &dyn ChunkedRenderInternal,
    text: &str,
    name_hint: &str,
    chunk_index: usize,
    chunk_total: usize,
) -> AppResult<TtsAudio> {
    let mut last: Option<AppError> = None;
    for attempt in 0..=CHUNK_RETRY_DELAYS_SECS.len() {
        if attempt > 0 {
            log::warn!(
                target: "researchai::tts",
                "chunk {chunk_index}/{chunk_total} failed, retrying (attempt {attempt}/{})",
                CHUNK_RETRY_DELAYS_SECS.len()
            );
            std::thread::sleep(std::time::Duration::from_secs(
                CHUNK_RETRY_DELAYS_SECS[attempt - 1],
            ));
        }
        match p.render_internal(text, name_hint, chunk_index, chunk_total) {
            Ok(audio) => return Ok(audio),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("retry loop runs at least once"))
}

/// Chunked orchestration shared by both providers: split long scripts at
/// sentence boundaries, render each piece via one subprocess invocation
/// (with per-chunk retries), then join the part WAVs into a single
/// exact-size RIFF file. Rendered parts are cached in a manifest so a
/// failed run resumes at the first missing chunk. Short scripts take the
/// provider's single-invocation fast path.
fn run_chunked(
    p: &dyn ChunkedRenderInternal,
    text: &str,
    name_hint: &str,
) -> AppResult<TtsAudio> {
    // Missing tools are a deterministic error: fail fast, no retries.
    p.preflight()?;

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
    let sig = parts_signature(&p.provider_id(), text);
    let cached = load_cached_parts(p.out_dir(), &sig);
    // A manifest listing MORE parts than the current chunker produces means
    // the chunking changed since it was written: ignore it entirely.
    let mut parts = if cached.len() <= total { cached } else { Vec::new() };
    if !parts.is_empty() {
        if parts.len() == total {
            log::info!(
                target: "researchai::tts",
                "reusing {total} previously rendered parts (manifest match)"
            );
            return finish_chunked(p, &parts, name_hint);
        }
        log::info!(
            target: "researchai::tts",
            "resuming chunked synthesis at part {}/{}",
            parts.len() + 1,
            total
        );
    }
    for i in parts.len() + 1..=total {
        match render_chunk_with_retry(p, &chunks[i - 1], name_hint, i, total) {
            Ok(audio) => parts.push(PathBuf::from(&audio.path)),
            Err(e) => {
                log::warn!(
                    target: "researchai::tts",
                    "chunked synthesis stopped after {}/{} parts: {e}",
                    parts.len(),
                    total
                );
                p.set_chunk_failed();
                // Cache what rendered so a retry resumes from here; keep the
                // part files on disk (they are the cache).
                save_cached_parts(p.out_dir(), &sig, &parts);
                return Err(e);
            }
        }
    }
    finish_chunked(p, &parts, name_hint)
}

/// Join rendered parts into the final audio, clear the parts cache and
/// transcode to MP3 when enabled. Shared by the fresh and resumed paths.
fn finish_chunked(
    p: &dyn ChunkedRenderInternal,
    parts: &[PathBuf],
    name_hint: &str,
) -> AppResult<TtsAudio> {
    let out_dir = p.out_dir();
    // One ffmpeg transcode for the whole narration, when enabled + possible.
    if p.mp3_enabled() && crate::services::transcription::ffmpeg_available() {
        match concat_wavs(parts, out_dir, name_hint) {
            Ok(wav) => match transcode_to_mp3(&wav, out_dir) {
                Ok(mp3) => {
                    let bytes = std::fs::metadata(&mp3)?.len();
                    let duration = wav_duration_ms_from_header(&wav)?.unwrap_or_else(|| {
                        wav_duration_ms(std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0))
                    });
                    parts.iter().for_each(|f| remove_file_best_effort(f));
                    remove_file_best_effort(&wav);
                    remove_file_best_effort(&out_dir.join("tts-parts.json"));
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
    let final_wav = concat_wavs(parts, out_dir, name_hint)?;
    parts.iter().for_each(|f| remove_file_best_effort(f));
    remove_file_best_effort(&out_dir.join("tts-parts.json"));
    TtsAudio::from_wav(&final_wav)
}

/// Object-safe view of the per-provider bits [`run_chunked`] needs.
pub(crate) trait ChunkedRenderInternal {
    /// Flag a partway failure so [`TextToSpeechProvider::is_chunk_error`]
    /// can report it (part files remain on disk for debugging).
    fn set_chunk_failed(&self);
    /// Deterministic tooling check run once before any chunk is rendered;
    /// failing here skips retries and the parts cache entirely.
    fn preflight(&self) -> AppResult<()>;
    /// Stable identity feeding the parts-cache signature.
    fn provider_id(&self) -> String;
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
    fn preflight(&self) -> AppResult<()> {
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
        Ok(())
    }
    fn provider_id(&self) -> String {
        // The voice model is the identity; the binary path is deliberately
        // excluded so a re-installed piper can resume cached parts.
        format!("piper:{}", self.voice_model_path.display())
    }
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
    fn preflight(&self) -> AppResult<()> {
        let ok = Command::new(&self.binary)
            .arg("-v")
            .arg("?")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return Err(AppError::msg(
                "macOS say is not responding — the `say` command is built into macOS; \
                 if this persists, switch to Piper in Settings → Speech.",
            ));
        }
        Ok(())
    }
    fn provider_id(&self) -> String {
        format!("macos-say:{}", self.voice)
    }
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
        let part_fmt = read_wav_format(part)
            .map_err(|e| AppError::msg(format!("Cannot read {}: {e}", part.display())))?
            .ok_or_else(|| AppError::msg(format!(
                "{} is not a strictly parseable PCM WAV file.",
                part.display()
            )))?;
        if part_fmt.audio_format != fmt.audio_format
            || part_fmt.channels != fmt.channels
            || part_fmt.sample_rate != fmt.sample_rate
            || part_fmt.bits_per_sample != fmt.bits_per_sample
        {
            return Err(AppError::msg(format!(
                "{} does not match the first part's audio format \
                 ({} Hz, {} ch, {} bit vs {} Hz, {} ch, {} bit) — refusing to concatenate.",
                part.display(),
                part_fmt.sample_rate,
                part_fmt.channels,
                part_fmt.bits_per_sample,
                fmt.sample_rate,
                fmt.channels,
                fmt.bits_per_sample
            )));
        }
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
    w.write_all(&fmt.audio_format.to_le_bytes())?; // PCM or float, as the parts
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
pub(crate) fn transcode_to_mp3(src: &Path, out_dir: &Path) -> AppResult<PathBuf> {
    let out = out_dir.join(format!(
        "{}.mp3",
        src.file_stem().and_then(|s| s.to_str()).unwrap_or("tts")
    ));
    let _ = std::fs::remove_file(&out);
    let ff = crate::services::transcription::resolve_ffmpeg().ok_or_else(|| {
        AppError::msg(
            "ffmpeg was not found — install it (e.g. `brew install ffmpeg`) to export MP3.",
        )
    })?;
    let output = Command::new(&ff)
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
/// `resource_dir` locates the bundled Master Voice render script.
pub fn provider_for(
    s: &crate::db::TtsSettings,
    resource_dir: Option<&Path>,
    out_dir: impl Into<PathBuf>,
) -> AppResult<Box<dyn TextToSpeechProvider>> {
    match s.provider.as_str() {
        "master-voice" => Ok(Box::new(crate::services::master_voice::MasterVoiceProvider::from_settings(
            s, resource_dir, out_dir,
        ))),
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

    /// WAV bytes as the fake piper writes them: declared 400-byte data
    /// chunk at 24 kHz/48 kB/s → exactly 8 ms. Built in Rust because shell
    /// `printf '\xHH'` is not portable (dash's printf has no \x support,
    /// which produced garbage WAVs on Ubuntu CI).
    fn fake_piper_wav_bytes() -> Vec<u8> {
        let mut bytes: Vec<u8> = b"RIFF".to_vec();
        bytes.extend_from_slice(&400u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&400u32.to_le_bytes());
        bytes.extend_from_slice(&vec![0u8; 400]);
        bytes
    }

    /// WAV bytes as the fake macOS `say` writes them: declared 48 000-byte
    /// data chunk at 48 kB/s → exactly 1000 ms. Built in Rust for the same
    /// dash/printf portability reason as [`fake_piper_wav_bytes`].
    fn fake_say_wav_bytes() -> Vec<u8> {
        let mut bytes: Vec<u8> = b"RIFF".to_vec();
        bytes.extend_from_slice(&4684u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&24_000u32.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&vec![0u8; 4680]);
        bytes
    }

    /// Write an executable fake `piper` that reads stdin and copies a
    /// Rust-generated valid 24 kHz mono 16-bit PCM WAV (400-byte payload)
    /// into `--output_file <path>`, then exits with the given code (POSIX
    /// sh loop for the argument scan — no bashisms, no printf escapes).
    fn write_fake_piper(dir: &Path, exit_code: i32) -> PathBuf {
        let src = dir.join("fake-piper-payload.wav");
        std::fs::write(&src, fake_piper_wav_bytes()).unwrap();
        let src = src.display().to_string();
        let script = dir.join("fake-piper.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nout=\"\"\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output_file\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\ncat > /dev/null\ncat '{src}' > \"$out\"\nexit {exit_code}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// Fake `piper` that counts every invocation into `dir/calls.txt` and
    /// exits 5 for calls `fail_from..=fail_to` (0/0 = never fails); otherwise
    /// writes the same valid 24 kHz WAV as [`write_fake_piper`].
    fn write_counting_fake_piper(dir: &Path, fail_from: usize, fail_to: usize) -> PathBuf {
        let script = dir.join(format!("counting-piper-{fail_from}-{fail_to}.sh"));
        let counter = dir.join("calls.txt").display().to_string();
        let src = dir.join("fake-piper-payload.wav");
        std::fs::write(&src, fake_piper_wav_bytes()).unwrap();
        let src = src.display().to_string();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nout=\"\"; prev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output_file\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\nc=\"{counter}\"\nn=$(cat \"$c\" 2>/dev/null) || true\n[ -z \"$n\" ] && n=0\nn=$((n+1))\necho \"$n\" > \"$c\"\nif [ \"$n\" -ge {fail_from} ] && [ \"$n\" -le {fail_to} ]; then exit 5; fi\ncat > /dev/null\ncat '{src}' > \"$out\"\nexit 0\n"
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
    fn split_sentences_skips_abbreviations_and_initials() {
        // Titles, initials and academic suffixes never end a sentence.
        let s = split_sentences("Dr. Smith and Prof. Jones met. They talked.");
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0], "Dr. Smith and Prof. Jones met.");

        let s = split_sentences("J. Smith left. Bye.");
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].starts_with("J. Smith"), "{s:?}");

        let s = split_sentences("The work by Smith et al. showed results. Done.");
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[0].ends_with("results."), "{s:?}");

        let s = split_sentences("See Fig. 3 and p.m. readings. Done.");
        assert_eq!(s.len(), 2, "{s:?}");

        // Real boundaries still split, and `?`/`!` bypass the checks.
        let s = split_sentences("Done? Yes! Go.");
        assert_eq!(s.len(), 3, "{s:?}");

        // List markers merge (safe over-merge: text is never dropped).
        let s = split_sentences("1. First item 2. Second item");
        assert_eq!(s.len(), 1, "{s:?}");
    }

    #[test]
    fn chunk_text_holds_the_ceiling_for_pathological_sentences() {
        // One 500-word sentence with NO sentence punctuation and no commas:
        // the hard cut must land on whole words at exactly the ceiling.
        let text = (0..500).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3, "expected 220+220+60, got {}", chunks.len());
        assert_eq!(chunks[0].split_whitespace().count(), MAX_CHUNK_WORDS);
        assert_eq!(chunks[1].split_whitespace().count(), MAX_CHUNK_WORDS);
        assert_eq!(chunks[2].split_whitespace().count(), 60);
        // Nothing lost or split mid-word.
        let rejoined = chunks.join(" ");
        assert_eq!(rejoined, text);

        // With a comma in the back half of the window, prefer that clause
        // boundary over the hard cut.
        let mut words: Vec<String> = (0..300).map(|i| format!("w{i}")).collect();
        words[150] = "w150,".into();
        let text = words.join(" ");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2, "got {}", chunks.len());
        assert_eq!(
            chunks[0].split_whitespace().count(),
            151,
            "cut should prefer the comma at 150"
        );
        assert!(chunks[0].ends_with(','), "{}", chunks[0]);
        assert_eq!(chunks[1].split_whitespace().count(), 149);
        assert_eq!(chunks.join(" "), text);
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
    fn retry_and_resume_recover_a_multi_chunk_narration() {
        let dir = TempDir::new_with_label("tts-resume");
        // Calls 3-5 fail (chunk 3 exhausts its three attempts); both fakes
        // share one counter so total invocations are observable.
        let failing = write_counting_fake_piper(dir.path(), 3, 5);
        let good = write_counting_fake_piper(dir.path(), 0, 0);
        let prov_bad = piper(dir.path(), false).with_piper_binary(&failing);
        let prov_good = piper(dir.path(), false).with_piper_binary(&good);

        // 3 words × 240 = 720 words → 4 chunks.
        let text = "Resume narration sentence. ".repeat(240);
        let err = prov_bad.render(&text, "resume").unwrap_err().to_string();
        assert!(err.contains("chunk 3 of"), "{err}");
        assert!(prov_bad.is_chunk_error());
        // Chunks 1-2 rendered once each; chunk 3 burned all three attempts.
        let calls = || {
            std::fs::read_to_string(dir.path().join("calls.txt"))
                .unwrap()
                .trim()
                .to_string()
        };
        assert_eq!(calls(), "5");
        let manifest = dir.path().join("out").join("tts-parts.json");
        assert!(manifest.exists(), "failed run must cache its good parts");

        // Second run resumes: only chunks 3-4 are synthesized (calls 6-7).
        let audio = prov_good.render(&text, "resume").unwrap();
        let raw = std::fs::read(&audio.path).unwrap();
        assert_eq!(raw.len(), 44 + 4 * 400, "parts 1-2 reused, 3-4 fresh");
        assert_eq!(calls(), "7");
        // Success clears the parts cache.
        assert!(!manifest.exists());
    }

    #[test]
    fn parts_manifest_cache_is_signature_guarded() {
        let dir = TempDir::new_with_label("tts-manifest");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let p1 = write_minimal_wav(&out, "p1.wav", 100);

        // Signature covers script + provider identity (not speed).
        let sig_a = parts_signature("prov-a", "text one");
        assert_ne!(sig_a, parts_signature("prov-a", "text two"));
        assert_ne!(sig_a, parts_signature("prov-b", "text one"));

        save_cached_parts(&out, &sig_a, &[p1.clone()]);
        assert_eq!(load_cached_parts(&out, &sig_a), vec![p1.clone()]);
        assert!(load_cached_parts(&out, "other-sig").is_empty());

        // Vanished part files are dropped from the cache view.
        std::fs::remove_file(&p1).unwrap();
        assert!(load_cached_parts(&out, &sig_a).is_empty());

        // Corrupt manifest is treated as an empty cache, never an error.
        std::fs::write(out.join("tts-parts.json"), b"{not json").unwrap();
        assert!(load_cached_parts(&out, &sig_a).is_empty());
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
        let src = dir.path().join("fake-say-payload.wav");
        std::fs::write(&src, fake_say_wav_bytes()).unwrap();
        let src = src.display().to_string();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n# emulate macOS say: -o <file>; speak text is last arg\nprev=\"\"\nout=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"-o\" ]; then out=\"$a\"; fi\n  prev=\"$a\"\ndone\ncat '{src}' > \"$out\"\nexit 0\n"
            ),
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

    /// Live render through the real `say` binary — proves audio bytes are
    /// actually produced on macOS (the zero-install fallback path used when
    /// Piper is unconfigured). Skips when `say` is unavailable.
    #[cfg(target_os = "macos")]
    #[test]
    fn live_say_renders_a_real_wav() {
        let dir = TempDir::new_with_label("tts-say-live");
        let out_dir = dir.path().join("out");
        let prov = MacOsSayProvider::new(String::new(), 1.0, false, &out_dir);
        if !prov.check().ready {
            return; // `say` missing in this environment — nothing to prove
        }
        let audio = prov.render("Testing audio generation.", "live-say").unwrap();
        assert_eq!(audio.format, "wav");
        assert!(std::path::Path::new(&audio.path).exists());
        let on_disk = std::fs::metadata(&audio.path).unwrap().len();
        assert_eq!(audio.bytes, on_disk);
        // "Testing audio generation." ≈ 1 s of speech; even a very short
        // utterance must yield a parseable RIFF header with a real duration.
        assert!(audio.duration_ms > 0, "got {} ms", audio.duration_ms);
    }

    #[test]
    fn provider_for_selects_by_settings() {
        let dir = TempDir::new_with_label("tts");
        let out = dir.path().join("out");
        let piper_settings = crate::db::TtsSettings {
            provider: "piper".into(),
            ..crate::db::TtsSettings::default()
        };
        assert!(provider_for(&piper_settings, None, &out).is_ok());
        // Non-macOS cfg path returns the macOS error; on macOS it constructs.
        let say_settings = crate::db::TtsSettings {
            provider: "macos-say".into(),
            ..crate::db::TtsSettings::default()
        };
        let _ = provider_for(&say_settings, None, &out);
        // Master voice constructs on every platform (readiness is check()'s job).
        let master_settings = crate::db::TtsSettings {
            provider: "master-voice".into(),
            ..crate::db::TtsSettings::default()
        };
        let p = provider_for(&master_settings, None, &out).unwrap();
        assert!(!p.check().ready, "no ref/script → not ready");
    }
}
