// Diarization model (GGUF) download with progress + integrity verification.
//
// Mirrors the summary engine's model_manager download pattern (streamed download
// with a progress callback), but for a single fixed model with a hard size +
// sha256 check. Fails loudly on any mismatch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

/// The Nemotron-3-Diarization GGUF model constants (verified in Phase 0/1).
pub const MODEL_URL: &str =
    "https://huggingface.co/nvidia/Nemotron-3-Diarization/resolve/main/Nemotron-3-Diarization.q8_0.gguf";
pub const MODEL_SIZE: u64 = 107012128;
pub const MODEL_SHA256: &str =
    "08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1";
pub const MODEL_FILENAME: &str = "Nemotron-3-Diarization.q8_0.gguf";

/// Download progress payload (emitted as a Tauri event / passed to a callback).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiarizationDownloadProgress {
    /// Bytes downloaded so far.
    pub downloaded_bytes: u64,
    /// Total file size in bytes.
    pub total_bytes: u64,
    /// Percentage complete (0-100).
    pub percent: u8,
}

/// The path where the model GGUF is stored.
pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(MODEL_FILENAME)
}

/// Check whether the model is present and valid (file exists + size + sha256).
pub fn is_model_downloaded(models_dir: &Path) -> bool {
    let path = model_path(models_dir);
    if !path.is_file() {
        return false;
    }
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() != MODEL_SIZE {
        return false;
    }
    sha256_file(&path).map(|h| h == MODEL_SHA256).unwrap_or(false)
}

/// Compute the lowercase hex sha256 of a file.
fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let digest = Sha256::digest(&data);
    Ok(hex_digest(&digest))
}

fn hex_digest(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Download the model into `models_dir`, streaming progress through the callback.
///
/// Downloads to a `.part` file, then verifies size + sha256 before promoting it
/// to the final name. Any mismatch deletes the file and returns an error.
pub async fn download_model(
    models_dir: &Path,
    progress_callback: Option<Box<dyn Fn(DiarizationDownloadProgress) + Send>>,
) -> Result<()> {
    if !models_dir.exists() {
        fs::create_dir_all(models_dir).await?;
    }

    let final_path = model_path(models_dir);
    let part_path = final_path.with_extension("gguf.part");

    // Skip if already valid.
    if is_model_downloaded(models_dir) {
        log::info!("Diarization model already present and valid, skipping download");
        if let Some(cb) = &progress_callback {
            cb(DiarizationDownloadProgress {
                downloaded_bytes: MODEL_SIZE,
                total_bytes: MODEL_SIZE,
                percent: 100,
            });
        }
        return Ok(());
    }

    // Remove a stale partial file.
    if part_path.exists() {
        let _ = fs::remove_file(&part_path).await;
    }

    log::info!("Downloading diarization model from {}", MODEL_URL);

    let client = Client::builder()
        .tcp_nodelay(true)
        .pool_max_idle_per_host(1)
        .timeout(Duration::from_secs(3600))
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(MODEL_URL)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to start download: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow!("Download failed with status: {}", response.status()));
    }

    let total_size = response.content_length().unwrap_or(MODEL_SIZE);

    let file = fs::File::create(&part_path)
        .await
        .map_err(|e| anyhow!("Failed to create file: {}", e))?;

    use tokio::io::BufWriter;
    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, file);

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_report = std::time::Instant::now();

    let mut stream = response.bytes_stream();

    while let Some(chunk) = timeout(Duration::from_secs(60), stream.next())
        .await
        .map_err(|_| anyhow!("Download stalled (no data for 60s"))?
    {
        let chunk = chunk.map_err(|e| anyhow!("Stream error: {}", e))?;
        hasher.update(&chunk);
        writer
            .write_all(&chunk)
            .await
            .map_err(|e| anyhow!("Error writing to file: {}", e))?;
        downloaded += chunk.len() as u64;

        let percent = if total_size > 0 {
            ((downloaded as f64 / total_size as f64) * 100.0).min(100.0) as u8
        } else {
            0
        };

        let is_complete = downloaded >= total_size;
        if is_complete || last_report.elapsed().as_millis() >= 250 {
            if let Some(cb) = &progress_callback {
                cb(DiarizationDownloadProgress {
                    downloaded_bytes: downloaded,
                    total_bytes: total_size,
                    percent,
                });
            }
            last_report = std::time::Instant::now();
        }
    }

    writer.flush().await?;
    drop(writer);

    // Verify size + sha256 before promoting.
    let meta = fs::metadata(&part_path).await?;
    if meta.len() != MODEL_SIZE {
        let _ = fs::remove_file(&part_path).await;
        return Err(anyhow!(
            "Model size mismatch: got {} bytes, expected {}",
            meta.len(),
            MODEL_SIZE
        ));
    }

    let digest = Sha256::finalize(hasher);
    let hash = hex_digest(&digest);
    if hash != MODEL_SHA256 {
        let _ = fs::remove_file(&part_path).await;
        return Err(anyhow!(
            "Model sha256 mismatch: got {}, expected {}",
            hash,
            MODEL_SHA256
        ));
    }

    // Promote to the final name.
    if final_path.exists() {
        fs::remove_file(&final_path).await?;
    }
    fs::rename(&part_path, &final_path).await?;

    log::info!("Diarization model downloaded and verified: {}", final_path.display());

    if let Some(cb) = &progress_callback {
        cb(DiarizationDownloadProgress {
            downloaded_bytes: MODEL_SIZE,
            total_bytes: MODEL_SIZE,
            percent: 100,
        });
    }

    Ok(())
}
