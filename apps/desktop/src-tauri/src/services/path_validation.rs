//! Save-time validation for user-supplied tool/model paths (Settings →
//! Speech and anything else where a file picker hands back an arbitrary
//! path).
//!
//! The pickers are unfiltered by necessity — a native open panel with a
//! content-type filter would hide binaries, and the same no-filter picker
//! serves GGUF model selection — so the classic misconfiguration is picking
//! the recording you want to work with *as* the binary. Without validation
//! that fails silently later (or, worse, shows a green "found" chip), so the
//! save command rejects paths that are clearly wrong with actionable copy.
//!
//! Empty and merely-not-yet-installed paths are allowed: the status chips in
//! the UI own the "missing on disk" story, this guard only rejects *wrong*.

use crate::error::{AppError, AppResult};

/// Audio/video extensions that are never a binary and never a model — the
/// exact picker mistake this guard exists for.
const MEDIA_EXTS: &[&str] = &[
    "wav", "mp3", "m4a", "m4b", "aac", "aif", "aiff", "flac", "ogg", "oga", "opus", "wma",
    "mp4", "mov", "mkv", "webm", "avi", "m4v",
];

/// Lowercase extension of `path` ("" when there is none).
fn ext(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn is_media(path: &str) -> bool {
    MEDIA_EXTS.contains(&ext(path).as_str())
}

/// Validate a user-supplied executable path (`whisper-cli`, `piper`, …).
///
/// * empty → OK (unset)
/// * missing file → OK (install later; the status chip shows "missing")
/// * audio/video extension, a directory, or a file without the executable
///   bit → rejected with `install_hint` attached
pub(crate) fn validate_binary_path(path: &str, tool: &str, install_hint: &str) -> AppResult<()> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    if is_media(path) {
        return Err(AppError::msg(format!(
            "“{path}” is an audio/video file, not the {tool} executable. Use Browse… to pick \
             the {tool} binary instead — {install_hint}"
        )));
    }
    let p = std::path::Path::new(path);
    if p.is_dir() {
        return Err(AppError::msg(format!(
            "“{path}” is a folder, not the {tool} binary — {install_hint}"
        )));
    }
    if p.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let executable = std::fs::metadata(p)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false);
            if !executable {
                return Err(AppError::msg(format!(
                    "“{path}” is not executable, so it cannot be the {tool} binary — \
                     {install_hint}"
                )));
            }
        }
    }
    Ok(())
}

/// Validate a user-supplied model/voice file path against the one extension
/// the tool accepts (`bin` for whisper GGML models, `onnx` for Piper voices).
///
/// Empty and missing are allowed (download later); a wrong extension — most
/// importantly an audio file — is not.
pub(crate) fn validate_model_path(
    path: &str,
    label: &str,
    expected_ext: &str,
) -> AppResult<()> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    let p = std::path::Path::new(path);
    if p.is_dir() {
        return Err(AppError::msg(format!(
            "“{path}” is a folder, not {label}."
        )));
    }
    let got = ext(path);
    if got != expected_ext.to_lowercase() {
        if is_media(path) {
            return Err(AppError::msg(format!(
                "“{path}” is an audio/video file, not {label} — pick the .{expected_ext} file \
                 with Browse…"
            )));
        }
        return Err(AppError::msg(format!(
            "“{path}” does not look like {label} — expected a .{expected_ext} file."
        )));
    }
    Ok(())
}

/// Validate a user-supplied *audio* path (the Master Voice reference
/// recording — the inverse of the binary/model guards above).
///
/// * empty → rejected when the provider needs it (caller decides)
/// * directory or a non-audio extension → rejected
/// * missing file → allowed (the status chip owns "missing on disk")
pub(crate) fn validate_audio_path(path: &str, label: &str) -> AppResult<()> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(());
    }
    let p = std::path::Path::new(path);
    if p.is_dir() {
        return Err(AppError::msg(format!(
            "“{path}” is a folder, not {label} — pick an audio file with Browse…"
        )));
    }
    if !is_media(path) {
        return Err(AppError::msg(format!(
            "“{path}” does not look like {label} — pick a recording such as .wav, .mp3 or \
             .m4a with Browse…"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_missing_paths_are_allowed() {
        assert!(validate_binary_path("", "piper", "hint").is_ok());
        assert!(validate_binary_path("   ", "piper", "hint").is_ok());
        assert!(validate_binary_path("/no/such/piper", "piper", "hint").is_ok());
        assert!(validate_model_path("", "a voice", "onnx").is_ok());
        assert!(validate_model_path("/no/such/voice.onnx", "a voice", "onnx").is_ok());
    }

    #[test]
    fn audio_files_are_rejected_as_binaries() {
        // The real-world mistake: the picker handed back the recording.
        let err = validate_binary_path(
            "/Users/odere/Downloads/take_I.wav",
            "piper",
            "e.g. brew install piper",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("audio/video file"), "got: {err}");
        assert!(err.contains("piper"), "got: {err}");
        // Rejected even when the file does not exist — the extension alone
        // proves the wrong thing was picked.
        assert!(validate_binary_path("recording.mp3", "whisper-cli", "hint").is_err());
        assert!(validate_binary_path("talk.m4a", "whisper-cli", "hint").is_err());
    }

    #[test]
    fn audio_files_are_rejected_as_models() {
        let err = validate_model_path(
            "/Users/odere/Downloads/take_I.wav",
            "a Piper voice model",
            "onnx",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("audio/video file"), "got: {err}");
        assert!(validate_model_path("model.gguf", "a whisper GGML model", "bin").is_err());
    }

    #[test]
    fn correct_extensions_pass() {
        assert!(validate_model_path("/voices/en_US-amy-medium.onnx", "a Piper voice", "onnx").is_ok());
        assert!(validate_model_path("/models/ggml-base.bin", "a GGML model", "bin").is_ok());
        assert!(validate_binary_path("/opt/homebrew/bin/piper", "piper", "hint").is_ok());
    }

    #[test]
    fn master_voice_reference_must_look_like_audio() {
        assert!(validate_audio_path("/Users/x/Downloads/take_I.wav", "the master voice recording").is_ok());
        assert!(validate_audio_path("/recordings/talk.m4a", "the master voice recording").is_ok());
        // The inverse mistake: a binary/model where audio is expected.
        let err = validate_audio_path("/opt/homebrew/bin/piper", "the master voice recording")
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not look like"), "got: {err}");
        assert!(validate_audio_path("/models/voice.onnx", "the master voice recording").is_err());
        // Directories are never recordings; missing files pass (chip shows it).
        let dir = crate::db::tests::TempDir::new_with_label("pathval-audio");
        assert!(validate_audio_path(&dir.path().to_string_lossy(), "the master voice recording").is_err());
        assert!(validate_audio_path("/no/such/recording.wav", "the master voice recording").is_ok());
        assert!(validate_audio_path("", "the master voice recording").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn directories_and_non_executables_are_rejected() {
        let dir = crate::db::tests::TempDir::new_with_label("pathval");
        let dir_path = dir.path().join("some-dir");
        std::fs::create_dir(&dir_path).unwrap();
        assert!(validate_binary_path(&dir_path.to_string_lossy(), "piper", "hint").is_err());

        let plain = dir.path().join("not-a-binary");
        std::fs::write(&plain, b"#!/bin/sh\necho hi\n").unwrap();
        // 0644 — no executable bit.
        let err = validate_binary_path(&plain.to_string_lossy(), "piper", "hint")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not executable"), "got: {err}");

        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_binary_path(&plain.to_string_lossy(), "piper", "hint").is_ok());
    }
}
