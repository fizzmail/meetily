// Tauri commands for speaker diarization (post-processing).
//
// Exposes: model download/status, the full diarization run (audio -> WAV ->
// sidecar -> match -> persist), speaker rename/merge, per-speaker "Listen"
// clips, and speaker-list/status reads.

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::state::AppState;

use super::matching as match_mod;
use super::model_download;
use super::repository::{self, MeetingSpeaker, TranscriptSpeakerAssignment};
use super::sidecar;

// ============================================================================
// Helpers
// ============================================================================

/// Resolve the meeting's folder and audio file path.
async fn resolve_meeting_audio(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<(String, PathBuf), String> {
    let folder: Option<String> = sqlx::query_scalar(
        "SELECT folder_path FROM meetings WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Failed to look up meeting folder: {}", e))?
    .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let folder = folder
        .filter(|f| !f.trim().is_empty())
        .ok_or_else(|| format!("Meeting {} has no folder path", meeting_id))?;

    let audio_path = find_audio_file(std::path::Path::new(&folder))
        .map_err(|e| format!("No audio file in {}: {}", folder, e))?;

    Ok((folder, audio_path))
}

/// Find the audio file in a meeting folder (common names first, then scan).
fn find_audio_file(folder: &std::path::Path) -> Result<PathBuf, String> {
    let candidates = [
        "audio.mp4", "audio.m4a", "audio.wav", "audio.mp3",
        "audio.flac", "audio.ogg", "recording.mp4",
        "audio.mkv", "audio.webm", "audio.wma",
    ];
    for name in candidates {
        let path = folder.join(name);
        if path.exists() {
            return Ok(path);
        }
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        let audio_exts = [
            "mp4", "m4a", "wav", "mp3", "flac", "ogg",
            "mkv", "webm", "wma",
        ];
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if audio_exts.contains(&ext.as_str()) {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("No audio file found in: {}", folder.display()))
}

/// Convert an audio file to a 16 kHz mono WAV using the bundled ffmpeg sidecar.
async fn convert_to_wav(audio_path: &PathBuf, out_wav: &PathBuf) -> Result<(), String> {
    let ffmpeg = crate::audio::ffmpeg::find_ffmpeg_path()
        .ok_or_else(|| "ffmpeg sidecar not found".to_string())?;

    let output = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(audio_path)
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(out_wav)
        .output()
        .await
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg conversion failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }

    Ok(())
}

/// Build a unique temp path for a WAV file.
fn make_temp_wav_path() -> PathBuf {
    let dir = std::env::temp_dir();
    let name = format!("nemo_diarize_{}.wav", uuid::Uuid::new_v4());
    dir.join(name)
}

