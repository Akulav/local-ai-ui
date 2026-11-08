//! Portable llama.cpp runtime: downloaded once from the official GitHub releases into LumenData/runtime.
//! Windows: the `win-<backend>-x64.zip` builds; Linux: `ubuntu-<backend>-x64.tar.gz`; macOS (Apple
//! Silicon): `macos-arm64.tar.gz`, which runs on the GPU through Metal.
//! CUDA (NVIDIA, Windows + Linux) comes as two files: the build (`…-bin-win-cuda-12.4-x64.zip`) and NVIDIA's
//! runtime libraries (`cudart-…` with the same suffix), which go next to llama-server. CUDA 12 or 13 is
//! picked from what `nvidia-smi` reports (`cuda_major`).

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

/// Is `name` the llama.cpp release file for this system and build (`backend`, or `cuda-12` / `cuda-13`)?
fn asset_matches(name: &str, build: &str) -> bool {
    if !name.starts_with("llama-") {
        return false;
    }
    if cfg!(windows) {
        let part = match build {
            "rocm" => "-bin-win-rocm",
            "cpu" => "-bin-win-cpu-x64",
            "cuda-12" => "-bin-win-cuda-12.",
            "cuda-13" => "-bin-win-cuda-13.",
            _ => "-bin-win-vulkan-x64",
        };
        name.contains(part) && name.ends_with(".zip") && name.contains("x64")
    } else if cfg!(target_os = "macos") {
        // (the same Metal build also runs CPU-only with --gpu-layers 0)
        name.ends_with("-bin-macos-arm64.tar.gz")
    } else {
        match build {
            "rocm" => name.contains("-bin-ubuntu-rocm-") && name.ends_with("-x64.tar.gz"),
            "cpu" => name.ends_with("-bin-ubuntu-x64.tar.gz"),
            "cuda-12" | "cuda-13" => name.contains(&format!("-bin-ubuntu-{build}.")) && name.ends_with("-x64.tar.gz"),
            _ => name.ends_with("-bin-ubuntu-vulkan-x64.tar.gz"),
        }
    }
}

/// NVIDIA's runtime libraries for a CUDA build: the `cudart-…` file of the same release with the same
/// ending (`cudart-llama-bin-win-cuda-12.4-x64.zip` for `llama-b123-bin-win-cuda-12.4-x64.zip`).
fn cudart_matches(name: &str, build_file: &str, tag: &str) -> bool {
    let Some(suffix) = build_file.strip_prefix(&format!("llama-{tag}")) else { return false };
    name.starts_with("cudart-") && name.ends_with(suffix)
}

/// CUDA build for this NVIDIA setup: 13.x needs a driver that supports CUDA 13 and a Turing or newer GPU
/// (compute capability 7.5+; CUDA 13 dropped Maxwell, Pascal and Volta), anything else gets 12.x.
fn cuda_major(driver_cuda: u32, lowest_cap: f32) -> u32 {
    if driver_cuda >= 13 && lowest_cap >= 7.5 { 13 } else { 12 }
}

/// (CUDA version the driver supports, lowest compute capability of the NVIDIA GPUs) from `nvidia-smi`.
fn parse_nvidia_smi(summary: &str, caps: &str) -> Option<(u32, f32)> {
    let driver = regex::Regex::new(r"CUDA Version:\s*(\d+)").ok()?.captures(summary)?[1].parse().ok()?;
    let cap = caps.lines().filter_map(|l| l.trim().parse::<f32>().ok()).fold(None, |m: Option<f32>, c| Some(m.map_or(c, |m| m.min(c))));
    Some((driver, cap.unwrap_or(0.0)))
}

