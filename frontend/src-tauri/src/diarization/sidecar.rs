// Diarization sidecar: spawns the nemo-speech CLI per run, captures JSON from
// stdout, and returns the parsed speaker segments.
//
// Mirrors the SidecarManager / init-get lifecycle of the summary engine, but
// nemo-speech is a one-shot CLI (args in, JSON out, process exits) rather than
// a persistent stdin/stdout server, so each run spawns a fresh process that is
// cleaned up when it completes (or on timeout).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tokio::process::Command;
use tokio::sync::Mutex;

/// Diarization run timeout. A 2-hour meeting at ~RTF 0.04-0.05 is ~5-8 min;
/// leave generous headroom.
pub const DIARIZATION_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// A single diarization segment: a time window attributed to one speaker.
#[derive(Debug, Clone, Deserialize)]
pub struct DiarizationSegment {
    /// Start time in seconds (3-decimal).
    pub start: f64,
    /// End time in seconds (3-decimal).
    pub end: f64,
    /// Speaker label (1-based integer).
    pub speaker: i32,
}

/// Shape of the nemo-speech JSON output.
#[derive(Debug, Deserialize)]
struct DiarizationOutput {
    #[serde(default)]
    #[allow(dead_code)]
    file: Option<String>,
    segments: Vec<DiarizationSegment>,
}

/// Manages spawning the nemo-speech sidecar for diarization runs.
pub struct DiarizationManager {
    /// Absolute path to the nemo-speech binary.
    binary_path: PathBuf,
    /// Directory the model lives in (passed to the sidecar via NEMO_SPEECH_MODEL_DIR).
    model_dir: PathBuf,
}

impl DiarizationManager {
    /// Create a new manager, resolving the sidecar binary and model directory.
    pub fn new(app_data_dir: &PathBuf) -> Result<Self> {
        let binary_path = Self::resolve_binary()?;
        // Keep the model with the rest of the app's data (NEMO_SPEECH_MODEL_DIR).
        let model_dir = app_data_dir.join("models").join("nemo-speech");
        std::fs::create_dir_all(&model_dir)
            .with_context(|| format!("Failed to create model dir: {}", model_dir.display()))?;

        log::info!("DiarizationManager initialized");
        log::info!("  binary: {}", binary_path.display());
        log::info!("  model dir: {}", model_dir.display());

        Ok(Self { binary_path, model_dir })
    }

    /// The model directory (where the GGUF is stored / expected).
    pub fn model_dir(&self) -> &PathBuf {
        &self.model_dir
    }

    /// Run diarization on an absolute WAV path against an absolute model path.
    ///
    /// Spawns a fresh sidecar process, waits up to `DIARIZATION_TIMEOUT`, and
    /// parses the JSON emitted on stdout. The process is cleaned up when it
    /// completes or is killed on timeout.
    pub async fn run(&self, wav_path: &PathBuf, model_path: &PathBuf) -> Result<Vec<DiarizationSegment>> {
        if !wav_path.is_absolute() {
            return Err(anyhow!("WAV path must be absolute: {}", wav_path.display()));
        }
        if !model_path.is_absolute() {
            return Err(anyhow!("Model path must be absolute: {}", model_path.display()));
        }

        log::info!("Spawning nemo-speech diarization");
        log::info!("  wav: {}", wav_path.display());
        log::info!("  model: {}", model_path.display());

        let mut command = Command::new(&self.binary_path);
        command
            .arg("diarize")
            .arg(wav_path)
            .arg("--model")
            .arg(model_path)
            .arg("--device")
            .arg("cpu")
            .arg("--preset")
            .arg("v3-offline")
            .arg("--format")
            .arg("json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NEMO_SPEECH_MODEL_DIR", &self.model_dir);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;
            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("Failed to spawn nemo-speech at {:?}", self.binary_path))?;