/// Fetch the transcript rows (id + timing) for a meeting.
async fn fetch_transcript_rows(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<Vec<match_mod::TranscriptRow>, String> {
    let rows: Vec<(String, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT id, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to fetch transcript rows: {}", e))?;

    Ok(rows
        .into_iter()
        .map(|(id, s, e)| match_mod::TranscriptRow {
            id,
            audio_start_time: s,
            audio_end_time: e,
        })
        .collect())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ============================================================================
// Model download / status
// ============================================================================

/// Download the diarization model, emitting progress events.
#[tauri::command]
pub async fn download_diarization_model<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let models_dir = app_data_dir.join("models").join("nemo-speech");

    let app_clone = app.clone();
    let progress_callback = Box::new(move |p: model_download::DiarizationDownloadProgress| {
        let _ = app_clone.emit(
            "diarization-model-download-progress",
            serde_json::json!({
                "downloaded_bytes": p.downloaded_bytes,
                "total_bytes": p.total_bytes,
                "percent": p.percent,
            }),
        );
    });

    match model_download::download_model(&models_dir, Some(progress_callback)).await {
        Ok(_) => {
            let _ = app.emit(
                "diarization-model-download-complete",
                serde_json::json!({}),
            );
            Ok(())
        }
        Err(e) => {
            let _ = app.emit(
                "diarization-model-download-error",
                serde_json::json!({ "error": e.to_string() }),
            );
            Err(e.to_string())
        }
    }
}

/// Check whether the diarization model is downloaded and valid.
#[tauri::command]
pub async fn is_diarization_model_downloaded<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let models_dir = app_data_dir.join("models").join("nemo-speech");
    Ok(model_download::is_model_downloaded(&models_dir))
}

// ============================================================================
// Diarization run
// ============================================================================

/// Run diarization for a meeting: audio -> WAV -> sidecar -> match -> persist.
#[tauri::command]
pub async fn run_diarization<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<MeetingSpeaker>, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let pool = state.db_manager.pool();
    let models_dir = app_data_dir.join("models").join("nemo-speech");

    // 1. Model must be downloaded.
    if !model_download::is_model_downloaded(&models_dir) {
        return Err("Diarization model is not downloaded. Download it in Settings first.".to_string());
    }
    let model_path = model_download::model_path(&models_dir);

    // 2. Locate the meeting audio.
    let (_folder, audio_path) = resolve_meeting_audio(pool, &meeting_id).await?;

    // 3. Temp WAV (cleaned up regardless of outcome).
    let wav_path = make_temp_wav_path();

    let started_at = now_rfc3339();
    let _ = repository::save_diarization_run(
        pool,
        &meeting_id,
        "running",
        Some(&started_at),
        None,
        None,
        None,
        None,
    )
    .await;

    let result: Result<Vec<MeetingSpeaker>, String> = async {
        // 4. Convert to 16 kHz mono WAV.
        convert_to_wav(&audio_path, &wav_path).await?;

        // 5. Run the sidecar.
        let segments = sidecar::run_diarization(&app_data_dir, &wav_path, &model_path)
            .await
            .map_err(|e| e.to_string())?;

        // 6. Match segments -> transcript rows.
        let rows = fetch_transcript_rows(pool, &meeting_id).await?;
        let assignments = match_mod::assign(&rows, &segments);

        let transcript_assignments: Vec<TranscriptSpeakerAssignment> = rows
            .iter()
            .zip(assignments.iter())
            .map(|(row, a)| TranscriptSpeakerAssignment {
                transcript_id: row.id.clone(),
                speaker_id: a.speaker_id,
                needs_review: a.needs_review,
            })
            .collect();

        // 7. Build the detected speaker list (distinct ids, default names).
        let speaker_ids: Vec<i32> = segments
            .iter()
            .map(|s| s.speaker)
            .collect::<std::collections::BTreeSet<i32>>()
            .into_iter()
            .collect();

        let speakers: Vec<MeetingSpeaker> = speaker_ids
            .iter()
            .map(|id| MeetingSpeaker {
                meeting_id: meeting_id.clone(),
                speaker_id: *id,
                display_name: Some(format!("Speaker {}", id)),
                is_merged: 0,
            })
            .collect();

        // 8. Persist.
        repository::save_transcript_speaker_assignments(pool, &meeting_id, &transcript_assignments)
            .await
            .map_err(|e| format!("Failed to save transcript assignments: {}", e))?;
        repository::save_meeting_speakers(pool, &meeting_id, &speakers)
            .await
            .map_err(|e| format!("Failed to save meeting speakers: {}", e))?;

        let finished_at = now_rfc3339();
        repository::save_diarization_run(
            pool,
            &meeting_id,
            "done",
            Some(&started_at),
            Some(&finished_at),
            Some(segments.len() as i32),
            Some(speaker_ids.len() as i32),
            None,
        )
        .await
        .map_err(|e| format!("Failed to save diarization run: {}", e))?;

        Ok(speakers)
    }
    .await;

    // Clean up the temp WAV.
    let _ = std::fs::remove_file(&wav_path);

    if let Err(e) = &result {
        let _ = repository::save_diarization_run(
            pool,
            &meeting_id,
            "error",
            Some(&started_at),
            Some(&now_rfc3339()),
            None,
            None,
            Some(e),
        )
        .await;
    }

    result
}