async fn nvidia_smi(args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("nvidia-smi");
    cmd.args(args);
    crate::platform::quiet(&mut cmd);
    let out = cmd.output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The build to download for `backend`: itself, or for CUDA `cuda-12` / `cuda-13` after asking the driver.
/// Fails early (before a ~600 MB download) without an NVIDIA driver or with one too old for CUDA 12.
async fn build_for(state: &AppState, backend: &str) -> Result<String> {
    if backend != "cuda" {
        return Ok(backend.to_string());
    }
    let summary = nvidia_smi(&[]).await.ok_or_else(|| {
        anyhow!("CUDA needs an NVIDIA graphics card and its driver (nvidia-smi was not found). Pick Vulkan in Settings for other cards.")
    })?;
    let caps = nvidia_smi(&["--query-gpu=compute_cap", "--format=csv,noheader"]).await.unwrap_or_default();
    let (driver, cap) = parse_nvidia_smi(&summary, &caps).ok_or_else(|| anyhow!("could not read the CUDA version from nvidia-smi"))?;
    if driver < 12 {
        return Err(anyhow!("the NVIDIA driver supports only CUDA {driver}; CUDA 12 or newer is needed - update the driver, or pick Vulkan in Settings"));
    }
    let major = cuda_major(driver, cap);
    state.log(format!("[runtime] NVIDIA driver supports CUDA {driver}, lowest compute capability {cap} -> CUDA {major} build"));
    Ok(format!("cuda-{major}"))
}

/// A llama.cpp download: the build, and for CUDA NVIDIA's runtime libraries.
pub struct Release {
    pub tag: String,
    pub url: String,
    pub name: String,
    /// (url, file name) of the `cudart-…` file
    pub cudart: Option<(String, String)>,
}

/// Newest llama.cpp release that has a build for this system and `backend` (a settings value).
pub async fn latest_release(state: &AppState, backend: &str) -> Result<Release> {
    let build = build_for(state, &effective_backend(backend)).await?;
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
    releases
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|r| pick_release(r, &build))
        .ok_or_else(|| anyhow!("no llama.cpp release has a build for '{build}' on this system"))
}

/// The files of one GitHub release for `build`; None if it lacks one (a CUDA build also needs its cudart file).
fn pick_release(r: &Value, build: &str) -> Option<Release> {
    let tag = r["tag_name"].as_str()?.to_string();
    let assets = r["assets"].as_array()?;
    let file = |a: &Value| Some((a["browser_download_url"].as_str()?.to_string(), a["name"].as_str()?.to_string()));
    let (url, name) = assets.iter().filter_map(file).find(|(_, n)| asset_matches(n, build))?;
    let cudart = if build.starts_with("cuda") {
        Some(assets.iter().filter_map(file).find(|(_, n)| cudart_matches(n, &name, &tag))?)
    } else {
        None
    };
    Some(Release { tag, url, name, cudart })
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
    let Release { tag, url, name, cudart } = latest_release(&state, backend).await?;

    let tmp_zip = state.paths.runtime.join(&name);
    let cancel = Arc::new(AtomicBool::new(false));
    crate::download::download(app, &state.http, "runtime", &format!("llama.cpp {tag} ({backend})"), &url, &tmp_zip, cancel.clone())
        .await
        .context("downloading llama.cpp")?;
    let tmp_cudart = match &cudart {
        Some((url, name)) => {
            let path = state.paths.runtime.join(name);
            crate::download::download(app, &state.http, "runtime", "NVIDIA CUDA runtime", url, &path, cancel)
                .await
                .context("downloading the CUDA runtime")?;
            Some(path)
        }
        None => None,
    };

    let dir = runtime_dir(&state, backend);
    let staging = state.paths.runtime.join(format!("staging-{backend}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let root = {
        let (archive, cudart, staging) = (tmp_zip.clone(), tmp_cudart.clone(), staging.clone());
        tokio::task::spawn_blocking(move || unpack_build(&archive, cudart.as_deref(), &staging)).await??
    };
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&root, &dir).or_else(|_| copy_dir(&root, &dir))?;
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_file(&tmp_zip);
    if let Some(c) = tmp_cudart {
        let _ = std::fs::remove_file(c);
    }
    std::fs::write(dir.join("version.txt"), &tag)?;
    state.log(format!("[runtime] installed llama.cpp {tag} ({name})"));
    Ok(dir.join(SERVER))
}

/// Unpacks a build (and NVIDIA's runtime libraries, for CUDA) under `staging`; returns the folder that holds
/// llama-server with everything it needs (some archives nest their files in a folder).
fn unpack_build(build: &Path, cudart: Option<&Path>, staging: &Path) -> Result<PathBuf> {
    unpack(build, &staging.join("build"))?;
    let root = find_server_dir(&staging.join("build")).ok_or_else(|| anyhow!("{SERVER} missing from the llama.cpp archive"))?;
    if let Some(c) = cudart {
        unpack(c, &staging.join("cudart"))?;
        copy_files_flat(&staging.join("cudart"), &root)?;
    }
    Ok(root)
}

