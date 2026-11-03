//! Resumable HTTP downloads used for models, the embedding model and the llama.cpp runtime.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

/// Resumable download to `dest` (via `dest.part`). Emits `download-progress` events tagged with `id`.
pub async fn download(
    app: &AppHandle,
    http: &reqwest::Client,
    id: &str,
    label: &str,
    url: &str,
    dest: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);

    let mut req = http.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.send().await?;
    let status = resp.status();
    if status.as_u16() == 416 {
        // already complete
        tokio::fs::rename(&part, dest).await?;
        return Ok(());
    }
    if !status.is_success() {
        return Err(anyhow!("HTTP {status} for {url}"));
    }
    if status.as_u16() != 206 {
        have = 0; // server ignored range
    }
    let total = resp.content_length().map(|l| l + have).unwrap_or(0);

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(&part)
        .await?;

    let mut stream = resp.bytes_stream();
    let mut done = have;
    let started = Instant::now();
    let mut last_emit = Instant::now();
    let mut speed_mark = (Instant::now(), done);
    let mut speed = 0.0f64;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            file.flush().await.ok();
            let _ = app.emit("download-progress", json!({"id": id, "label": label, "cancelled": true}));
            return Err(anyhow!("cancelled"));
        }
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if last_emit.elapsed().as_millis() > 300 {
            let dt = speed_mark.0.elapsed().as_secs_f64();
            if dt > 0.8 {
                let inst = (done - speed_mark.1) as f64 / dt;
                speed = if speed == 0.0 { inst } else { speed * 0.6 + inst * 0.4 };
                speed_mark = (Instant::now(), done);
            }
            let _ = app.emit(
                "download-progress",
                json!({"id": id, "label": label, "done": done, "total": total, "speed": speed, "elapsed": started.elapsed().as_secs()}),
            );
            last_emit = Instant::now();
        }
    }
    file.flush().await?;
    drop(file);
    if total > 0 && done < total {
        return Err(anyhow!("download interrupted ({done}/{total} bytes) - try again to resume"));
    }
    tokio::fs::rename(&part, dest).await?;
    let _ = app.emit("download-progress", json!({"id": id, "label": label, "done": done, "total": total, "finished": true}));
    Ok(())
}