// ============================================================================
// Speaker rename / merge
// ============================================================================

/// Rename a speaker's display name.
#[tauri::command]
pub async fn rename_speaker<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    speaker_id: i32,
    new_name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    let ok = repository::rename_speaker(pool, &meeting_id, speaker_id, &new_name)
        .await
        .map_err(|e| format!("Failed to rename speaker: {}", e))?;
    if !ok {
        return Err(format!("Speaker {} not found", speaker_id));
    }
    Ok(())
}

/// Merge one speaker into another.
#[tauri::command]
pub async fn merge_speakers<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    from_id: i32,
    to_id: i32,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if from_id == to_id {
        return Err("Cannot merge a speaker into itself".to_string());
    }
    let pool = state.db_manager.pool();
    let ok = repository::merge_speakers(pool, &meeting_id, from_id, to_id)
        .await
        .map_err(|e| format!("Failed to merge speakers: {}", e))?;
    if !ok {
        return Err(format!("Speaker {} not found to merge", from_id));
    }
    Ok(())
}

// ============================================================================
// Speaker clip (Listen)
// ============================================================================

/// Cut a short clip from the speaker's longest attributed transcript row.
#[tauri::command]
pub async fn get_speaker_clip<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    speaker_id: i32,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let pool = state.db_manager.pool();

    // Resolve the meeting audio.
    let (folder, audio_path) = resolve_meeting_audio(pool, &meeting_id).await?;

    // Find the longest transcript row attributed to this speaker.
    let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT audio_start_time, audio_end_time
         FROM transcripts
         WHERE meeting_id = ? AND speaker_id = ?
           AND audio_start_time IS NOT NULL AND audio_end_time IS NOT NULL
         ORDER BY (audio_end_time - audio_start_time) DESC
         LIMIT 1",
    )
    .bind(&meeting_id)
    .bind(speaker_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("Failed to find speaker clip window: {}", e))?;

    let (start, end) = row
        .and_then(|(s, e)| match (s, e) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        })
        .ok_or_else(|| format!("No audio found for speaker {}", speaker_id))?;

    let row_duration = (end - start).max(0.0);
    let clip_duration = row_duration.min(4.0);
    if clip_duration <= 0.0 {
        return Err(format!("Speaker {} has no usable audio window", speaker_id));
    }

    // Output the clip into the meeting folder (persists with the meeting).
    let clip_path = PathBuf::from(&folder).join(format!("speaker_clip_{}.wav", speaker_id));

    let ffmpeg = crate::audio::ffmpeg::find_ffmpeg_path()
        .ok_or_else(|| "ffmpeg sidecar not found".to_string())?;

    let output = tokio::process::Command::new(&ffmpeg)
        .arg("-y")
        .arg("-ss")
        .arg(format!("{:.3}", start))
        .arg("-i")
        .arg(&audio_path)
        .arg("-t")
        .arg(format!("{:.3}", clip_duration))
        .arg("-ac")
        .arg("1")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(&clip_path)
        .output()
        .await
        .map_err(|e| format!("Failed to run ffmpeg for clip: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg clip cut failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }

    Ok(clip_path.to_string_lossy().to_string())
}

// ============================================================================
// Reads
// ============================================================================

/// Get the speaker list for a meeting (for badges + naming).
#[tauri::command]
pub async fn get_meeting_speakers<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<MeetingSpeaker>, String> {
    let pool = state.db_manager.pool();
    repository::get_meeting_speakers(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to get meeting speakers: {}", e))
}

/// Get the current diarization run status for a meeting.
#[tauri::command]
pub async fn get_diarization_status<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let pool = state.db_manager.pool();
    repository::get_diarization_status(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to get diarization status: {}", e))
}
