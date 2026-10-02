// Speaker diarization (post-processing only).
//
// Runs the NeMo-Speech.cpp sidecar against the saved meeting audio to produce
// per-segment speaker labels, then matches those segments onto transcript rows.
// This module never touches the transcription stack (VAD / Whisper / Parakeet).

pub mod commands;
pub mod matching;
pub mod model_download;
pub mod repository;
pub mod sidecar;
