//! Portable llama.cpp runtime: downloaded once from the official GitHub releases into LumenData/runtime.
//! Windows: the `win-<backend>-x64.zip` builds; Linux: `ubuntu-<backend>-x64.tar.gz`; macOS (Apple
//! Silicon): `macos-arm64.tar.gz`, which runs on the GPU through Metal.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::state::{AppState, LUMEN_UA};

/// llama-server's file name on this system.
const SERVER: &str = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };

/// The backend actually used: macOS has one build (Metal), so any GPU choice means "metal" there.
pub fn effective_backend(backend: &str) -> String {
    if cfg!(target_os = "macos") && backend != "cpu" {
        "metal".into()
    } else {
        backend.to_string()
    }
}

pub fn runtime_dir(state: &AppState, backend: &str) -> PathBuf {
    state.paths.runtime.join(format!("llama-{}", effective_backend(backend)))
}

pub fn server_exe(state: &AppState, backend: &str) -> PathBuf {
    runtime_dir(state, backend).join(SERVER)
}

pub fn installed_version(state: &AppState, backend: &str) -> Option<String> {
    let dir = runtime_dir(state, backend);
    if !dir.join(SERVER).exists() {
        return None;
    }
    Some(std::fs::read_to_string(dir.join("version.txt")).unwrap_or_else(|_| "unknown".into()).trim().to_string())
}

/// Is `name` the llama.cpp release file for this system and backend?
fn asset_matches(name: &str, backend: &str) -> bool {
    if !name.starts_with("llama-") {
        return false;
    }
    if cfg!(windows) {
        let part = match backend {
            "rocm" => "-bin-win-rocm",
            "cpu" => "-bin-win-cpu-x64",
            _ => "-bin-win-vulkan-x64",
        };
        name.contains(part) && name.ends_with(".zip") && name.contains("x64")
    } else if cfg!(target_os = "macos") {
        // (the same Metal build also runs CPU-only with --gpu-layers 0)
        name.ends_with("-bin-macos-arm64.tar.gz")
    } else {
        match backend {
            "rocm" => name.contains("-bin-ubuntu-rocm-") && name.ends_with("-x64.tar.gz"),
            "cpu" => name.ends_with("-bin-ubuntu-x64.tar.gz"),
            _ => name.ends_with("-bin-ubuntu-vulkan-x64.tar.gz"),
        }
    }
}

/// Newest llama.cpp release that has a build for this system and `backend`: (tag, url, file name).
pub async fn latest_release(state: &AppState, backend: &str) -> Result<(String, String, String)> {
    let releases: Value = state
        .http
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20")
        .header("User-Agent", LUMEN_UA)
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let backend = effective_backend(backend);
    releases
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            r["assets"].as_array()?.iter().find_map(|a| {
                let n = a["name"].as_str()?;
                asset_matches(n, &backend).then(|| (tag.clone(), a["browser_download_url"].as_str().unwrap_or_default().to_string(), n.to_string()))
            })
        })
        .ok_or_else(|| anyhow!("no llama.cpp release has a build for backend '{backend}' on this system"))
}

/// Unpacks a llama.cpp archive (.zip or .tar.gz) into `dest`, keeping the executable bits.
fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(archive)?;
    if archive.to_string_lossy().ends_with(".tar.gz") {
        tar::Archive::new(flate2::read::GzDecoder::new(f)).unpack(dest)?;
    } else {
        zip::ZipArchive::new(f)?.extract(dest)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // (zip entries may carry no Unix mode: make the tools runnable either way)
        if let Some(dir) = find_server_dir(dest) {
            for e in std::fs::read_dir(dir)?.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("llama-") && !n.contains('.') {
                    let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
    Ok(())
}

/// Replaces the runtime with the newest build (stops the servers first, reloads the model after).
/// Returns the installed version.
pub async fn update(app: &AppHandle) -> Result<String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let backend = effective_backend(&state.settings.read().backend);
    let model = state.info.read().model.clone();
    // the runtime folder is replaced, so both servers must release their exe first
    crate::server::stop_all(app).await;
    crate::server::stop_llm(app).await; // resets the status shown in the UI
    ensure(app, &backend, true).await?;
    let version = installed_version(&state, &backend);
    crate::updater::runtime_updated(app, version.clone());
    if let Some(m) = model {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::server::start_llm(&app2, &m).await;
        });
    }
    Ok(version.unwrap_or_default())
}