/// Copies every file under `from` (any depth) straight into `to`, keeping files already there: NVIDIA's
/// libraries must sit next to llama-server (Windows DLL search, Linux LD_LIBRARY_PATH).
fn copy_files_flat(from: &Path, to: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_files_flat(&p, to)?;
        } else {
            let target = to.join(e.file_name());
            if !target.exists() {
                std::fs::copy(&p, target)?;
            }
        }
    }
    Ok(())
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

    /// Asset list of llama.cpp release b12194 (2026-11-08), the CUDA part and its neighbours.
    const B12194: &[&str] = &[
        "cudart-llama-b12194-bin-ubuntu-cuda-12.8-x64.tar.gz",
        "cudart-llama-b12194-bin-ubuntu-cuda-13.5-arm64.tar.gz",
        "cudart-llama-b12194-bin-ubuntu-cuda-13.5-x64.tar.gz",
        "cudart-llama-bin-win-cuda-12.4-x64.zip",
        "cudart-llama-bin-win-cuda-13.5-arm64.zip",
        "cudart-llama-bin-win-cuda-13.5-x64.zip",
        "llama-b12194-bin-ubuntu-cuda-12.8-x64.tar.gz",
        "llama-b12194-bin-ubuntu-cuda-13.5-arm64.tar.gz",
        "llama-b12194-bin-ubuntu-cuda-13.5-x64.tar.gz",
        "llama-b12194-bin-ubuntu-rocm-10.0-x64.tar.gz",
        "llama-b12194-bin-ubuntu-vulkan-x64.tar.gz",
        "llama-b12194-bin-ubuntu-x64.tar.gz",
        "llama-b12194-bin-win-cpu-x64.zip",
        "llama-b12194-bin-win-cuda-12.4-x64.zip",
        "llama-b12194-bin-win-cuda-13.5-arm64.zip",
        "llama-b12194-bin-win-cuda-13.5-x64.zip",
        "llama-b12194-bin-win-rocm-10.0-x64.zip",
        "llama-b12194-bin-win-vulkan-x64.zip",
    ];

    fn release_json(names: &[&str]) -> Value {
        let assets: Vec<Value> = names.iter().map(|n| serde_json::json!({"name": n, "browser_download_url": format!("https://x/{n}")})).collect();
        serde_json::json!({"tag_name": "b12194", "assets": assets})
    }

    /// A CUDA build comes with the NVIDIA runtime file of the same flavour; other builds come alone.
    #[test]
    fn cuda_build_comes_with_its_runtime() {
        let r = release_json(B12194);
        let files = |build: &str| pick_release(&r, build).map(|r| (r.name, r.cudart.map(|c| c.1)));
        if cfg!(windows) {
            assert_eq!(files("cuda-12"), Some(("llama-b12194-bin-win-cuda-12.4-x64.zip".into(), Some("cudart-llama-bin-win-cuda-12.4-x64.zip".into()))));
            assert_eq!(files("cuda-13"), Some(("llama-b12194-bin-win-cuda-13.5-x64.zip".into(), Some("cudart-llama-bin-win-cuda-13.5-x64.zip".into()))));
            assert_eq!(files("vulkan"), Some(("llama-b12194-bin-win-vulkan-x64.zip".into(), None)));
        } else if cfg!(target_os = "linux") {
            assert_eq!(files("cuda-12"), Some(("llama-b12194-bin-ubuntu-cuda-12.8-x64.tar.gz".into(), Some("cudart-llama-b12194-bin-ubuntu-cuda-12.8-x64.tar.gz".into()))));
            assert_eq!(files("cuda-13"), Some(("llama-b12194-bin-ubuntu-cuda-13.5-x64.tar.gz".into(), Some("cudart-llama-b12194-bin-ubuntu-cuda-13.5-x64.tar.gz".into()))));
            assert_eq!(files("vulkan"), Some(("llama-b12194-bin-ubuntu-vulkan-x64.tar.gz".into(), None)));
        }
        if !cfg!(target_os = "macos") {
            // a release whose runtime file is missing is skipped, not installed half
            let partial: Vec<&str> = B12194.iter().copied().filter(|n| !n.starts_with("cudart-")).collect();
            assert!(pick_release(&release_json(&partial), "cuda-12").is_none());
            assert!(pick_release(&release_json(&partial), "vulkan").is_some());
        }
    }

    /// Real CUDA 12 install from GitHub's newest release (~650 MB on Windows, ~750 MB on Linux; kept in
    /// `LUMEN_TEST_CUDA_DIR` for reruns): both files are found, unpacked into one folder, NVIDIA's
    /// libraries sit next to llama-server, and llama-server starts. Works without an NVIDIA card.
    /// `LUMEN_TEST_CUDA_DIR=… cargo test live_cuda_install -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_cuda_install() {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let cache = PathBuf::from(std::env::var("LUMEN_TEST_CUDA_DIR").expect("set LUMEN_TEST_CUDA_DIR"));
        std::fs::create_dir_all(&cache).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let http = reqwest::Client::builder().user_agent(LUMEN_UA).build().unwrap();
        let rel = rt.block_on(async {
            let list: Value = http.get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=5").send().await.unwrap().json().await.unwrap();
            let rel = list.as_array().unwrap().iter().find_map(|r| pick_release(r, "cuda-12")).expect("a release with a CUDA 12 build");
            let mut files = vec![(rel.url.clone(), rel.name.clone())];
            files.extend(rel.cudart.clone());
            for (url, name) in files {
                let path = cache.join(&name);
                if path.exists() {
                    continue;
                }
                println!("downloading {name}");
                let mut out = tokio::fs::File::create(cache.join("partial")).await.unwrap();
                let mut body = http.get(&url).send().await.unwrap().error_for_status().unwrap().bytes_stream();
                while let Some(chunk) = body.next().await {
                    out.write_all(&chunk.unwrap()).await.unwrap();
                }
                out.flush().await.unwrap();
                drop(out);
                std::fs::rename(cache.join("partial"), &path).unwrap();
            }
            rel
        });
        println!("{} + {:?}", rel.name, rel.cudart.as_ref().map(|c| &c.1));
        let staging = cache.join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        let cudart = cache.join(&rel.cudart.as_ref().unwrap().1);
        let root = unpack_build(&cache.join(&rel.name), Some(&cudart), &staging).unwrap();
        let names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        let has = |p: &str| names.iter().any(|n| n.starts_with(p));
        assert!(names.iter().any(|n| n == SERVER));
        assert!(has("ggml-cuda") || has("libggml-cuda"), "the CUDA backend: {names:?}");
        assert!(has("cudart64_12") || has("libcudart.so.12"), "NVIDIA's runtime next to llama-server: {names:?}");
        assert!(has("cublas64_12") || has("libcublas.so.12"), "cuBLAS next to llama-server: {names:?}");
        let mut cmd = std::process::Command::new(root.join(SERVER));
        cmd.arg("--version");
        if cfg!(target_os = "linux") {
            cmd.env("LD_LIBRARY_PATH", &root);
        }
        let out = cmd.output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        println!("{text}");
        assert!(out.status.success() && text.contains("version"), "llama-server starts");
        let _ = std::fs::remove_dir_all(&staging);
    }

    #[test]
    fn cuda_flavour_follows_driver_and_gpu() {
        let summary = "| NVIDIA-SMI 581.42                 Driver Version: 581.42         CUDA Version: 13.0     |";
        assert_eq!(parse_nvidia_smi(summary, "8.9\n"), Some((13, 8.9)));
        assert_eq!(parse_nvidia_smi(summary, "8.9\n6.1\n"), Some((13, 6.1)), "the oldest card decides");
        assert_eq!(parse_nvidia_smi("no header here", "8.9"), None);
        assert_eq!(cuda_major(13, 8.9), 13); // RTX 40 on a new driver
        assert_eq!(cuda_major(13, 12.0), 13); // RTX 50
        assert_eq!(cuda_major(13, 6.1), 12); // GTX 10xx: CUDA 13 dropped Pascal
        assert_eq!(cuda_major(12, 8.9), 12); // driver too old for 13
        assert_eq!(cuda_major(13, 0.0), 12); // capability unknown: the safe choice
    }
}