        // Take the pipes so we can read them while waiting (and kill on timeout).
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Read stdout/stderr to completion in parallel with waiting on the process.
        // `child.wait()` takes `&mut self`, so the child handle stays with us and we
        // can kill it on timeout.
        let (stdout_bytes, stderr_bytes, status) =
            match tokio::time::timeout(
                DIARIZATION_TIMEOUT,
                async {
                    let (so, se, st) = tokio::join!(
                        Self::read_pipe(stdout),
                        Self::read_pipe(stderr),
                        child.wait(),
                    );
                    (so, se, st)
                },
            )
            .await
            {
                Ok((so, se, st)) => (so, se, st),
                Err(_) => {
                    // Timed out: kill the (still-running) process.
                    let _ = child.kill().await;
                    return Err(anyhow!(
                        "Diarization timed out after {:?}",
                        DIARIZATION_TIMEOUT
                    ));
                }
            };

        let status = status
            .map_err(|e| anyhow!("Failed to wait for nemo-speech: {}", e))?;

        let stderr = String::from_utf8_lossy(&stderr_bytes);
        if !stderr.trim().is_empty() {
            log::info!("nemo-speech stderr: {}", stderr.trim());
        }

        if !status.success() {
            return Err(anyhow!(
                "nemo-speech exited with {}: {}",
                status,
                stderr.trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let segments = Self::parse_json_from_stdout(&stdout)?;

        log::info!("Diarization complete: {} segments", segments.len());
        Ok(segments)
    }

    /// Read a pipe to completion (completes when the process exits and closes it).
    async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(mut pipe: Option<R>) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        if let Some(p) = pipe.as_mut() {
            let mut buf = Vec::new();
            let _ = p.read_to_end(&mut buf).await;
            buf
        } else {
            Vec::new()
        }
    }

    /// Extract the JSON object from the sidecar's stdout (which may include
    /// progress text) and deserialize the segments.
    fn parse_json_from_stdout(stdout: &str) -> Result<Vec<DiarizationSegment>> {
        let first_brace = stdout
            .find('{')
            .ok_or_else(|| anyhow!("No JSON object found in nemo-speech output"))?;
        let last_brace = stdout
            .rfind('}')
            .ok_or_else(|| anyhow!("No JSON object found in nemo-speech output"))?;

        if last_brace < first_brace {
            return Err(anyhow!("Malformed JSON in nemo-speech output"));
        }

        let json = &stdout[first_brace..=last_brace];
        let parsed: DiarizationOutput = serde_json::from_str(json)
            .with_context(|| format!("Failed to parse diarization JSON: {}", json))?;

        Ok(parsed.segments)
    }

    /// Resolve the path to the nemo-speech sidecar binary.
    fn resolve_binary() -> Result<PathBuf> {
        // 1. Environment variable override (dev / manual).
        if let Ok(env_path) = std::env::var("MEETILY_NEMO_SPEECH") {
            if !env_path.is_empty() {
                let path = PathBuf::from(env_path);
                if path.exists() {
                    log::info!("Using nemo-speech from MEETILY_NEMO_SPEECH: {}", path.display());
                    return Ok(path);
                }
            }
        }

        let target_triple = Self::target_triple();

        // 2. Relative to the current executable (bundled sidecar, most reliable).
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let binary_name = Self::sidecar_name(&target_triple);
                let bundled = exe_dir.join(&binary_name);
                if bundled.exists() {
                    log::info!("Found nemo-speech next to executable: {}", bundled.display());
                    return Ok(bundled);
                }

                // Fuzzy match in exe dir.
                if let Ok(entries) = std::fs::read_dir(exe_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("nemo-speech") && !name.ends_with(".d") {
                                log::info!("Found fuzzy match next to executable: {}", path.display());
                                return Ok(path);
                            }
                        }
                    }
                }
            }
        }

        // 3. Bundled resources (RESOURCE_DIR) fallback.
        if let Ok(resource_dir) = std::env::var("RESOURCE_DIR") {
            let resource_path = PathBuf::from(&resource_dir);
            let binary_name = Self::sidecar_name(&target_triple);
            let bundled = resource_path.join(&binary_name);
            if bundled.exists() {
                log::info!("Found nemo-speech in RESOURCE_DIR: {}", bundled.display());
                return Ok(bundled);
            }
            if let Ok(entries) = std::fs::read_dir(&resource_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("nemo-speech") && !name.ends_with(".d") {
                            log::info!("Found fuzzy match in RESOURCE_DIR: {}", path.display());
                            return Ok(path);
                        }
                    }
                }
            }
        }

        // 4. Dev fallback: relative to the workspace (no target triple in dev builds).
        if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
            let candidates = vec![
                PathBuf::from(&manifest_dir).join("binaries").join(Self::sidecar_name(&target_triple)),
                PathBuf::from(&manifest_dir).join("binaries").join("nemo-speech"),
                PathBuf::from(&manifest_dir).join("binaries").join("nemo-speech.exe"),
            ];
            for candidate in candidates {
                if candidate.exists() {
                    log::info!("Using dev nemo-speech: {}", candidate.display());
                    return Ok(candidate);
                }
            }
        }

        Err(anyhow!(
            "nemo-speech sidecar not found. Build it (see build-windows.yml) or set MEETILY_NEMO_SPEECH."
        ))
    }

    fn sidecar_name(target_triple: &str) -> String {
        if cfg!(windows) {
            format!("nemo-speech-{}.exe", target_triple)
        } else {
            format!("nemo-speech-{}", target_triple)
        }
    }

    fn target_triple() -> String {
        std::env::var("TARGET").unwrap_or_else(|_| {
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            { "x86_64-unknown-linux-gnu".to_string() }
            #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
            { "aarch64-unknown-linux-gnu".to_string() }
            #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
            { "x86_64-apple-darwin".to_string() }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            { "aarch64-apple-darwin".to_string() }
            #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
            { "x86_64-pc-windows-msvc".to_string() }
            #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
            { "aarch64-pc-windows-msvc".to_string() }
            #[cfg(not(any(
                all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
                all(target_os = "macos", any(target_arch = "x86_64", target_arch = "aarch64")),
                all(target_os = "windows", any(target_arch = "x86_64", target_arch = "aarch64"))
            )))]
            { "unknown".to_string() }
        })
    }
}