/// Download (or update) the llama.cpp build for the chosen backend.
pub async fn ensure(app: &AppHandle, backend: &str, force: bool) -> Result<PathBuf> {
    let state = app.state::<Arc<AppState>>();
    let backend = &effective_backend(backend);
    let exe = server_exe(&state, backend);
    if exe.exists() && !force {
        return Ok(exe);
    }
    state.log(format!("[runtime] fetching llama.cpp ({backend}) release list"));
    let (tag, url, name) = latest_release(&state, backend).await?;

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
        let archive = tmp_zip.clone();
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || unpack(&archive, &staging)).await??;
    }
    // Some archives nest files in a folder: find the directory that holds llama-server
    let root = find_server_dir(&staging).ok_or_else(|| anyhow!("{SERVER} missing from the llama.cpp archive"))?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&root, &dir).or_else(|_| copy_dir(&root, &dir))?;
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&tmp_zip);
    std::fs::write(dir.join("version.txt"), &tag)?;
    state.log(format!("[runtime] installed llama.cpp {tag} ({backend})"));
    Ok(dir.join(SERVER))
}

fn find_server_dir(dir: &std::path::Path) -> Option<PathBuf> {
    if dir.join(SERVER).exists() {
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
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--list-devices");
    crate::platform::quiet(&mut cmd);
    #[cfg(target_os = "linux")]
    if let Some(dir) = exe.parent() {
        cmd.env("LD_LIBRARY_PATH", dir);
    }
    let out = cmd.output().await.ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let re = regex::Regex::new(r"(?m)^\s*\w+\d+:\s*(.+?)\s*\((\d+)\s*MiB").ok()?;
    let c = re.captures(&text)?;
    Some((c[1].to_string(), c[2].parse::<u64>().ok()? * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real file names of llama.cpp release b12179: exactly one matches per system and backend.
    #[test]
    fn picks_this_systems_llama_cpp_build() {
        let names = [
            "llama-b12179-bin-win-vulkan-x64.zip",
            "llama-b12179-bin-win-cpu-x64.zip",
            "llama-b12179-bin-win-cpu-arm64.zip",
            "cudart-llama-b12179-bin-ubuntu-cuda-12.8-x64.tar.gz",
            "llama-b12179-bin-macos-arm64.tar.gz",
            "llama-b12179-bin-macos-x64.tar.gz",
            "llama-b12179-bin-ubuntu-arm64.tar.gz",
            "llama-b12179-bin-ubuntu-rocm-10.0-x64.tar.gz",
            "llama-b12179-bin-ubuntu-vulkan-arm64.tar.gz",
            "llama-b12179-bin-ubuntu-vulkan-x64.tar.gz",
            "llama-b12179-bin-ubuntu-x64.tar.gz",
        ];
        let pick = |backend: &str| -> Vec<&str> { names.iter().copied().filter(|n| asset_matches(n, &effective_backend(backend))).collect() };
        if cfg!(windows) {
            assert_eq!(pick("vulkan"), ["llama-b12179-bin-win-vulkan-x64.zip"]);
            assert_eq!(pick("cpu"), ["llama-b12179-bin-win-cpu-x64.zip"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(effective_backend("vulkan"), "metal");
            assert_eq!(pick("vulkan"), ["llama-b12179-bin-macos-arm64.tar.gz"]);
        } else {
            assert_eq!(pick("vulkan"), ["llama-b12179-bin-ubuntu-vulkan-x64.tar.gz"]);
            assert_eq!(pick("cpu"), ["llama-b12179-bin-ubuntu-x64.tar.gz"]);
            assert_eq!(pick("rocm"), ["llama-b12179-bin-ubuntu-rocm-10.0-x64.tar.gz"]);
        }
    }
}
