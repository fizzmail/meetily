# Phase 2: Speaker Diarization — App Integration

Integrate the NeMo-Speech.cpp sidecar (built + validated in Phase 1) into the Minutely app: settings UI, on-demand model download, the "Assign Speakers" flow, segment→transcript matching, speaker naming/merge, and "Listen" clips. This is **post-processing only** — zero changes to the transcription stack.

## HARD CONSTRAINTS
- Do NOT commit or push anything. Leave all changes uncommitted in the working tree.
- Do NOT touch `frontend/src/app/layout.tsx` (preview bypass, stays uncommitted by design).
- Do NOT modify the transcription stack: `src/audio/*`, VAD, Parakeet/Whisper engine, the recording path. Diarization runs AFTER a meeting is recorded, on the saved `audio.mp4`.
- **CRITICAL NAME COLLISION:** the existing `transcripts.speaker` column stores the audio *source* (`'mic'` / `'system'`). Do NOT repurpose or overwrite it. The new diarization identity column MUST be named **`speaker_id`** (nullable INTEGER).
- Do NOT build the Windows sidecar here (that's Phase 1 / CI). Assume the sidecar bundle exists. Your job is the app code + config that consumes it.

## Verified facts (from Phase 0/1 — use as-is, do not re-verify)
- **Sidecar bundle** (from the Phase 1 artifact `nemo-speech-sidecar-windows-x64`): `nemo-speech.exe` + `ggml*.dll` + `nemo_speech_asr*.dll` + `vcomp140.dll`. On the app it's a Tauri sidecar at `binaries/nemo-speech` (Windows: `nemo-speech-x86_64-pc-windows-msvc.exe`).
- **The exact command (ABSOLUTE paths mandatory — the binary resolves relative to its own dir):**
  `nemo-speech.exe diarize <abs.wav> --model <abs.gguf> --device cpu --preset v3-offline --format json`
- **Verified JSON output shape:** `{"file":"<path>","segments":[{"start":0.000,"end":3.659,"speaker":1},...]}` — `start`/`end` in seconds (3-decimal), `speaker` is an integer (1-based).
- **Model constants:** URL `https://huggingface.co/nvidia/Nemotron-3-Diarization/resolve/main/Nemotron-3-Diarization.q8_0.gguf`, size `107012128`, sha256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1`. 100M-param Sortformer, up to 8 speakers (a CAP — the model estimates the count; there is NO speaker-count input), 16 kHz mono input, ~170 MB RAM.
- **16 kHz mono WAV conversion** (bundled `ffmpeg` sidecar, already in the app): `ffmpeg -i <abs>.mp4 -ac 1 -ar 16000 -c:a pcm_s16le <abs>.wav`.
- **`NEMO_SPEECH_MODEL_DIR`** env var — point it at the app's shared data dir so the model lives with the rest of the app's data.
- **Patterns to MIRROR (read them first, don't invent new patterns):**
  - Sidecar spawn: `src/summary/summary_engine/sidecar.rs` (`SidecarManager`) + `client.rs` (`init_sidecar_manager`, `get_sidecar_manager`).
  - Model download with progress: `src/summary/summary_engine/model_manager.rs`.
  - ffmpeg sidecar: `src/audio/ffmpeg.rs`.
  - externalBin: `tauri.conf.json` lines ~101-103 (`["binaries/llama-helper", "binaries/ffmpeg"]`).
  - Migrations: `sqlx::migrate!("./migrations")` in `src/database/manager.rs`; files in `frontend/src-tauri/migrations/` (e.g. `20251110000001_add_speaker_field.sql`).
  - Transcript model: `src/database/models.rs` (`pub struct Transcript`, lines ~26-38).
  - Repositories: `src/database/repositories/transcript.rs`, `meeting.rs`.

## Deliverables (work in this order)

### 1. DB migration — `frontend/src-tauri/migrations/20261002000000_add_diarization.sql`
```sql
-- Diarization identity (post-processing). Distinct from transcripts.speaker (audio source).
ALTER TABLE transcripts ADD COLUMN speaker_id INTEGER;

CREATE TABLE IF NOT EXISTS meeting_speakers (
    meeting_id   TEXT NOT NULL,
    speaker_id   INTEGER NOT NULL,
    display_name TEXT,
    is_merged    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (meeting_id, speaker_id)
);

CREATE TABLE IF NOT EXISTS diarization_runs (
    meeting_id     TEXT PRIMARY KEY,
    status         TEXT NOT NULL,   -- 'idle' | 'running' | 'done' | 'error'
    started_at     TEXT,
    finished_at    TEXT,
    segment_count  INTEGER,
    speaker_count  INTEGER,
    error          TEXT
);
```
Add matching fields to the Rust `Transcript` model (`speaker_id: Option<i32>`) and the repositories that read/write transcripts (mirror how `audio_start_time` is handled).

### 2. Rust sidecar module — `src/diarization/mod.rs` + `sidecar.rs`
- Spawn `nemo-speech` per diarization run (mirror `SidecarManager`). Set `NEMO_SPEECH_MODEL_DIR` to the app data dir.
- Run the EXACT command with ABSOLUTE paths for the WAV and the model.
- Parse JSON from stdout: extract the substring from the first `{` to the last `}` (the CLI may emit progress text), then deserialize.
- Return `Vec<DiarizationSegment { start: f64, end: f64, speaker: i32 }>`.
- Timeout: 20 minutes (a 2-hour meeting at ~RTF 0.04-0.05 is ~5-8 min; leave headroom).
- A `DiarizationManager` (mirror the sidecar manager's init/get lifecycle) so the sidecar is spawned on demand per run and cleaned up after.

### 3. Model download — `src/diarization/model_download.rs`
- Download the GGUF with progress events (mirror `model_manager.rs`): emit a Tauri event with bytes-downloaded / total / percent.
- Verify size == 107012128 AND sha256 == the constant; fail loudly on mismatch.
- Store in the app data dir (the `NEMO_SPEECH_MODEL_DIR` location).
- `is_model_downloaded()` → bool (file exists + size + sha256).

### 4. Tauri commands — `src/diarization/commands.rs` (register all in `lib.rs`)
- `download_diarization_model()` → progress events + final ok/error.
- `is_diarization_model_downloaded()` → bool.
- `run_diarization(meeting_id)`:
  1. Locate the meeting's `audio.mp4` (mirror how the app resolves meeting file paths).
  2. Convert to 16 kHz mono WAV via the bundled `ffmpeg` sidecar (temp file).
  3. Run the sidecar (deliverable 2).
  4. Match segments → transcript rows (deliverable 5).
  5. Persist `transcripts.speaker_id` + `meeting_speakers` rows + a `diarization_runs` row (status 'done', counts).
  6. Return the detected speaker list (ids + default display names like "Speaker 1").
  - On any failure: `diarization_runs` status 'error' + the error string; clean up the temp WAV.
- `rename_speaker(meeting_id, speaker_id, new_name)` → update `meeting_speakers.display_name`.
- `merge_speakers(meeting_id, from_id, to_id)` → set all `transcripts.speaker_id = from_id` to `to_id`, mark `meeting_speakers(from_id).is_merged = 1`, delete the `from_id` row.
- `get_speaker_clip(meeting_id, speaker_id)` → find the speaker's LONGEST single-speaker segment, cut a 3-5 s clip from it via the bundled `ffmpeg`, return the clip path (for the "Listen" button).

### 5. Segment→transcript matching — `src/diarization/match.rs`
For each transcript row R = `[rs, re]` (from `audio_start_time` / `audio_end_time`; skip rows where either is NULL):
- For each segment S = `[ss, se]` with speaker p:
  - `overlap = max(0, min(re, se) - max(rs, ss))`
  - `score = overlap / (se - ss)`  (fraction of the segment covered by the row)
- Assign R to the speaker with the **max score**.
- **Needs-review flag:** if the top-2 scores are within 0.1 of each other AND the winning segment is > 2 s, flag the row "needs review" (the mono mix blends both voices — never silently attribute). Store the flag (e.g. a `diarization_runs`-adjacent column or a `transcripts.speaker_review INTEGER` — your choice, keep it simple).
- If no segment overlaps R, leave R unassigned (`speaker_id = NULL`).

### 6. Frontend — Settings
- New **"Speaker Diarization"** section in Settings (near the Transcription area): an enable toggle, a model-download button (with a progress bar driven by the download events), and model status (not downloaded / downloading / ready).

### 7. Frontend — Meeting detail
- An **"Assign Speakers"** button (only enabled when the model is downloaded): triggers `run_diarization`, shows progress, then a **naming dialog** listing the detected speakers.
- **Speaker badges** on transcript rows (color per speaker; "needs review" rows get a distinct marker).
- **Rename** dialog + **merge-duplicates** action.
- A **"Listen"** button per speaker → plays the clip from `get_speaker_clip` via the existing audio player.

### 8. Build integration (code change — CANNOT be tested on this Linux box; CI validates it)
- `tauri.conf.json` `externalBin`: add `"binaries/nemo-speech"`.
- `build-windows.yml`: add a step (before "Build Tauri app") that produces the nemo-speech sidecar bundle and copies it into `frontend/src-tauri/binaries/` as `nemo-speech-x86_64-pc-windows-msvc.exe` (+ the DLLs). You may port the build logic from `.github/workflows/build-nemo-speech.yml` (the Phase 1 workflow) into a step, or reference the Phase 1 approach. Keep it self-contained.

## Effort estimate
A feature, not a rewrite. The sidecar spawn + model download mirror existing patterns (deliverables 2-3 are mostly "copy the pattern, swap the binary + constants"). The matching (deliverable 5) is new but bounded. The UI (6-7) is additive. The build integration (8) is a code change validated by CI, not by this box.

## Acceptance criteria (verify before reporting done)
1. `cargo check` clean (in `frontend/src-tauri`).
2. `npx tsc --noEmit` clean (in `frontend`).
3. The new migration file exists and is valid SQL; the `Transcript` model + repositories updated for `speaker_id`.
4. All new commands registered in `lib.rs`.
5. `git status` shows only the new/modified files — NO `layout.tsx`, NO `tauri.conf.json` change that breaks the existing sidecars, nothing committed.
6. Report back: the sidecar spawn path you used, the matching algorithm as implemented, the UI locations, the exact `externalBin` entry, and any surprises (e.g. how meeting file paths are resolved, whether an existing "settings" section pattern was reused).

## Out of scope (deferred)
- Live diarization during recording (this is post-processing only).
- Any change to the transcription stack.
- Including speaker names in the summary prompt (optional later).
