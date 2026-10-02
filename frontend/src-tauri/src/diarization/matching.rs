// Segment -> transcript matching.
//
// For each transcript row (a time window [rs, re]) we score every diarization
// segment by how much of the segment the row covers, then attribute the row to
// the speaker with the highest score. Ambiguous rows (two speakers with close
// scores over a long segment) are flagged "needs review" rather than silently
// attributed, because the mono mix blends both voices.

use super::sidecar::DiarizationSegment;

/// A transcript row to be matched against diarization segments.
#[derive(Debug, Clone)]
pub struct TranscriptRow {
    pub id: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
}

/// The result of matching a single transcript row.
#[derive(Debug, Clone)]
pub struct RowAssignment {
    /// The attributed speaker id, or None if no segment overlapped the row.
    pub speaker_id: Option<i32>,
    /// True when the attribution is ambiguous and should be reviewed.
    pub needs_review: bool,
}

/// A single (speaker, best-score, winning-segment-duration) triple used during
/// matching.
#[derive(Debug, Clone, Copy)]
struct SpeakerScore {
    speaker: i32,
    score: f64,
    winning_segment_duration: f64,
}

/// Match transcript rows against diarization segments.
///
/// Returns one [`RowAssignment`] per row, in the same order as `rows`.
pub fn assign(rows: &[TranscriptRow], segments: &[DiarizationSegment]) -> Vec<RowAssignment> {
    rows.iter()
        .map(|row| match_row(row, segments))
        .collect()
}

/// Match a single transcript row against all segments.
fn match_row(row: &TranscriptRow, segments: &[DiarizationSegment]) -> RowAssignment {
    let (rs, re) = match (row.audio_start_time, row.audio_end_time) {
        (Some(s), Some(e)) => (s, e),
        _ => {
            // Skip rows without timing info.
            return RowAssignment {
                speaker_id: None,
                needs_review: false,
            };
        }
    };

    // Best score per speaker (max over that speaker's segments).
    let mut best: Vec<SpeakerScore> = Vec::new();

    for seg in segments {
        let seg_duration = seg.end - seg.start;
        if seg_duration <= 0.0 {
            continue;
        }

        let overlap = (re.min(seg.end) - rs.max(seg.start)).max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        let score = overlap / seg_duration;

        // Update this speaker's best entry.
        match best.iter_mut().find(|s| s.speaker == seg.speaker) {
            Some(entry) => {
                if score > entry.score {
                    entry.score = score;
                    entry.winning_segment_duration = seg_duration;
                }
            }
            None => best.push(SpeakerScore {
                speaker: seg.speaker,
                score,
                winning_segment_duration: seg_duration,
            }),
        }
    }

    // No segment overlapped this row.
    if best.is_empty() {
        return RowAssignment {
            speaker_id: None,
            needs_review: false,
        };
    }

    // Rank speakers by score (descending).
    best.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let winner = best[0];
    if winner.score <= 0.0 {
        return RowAssignment {
            speaker_id: None,
            needs_review: false,
        };
    }

    // Needs review: the top-2 distinct speakers are within 0.1 of each other AND
    // the winning segment is longer than 2s (a long, blended window).
    let needs_review = if best.len() >= 2 {
        let runner_up = best[1].score;
        (winner.score - runner_up) < 0.1 && winner.winning_segment_duration > 2.0
    } else {
        false
    };

    RowAssignment {
        speaker_id: Some(winner.speaker),
        needs_review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, speaker: i32) -> DiarizationSegment {
        DiarizationSegment {
            start,
            end,
            speaker,
        }
    }

    fn row(id: &str, start: Option<f64>, end: Option<f64>) -> TranscriptRow {
        TranscriptRow {
            id: id.to_string(),
            audio_start_time: start,
            audio_end_time: end,
        }
    }

    #[test]
    fn row_without_timing_is_unassigned() {
        let rows = vec![row("a", None, Some(5.0))];
        let segments = vec![seg(0.0, 5.0, 1)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, None);
        assert!(!out[0].needs_review);
    }

    #[test]
    fn no_overlap_is_unassigned() {
        let rows = vec![row("a", Some(10.0), Some(12.0))];
        let segments = vec![seg(0.0, 5.0, 1)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, None);
    }

    #[test]
    fn clear_single_speaker() {
        let rows = vec![row("a", Some(1.0), Some(2.0))];
        let segments = vec![seg(0.0, 2.0, 1), seg(5.0, 10.0, 2)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, Some(1));
        assert!(!out[0].needs_review);
    }

    #[test]
    fn ambiguous_close_scores_on_long_segment_needs_review() {
        // Row fully inside a long segment of speaker 1, and a speaker 2 segment
        // that also fully covers it -> both scores ~1.0, within 0.1, segment > 2s.
        let rows = vec![row("a", Some(3.0), Some(4.0))];
        let segments = vec![seg(0.0, 10.0, 1), seg(0.0, 10.0, 2)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, Some(1));
        assert!(out[0].needs_review);
    }

    #[test]
    fn close_scores_but_short_segment_not_flagged() {
        // Both fully cover the row (score ~1.0) but the segments are < 2s.
        let rows = vec![row("a", Some(0.5), Some(1.0))];
        let segments = vec![seg(0.0, 1.5, 1), seg(0.0, 1.5, 2)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, Some(1));
        assert!(!out[0].needs_review);
    }

    #[test]
    fn clearly_different_scores_not_flagged() {
        // Row [0,10] fully covers speaker 1's [0,10] (score 1.0) but only
        // partially covers speaker 2's [0,20] (score 0.5). Difference 0.5 > 0.1
        // -> not flagged.
        let rows = vec![row("a", Some(0.0), Some(10.0))];
        let segments = vec![seg(0.0, 10.0, 1), seg(0.0, 20.0, 2)];
        let out = assign(&rows, &segments);
        assert_eq!(out[0].speaker_id, Some(1));
        assert!(!out[0].needs_review);
    }

    #[test]
    fn multi_speaker_all_fully_covered_is_flagged() {
        // Row [0,10] fully covers three distinct speakers' segments (all score
        // 1.0). Top-2 within 0.1 and the winning segment (10s) is > 2s -> review.
        let rows = vec![row("a", Some(0.0), Some(10.0))];
        let segments = vec![seg(0.0, 10.0, 1), seg(0.0, 10.0, 2), seg(8.0, 10.0, 3)];
        let out = assign(&rows, &segments);
        assert!(out[0].needs_review);
    }
}
