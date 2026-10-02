-- Migration: Add speaker diarization (post-processing) support
--
-- NOTE: transcripts.speaker (TEXT) already exists and stores the audio SOURCE
-- ('mic' / 'system'). It is intentionally NOT touched here. The diarization
-- identity is stored in the new `speaker_id` column (INTEGER, nullable).
--
-- `speaker_review` (INTEGER, nullable) flags a transcript row whose speaker
-- attribution is ambiguous (mono mix blended two voices) and needs manual review.

-- Diarization identity (post-processing). Distinct from transcripts.speaker (audio source).
ALTER TABLE transcripts ADD COLUMN speaker_id INTEGER;

-- Per-row "needs review" flag set by the segment->transcript matcher.
ALTER TABLE transcripts ADD COLUMN speaker_review INTEGER;

-- Per-meeting speaker registry: identity + user-assigned display name + merge state.
CREATE TABLE IF NOT EXISTS meeting_speakers (
    meeting_id   TEXT NOT NULL,
    speaker_id   INTEGER NOT NULL,
    display_name TEXT,
    is_merged    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (meeting_id, speaker_id)
);

-- Per-meeting diarization run status/counts.
CREATE TABLE IF NOT EXISTS diarization_runs (
    meeting_id     TEXT PRIMARY KEY,
    status         TEXT NOT NULL,   -- 'idle' | 'running' | 'done' | 'error'
    started_at     TEXT,
    finished_at    TEXT,
    segment_count  INTEGER,
    speaker_count  INTEGER,
    error          TEXT
);
