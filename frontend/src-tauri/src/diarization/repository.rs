// Database access for diarization: transcript speaker assignment, per-meeting
// speaker registry, and diarization run status.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

/// A per-meeting speaker (identity + display name + merge state).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MeetingSpeaker {
    pub meeting_id: String,
    pub speaker_id: i32,
    pub display_name: Option<String>,
    pub is_merged: i32,
}

/// A single transcript-row assignment to persist.
pub struct TranscriptSpeakerAssignment {
    pub transcript_id: String,
    pub speaker_id: Option<i32>,
    pub needs_review: bool,
}

/// Persist speaker assignments for a set of transcript rows.
pub async fn save_transcript_speaker_assignments(
    pool: &SqlitePool,
    meeting_id: &str,
    assignments: &[TranscriptSpeakerAssignment],
) -> Result<(), sqlx::Error> {
    for a in assignments {
        sqlx::query(
            "UPDATE transcripts
             SET speaker_id = ?, speaker_review = ?
             WHERE meeting_id = ? AND id = ?",
        )
        .bind(a.speaker_id)
        .bind(if a.needs_review { 1 } else { 0 })
        .bind(meeting_id)
        .bind(&a.transcript_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Upsert (replace) the speaker registry for a meeting.
pub async fn save_meeting_speakers(
    pool: &SqlitePool,
    meeting_id: &str,
    speakers: &[MeetingSpeaker],
) -> Result<(), sqlx::Error> {
    // Clear existing (non-merged rows are replaced; merged rows are preserved).
    sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ? AND is_merged = 0")
        .bind(meeting_id)
        .execute(pool)
        .await?;

    for s in speakers {
        sqlx::query(
            "INSERT INTO meeting_speakers (meeting_id, speaker_id, display_name, is_merged)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(meeting_id, speaker_id) DO UPDATE SET
                display_name = excluded.display_name,
                is_merged = excluded.is_merged",
        )
        .bind(meeting_id)
        .bind(s.speaker_id)
        .bind(&s.display_name)
        .bind(s.is_merged)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Get all speakers for a meeting (including merged, so the UI can filter).
pub async fn get_meeting_speakers(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Vec<MeetingSpeaker>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MeetingSpeaker>(
        "SELECT meeting_id, speaker_id, display_name, is_merged
         FROM meeting_speakers
         WHERE meeting_id = ?
         ORDER BY speaker_id ASC",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Rename a speaker's display name.
pub async fn rename_speaker(
    pool: &SqlitePool,
    meeting_id: &str,
    speaker_id: i32,
    new_name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE meeting_speakers SET display_name = ? WHERE meeting_id = ? AND speaker_id = ?",
    )
    .bind(new_name)
    .bind(meeting_id)
    .bind(speaker_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Merge `from_id` into `to_id`: reassign all transcript rows, then mark the
/// source speaker as merged (kept, flagged) so the UI can hide it.
pub async fn merge_speakers(
    pool: &SqlitePool,
    meeting_id: &str,
    from_id: i32,
    to_id: i32,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    // 1. Reassign transcript rows.
    sqlx::query(
        "UPDATE transcripts SET speaker_id = ? WHERE meeting_id = ? AND speaker_id = ?",
    )
    .bind(to_id)
    .bind(meeting_id)
    .bind(from_id)
    .execute(&mut *tx)
    .await?;

    // 2. Mark the source speaker as merged (kept so it is tracked as merged).
    let marked = sqlx::query(
        "UPDATE meeting_speakers SET is_merged = 1 WHERE meeting_id = ? AND speaker_id = ?",
    )
    .bind(meeting_id)
    .bind(from_id)
    .execute(&mut *tx)
    .await?;

    // 3. Ensure the target speaker exists (in case it had no registry row).
    let target_exists: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM meeting_speakers WHERE meeting_id = ? AND speaker_id = ?",
    )
    .bind(meeting_id)
    .bind(to_id)
    .fetch_optional(&mut *tx)
    .await?;
    if target_exists.is_none() {
        sqlx::query(
            "INSERT INTO meeting_speakers (meeting_id, speaker_id, display_name, is_merged)
             VALUES (?, ?, ?, 0)",
        )
        .bind(meeting_id)
        .bind(to_id)
        .bind(None::<String>)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(marked.rows_affected() > 0)
}

/// Upsert a diarization run status row.
pub async fn save_diarization_run(
    pool: &SqlitePool,
    meeting_id: &str,
    status: &str,
    started_at: Option<&str>,
    finished_at: Option<&str>,
    segment_count: Option<i32>,
    speaker_count: Option<i32>,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO diarization_runs (meeting_id, status, started_at, finished_at, segment_count, speaker_count, error)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET
            status = excluded.status,
            started_at = excluded.started_at,
            finished_at = excluded.finished_at,
            segment_count = excluded.segment_count,
            speaker_count = excluded.speaker_count,
            error = excluded.error",
    )
    .bind(meeting_id)
    .bind(status)
    .bind(started_at)
    .bind(finished_at)
    .bind(segment_count)
    .bind(speaker_count)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the current diarization run status for a meeting.
pub async fn get_diarization_status(
    pool: &SqlitePool,
    meeting_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT status FROM diarization_runs WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}