// ============================================================================
// Global Diarization Manager (init/get lifecycle, mirrors the summary sidecar)
// ============================================================================

lazy_static::lazy_static! {
    static ref DIARIZATION_MANAGER: Mutex<Option<Arc<DiarizationManager>>> =
        Mutex::new(None);
}

/// Initialize the global diarization manager.
pub async fn init_diarization_manager(app_data_dir: &PathBuf) -> Result<()> {
    let manager = DiarizationManager::new(app_data_dir)?;
    let mut global = DIARIZATION_MANAGER.lock().await;
    *global = Some(Arc::new(manager));
    Ok(())
}

/// Get (or lazily initialize) the global diarization manager.
async fn get_diarization_manager(app_data_dir: &PathBuf) -> Result<Arc<DiarizationManager>> {
    let global = DIARIZATION_MANAGER.lock().await;
    if let Some(manager) = global.as_ref() {
        return Ok(manager.clone());
    }
    drop(global);
    init_diarization_manager(app_data_dir).await?;
    let global = DIARIZATION_MANAGER.lock().await;
    global
        .clone()
        .ok_or_else(|| anyhow!("Diarization manager not initialized"))
}

/// Run diarization using the global manager.
pub async fn run_diarization(
    app_data_dir: &PathBuf,
    wav_path: &PathBuf,
    model_path: &PathBuf,
) -> Result<Vec<DiarizationSegment>> {
    let manager = get_diarization_manager(app_data_dir).await?;
    manager.run(wav_path, model_path).await
}
