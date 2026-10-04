//! Speech-to-text commands (Phase 7, spec §20, §47.7): whisper.cpp settings,
//! availability probe and transcription jobs. Transcriptions run one-shot on
//! a blocking thread (whisper.cpp batch mode) and land as real `transcript`
//! documents in the library.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Everything the Audio tab and Settings → Speech need in one probe.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SttStatusResponse {
    /// True when both paths are set (may still be missing on disk — see the
    /// individual `*_found` flags).
    pub configured: bool,
    pub cli_found: bool,
    pub model_found: bool,
    pub ffmpeg_found: bool,
    pub whisper_cli_path: String,
    pub whisper_model_path: String,
    pub language: String,
    pub convert_with_ffmpeg: bool,
}

fn provider(
    state: &AppState,
    s: &crate::db::SttSettings,
) -> crate::services::transcription::WhisperCppProvider {
    let work = state.data_dir.join("transcribe");
    crate::services::transcription::WhisperCppProvider::from_settings(s, work)
}

fn status_response(state: &AppState) -> AppResult<SttStatusResponse> {
    let s = state.db.get_stt_settings()?;
    let st = crate::services::transcription::SpeechToTextProvider::check(&provider(state, &s));
    Ok(SttStatusResponse {
        configured: !s.whisper_cli_path.is_empty() && !s.whisper_model_path.is_empty(),
        cli_found: st.cli_found,
        model_found: st.model_found,
        ffmpeg_found: st.ffmpeg_found,
        whisper_cli_path: s.whisper_cli_path,
        whisper_model_path: s.whisper_model_path,
        language: s.language,
        convert_with_ffmpeg: s.convert_with_ffmpeg,
    })
}

#[tauri::command]
pub fn stt_check(state: State<'_, AppState>) -> AppResult<SttStatusResponse> {
    status_response(&state)
}

#[tauri::command]
pub fn stt_get_settings(state: State<'_, AppState>) -> AppResult<crate::db::SttSettings> {
    state.db.get_stt_settings()
}

#[tauri::command]
pub fn stt_save_settings(
    state: State<'_, AppState>,
    settings: crate::db::SttSettings,
) -> AppResult<crate::db::SttSettings> {
    let s = prepare_stt_settings(settings)?;
    state.db.save_stt_settings(&s)?;
    Ok(s)
}

/// Normalize and validate saved STT settings (kept free of Tauri state so
/// the guards are unit-testable). The picker is unfiltered, so picking the
/// recording you want to transcribe *as* whisper-cli is the classic mistake
/// — reject it here with guidance instead of failing silently at job time.
fn prepare_stt_settings(mut s: crate::db::SttSettings) -> AppResult<crate::db::SttSettings> {
    s.whisper_cli_path = s.whisper_cli_path.trim().to_string();
    s.whisper_model_path = s.whisper_model_path.trim().to_string();
    let lang = s.language.trim();
    s.language = if lang.is_empty() {
        "auto".into()
    } else {
        lang.to_ascii_lowercase()
    };
    crate::services::path_validation::validate_binary_path(
        &s.whisper_cli_path,
        "whisper-cli",
        "build whisper.cpp and point at its whisper-cli binary, e.g. \
         /opt/whisper.cpp/build/bin/whisper-cli",
    )?;
    crate::services::path_validation::validate_model_path(
        &s.whisper_model_path,
        "a whisper GGML model",
        "bin",
    )?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_a_recording_as_whisper_cli_is_rejected() {
        let err = prepare_stt_settings(crate::db::SttSettings {
            whisper_cli_path: "/Users/odere/Downloads/take_I.wav".into(),
            ..crate::db::SttSettings::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("audio/video file"), "got: {err}");
        assert!(err.contains("whisper-cli"), "got: {err}");
    }

    #[test]
    fn saving_a_whisper_model_with_the_wrong_extension_is_rejected() {
        let err = prepare_stt_settings(crate::db::SttSettings {
            whisper_model_path: "/models/voice.onnx".into(),
            ..crate::db::SttSettings::default()
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains(".bin"), "got: {err}");
    }

    #[test]
    fn unset_and_well_formed_paths_save() {
        let s = prepare_stt_settings(crate::db::SttSettings::default()).unwrap();
        assert_eq!(s.language, "auto");
        let s = prepare_stt_settings(crate::db::SttSettings {
            whisper_cli_path: "  /opt/whisper.cpp/build/bin/whisper-cli  ".into(),
            whisper_model_path: "/opt/whisper.cpp/models/ggml-base.bin".into(),
            ..crate::db::SttSettings::default()
        })
        .unwrap();
        assert_eq!(s.whisper_cli_path, "/opt/whisper.cpp/build/bin/whisper-cli");
    }
}

/// Result of one transcription job (Phase 7).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionJobResult {
    pub document_id: String,
    pub segments: usize,
    pub duration_ms: u64,
    pub language: Option<String>,
    /// Extra note for the UI toast (e.g. embeddings deferred).
    pub detail: String,
}

/// Transcribe an audio file with the configured whisper.cpp provider and
/// store it as a `transcript` document (deduped by audio checksum).
#[tauri::command]
pub async fn stt_transcribe(
    app: AppHandle,
    project_id: String,
    audio_path: String,
) -> AppResult<TranscriptionJobResult> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let s = state.db.get_stt_settings()?;
        if s.whisper_cli_path.trim().is_empty() || s.whisper_model_path.trim().is_empty() {
            return Err(AppError::msg(
                "Whisper is not configured — set the whisper-cli binary and a GGML model in \
                 Settings → Speech.",
            ));
        }
        let audio = std::path::PathBuf::from(&audio_path);
        if !audio.exists() {
            return Err(AppError::msg(format!(
                "Audio file not found: {}",
                audio.display()
            )));
        }

        let prov = provider(&state, &s);
        let result =
            crate::services::transcription::SpeechToTextProvider::transcribe(&prov, &audio)?;
        let doc_id = crate::services::library::save_transcription(
            &state.db,
            &project_id,
            &audio,
            &result,
        )?;

        // Best-effort embeddings so the transcript joins hybrid search;
        // keyword search already works when the engine is offline.
        let mut detail = String::new();
        if let Err(e) = crate::services::retrieval::ensure_document_embedded(&state.db, &doc_id) {
            detail = format!("Indexed for keyword search; embeddings pending ({e}).");
        }

        Ok(TranscriptionJobResult {
            document_id: doc_id,
            segments: result.segments.len(),
            duration_ms: result.duration_ms,
            language: result.language,
            detail,
        })
    })
    .await
    .map_err(|e| AppError::msg(format!("Transcription failed to run: {e}")))?
}
