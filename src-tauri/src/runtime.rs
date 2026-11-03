//! Portable llama.cpp runtime: downloaded once from the official GitHub releases into LumenData/runtime.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

pub fn runtime_dir(state: &AppState, backend: &str) -> PathBuf {
    state.paths.runtime.join(format!("llama-{backend}"))
}

pub fn server_exe(state: &AppState, backend: &str) -> PathBuf {
    runtime_dir(state, backend).join("llama-server.exe")
}

pub fn installed_version(state: &AppState, backend: &str) -> Option<String> {
    let dir = runtime_dir(state, backend);
    if !dir.join("llama-server.exe").exists() {
        return None;
    }
    Some(std::fs::read_to_string(dir.join("version.txt")).unwrap_or_else(|_| "unknown".into()).trim().to_string())
}

fn asset_suffix(backend: &str) -> &'static str {
    match backend {
        "rocm" => "-bin-win-rocm",
        "cpu" => "-bin-win-cpu-x64.zip",
        _ => "-bin-win-vulkan-x64.zip",
    }
}

/// Download (or update) the llama.cpp build for the chosen backend.
pub async fn ensure(app: &AppHandle, backend: &str, force: bool) -> Result<PathBuf> {
    let state = app.state::<Arc<AppState>>();
    let exe = server_exe(&state, backend);
    if exe.exists() && !force {
        return Ok(exe);
    }
    state.log(format!("[runtime] fetching llama.cpp ({backend}) release list"));
    let releases: Value = state
        .http
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let suffix = asset_suffix(backend);
    let (tag, url, name) = releases
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            r["assets"].as_array()?.iter().find_map(|a| {
                let n = a["name"].as_str()?;
                let ok = n.starts_with("llama-") && n.contains(suffix) && n.ends_with(".zip") && n.contains("x64");
                ok.then(|| (tag.clone(), a["browser_download_url"].as_str().unwrap_or_default().to_string(), n.to_string()))
            })
        })
        .ok_or_else(|| anyhow!("no llama.cpp release asset found for backend '{backend}'"))?;

    let tmp_zip = state.paths.runtime.join(&name);
    let cancel = Arc::new(AtomicBool::new(false));
    crate::download::download(app, &state.http, "runtime", &format!("llama.cpp {tag} ({backend})"), &url, &tmp_zip, cancel)
        .await
        .context("downloading llama.cpp")?;

    let dir = runtime_dir(&state, backend);
    let staging = state.paths.runtime.join(format!("staging-{backend}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    {
        let zip_path = tmp_zip.clone();
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let f = std::fs::File::open(&zip_path)?;
            let mut z = zip::ZipArchive::new(f)?;
            z.extract(&staging)?;
            Ok(())
        })
        .await??;
    }
    // Some archives nest files in a folder: find the directory that holds llama-server.exe
    let root = find_server_dir(&staging).ok_or_else(|| anyhow!("llama-server.exe missing from archive"))?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&root, &dir).or_else(|_| copy_dir(&root, &dir))?;
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&tmp_zip);
    std::fs::write(dir.join("version.txt"), &tag)?;
    state.log(format!("[runtime] installed llama.cpp {tag} ({backend})"));
    Ok(dir.join("llama-server.exe"))
}

fn find_server_dir(dir: &std::path::Path) -> Option<PathBuf> {
    if dir.join("llama-server.exe").exists() {
        return Some(dir.to_path_buf());
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.path().is_dir() {
            if let Some(p) = find_server_dir(&e.path()) {
                return Some(p);
            }
        }
    }
    None
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let t = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &t)?;
        } else {
            std::fs::copy(e.path(), t)?;
        }
    }
    Ok(())
}

/// Parse `llama-server --list-devices` -> (gpu name, total VRAM bytes)
pub async fn detect_gpu(exe: &std::path::Path) -> Option<(String, u64)> {
    let out = tokio::process::Command::new(exe)
        .arg("--list-devices")
        .creation_flags(0x08000000)
        .output()
        .await
        .ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let re = regex::Regex::new(r"(?m)^\s*\w+\d+:\s*(.+?)\s*\((\d+)\s*MiB").ok()?;
    let c = re.captures(&text)?;
    Some((c[1].to_string(), c[2].parse::<u64>().ok()? * 1024 * 1024))
}
