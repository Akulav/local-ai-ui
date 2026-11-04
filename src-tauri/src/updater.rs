//! Updates: checks GitHub for a newer Lumen release and a newer llama.cpp build (in the background
//! and on demand), and installs a Lumen release after verifying it against the release's SHA256SUMS.txt.
//! Portable copies swap their exe; installed copies (uninstall.exe next to the exe) run the NSIS installer.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{now_ms, AppState, LUMEN_UA};

/// GitHub repository the releases come from.
pub const REPO: &str = "Akulav/local-ai-ui";
/// Background check interval (the first check runs shortly after start-up).
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Debug, Default, Serialize)]
pub struct AppRelease {
    pub version: String,
    /// release page on GitHub
    pub url: String,
    pub zip: String,
    pub setup: String,
    pub sums: String,
}

/// What the UI and the tray show. `app` / `runtime` are set only when something newer exists.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub app: Option<AppRelease>,
    pub runtime_current: Option<String>,
    pub runtime: Option<String>,
    pub checked_at: i64,
    pub checking: bool,
    pub installing: bool,
    pub error: String,
}

/// Emits `update-status` and rebuilds the tray menu so both show the same state.
pub fn publish(app: &AppHandle) {
    let info = app.state::<Arc<AppState>>().updates.read().clone();
    crate::refresh_tray(app, &info);
    let _ = app.emit("update-status", info);
}

/// Runs the first check after start-up, then every few hours (when enabled in Settings).
pub fn spawn_background(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let state = app.state::<Arc<AppState>>().inner().clone();
            if state.quitting.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if state.settings.read().check_updates {
                check(&app).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Checks both sources and publishes the result. Never fails: problems end up in `error`.
pub async fn check(app: &AppHandle) -> UpdateInfo {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if state.updates.read().checking {
        let busy = state.updates.read().clone();
        return busy;
    }
    state.updates.write().checking = true;
    publish(app);
    let current = app.package_info().version.clone();
    let backend = state.settings.read().backend.clone();
    let (app_rel, rt) = tokio::join!(latest_app(&state, &current), crate::runtime::latest_release(&state, &backend));
    let runtime_current = crate::runtime::installed_version(&state, &backend);
    {
        let mut u = state.updates.write();
        u.current = current.to_string();
        u.checked_at = now_ms();
        u.checking = false;
        u.error.clear();
        match app_rel {
            Ok(r) => u.app = r,
            Err(e) => u.error = format!("Lumen: {e}"),
        }
        match rt {
            // only offered once a runtime is installed (the first one is fetched when a model loads)
            Ok((tag, _, _)) => u.runtime = runtime_current.as_deref().filter(|cur| newer_runtime(cur, &tag)).map(|_| tag),
            Err(e) => {
                if !u.error.is_empty() {
                    u.error.push_str(" · ");
                }
                u.error.push_str(&format!("llama.cpp: {e}"));
            }
        }
        u.runtime_current = runtime_current;
    }
    let info = state.updates.read().clone();
    state.log(format!("[update] checked: Lumen {:?}, llama.cpp {:?} {}", info.app.as_ref().map(|a| &a.version), info.runtime, info.error));
    publish(app);
    info
}

/// Called after the llama.cpp runtime was replaced, so the offer disappears.
pub fn runtime_updated(app: &AppHandle, version: Option<String>) {
    {
        let state = app.state::<Arc<AppState>>();
        let mut u = state.updates.write();
        u.runtime = None;
        u.runtime_current = version;
    }
    publish(app);
}

/// llama.cpp tags are `b<build number>`; compare numerically, otherwise any different tag is newer.
fn newer_runtime(current: &str, latest: &str) -> bool {
    let n = |s: &str| s.trim().trim_start_matches('b').parse::<u64>().ok();
    match (n(current), n(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => current.trim() != latest.trim(),
    }
}

async fn latest_app(state: &AppState, current: &semver::Version) -> Result<Option<AppRelease>> {
    let resp = state
        .http
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", LUMEN_UA)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if resp.status().as_u16() == 404 {
        return Ok(None); // no published release yet
    }
    let v: Value = resp.error_for_status()?.json().await?;
    let tag = v["tag_name"].as_str().unwrap_or_default();
    let version = semver::Version::parse(tag.trim_start_matches('v')).map_err(|e| anyhow!("bad release tag {tag}: {e}"))?;
    if version <= *current {
        return Ok(None);
    }
    let asset = |pred: &dyn Fn(&str) -> bool| -> String {
        v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"].as_str().is_some_and(pred))
            .and_then(|a| a["browser_download_url"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(Some(AppRelease {
        version: version.to_string(),
        url: v["html_url"].as_str().unwrap_or_default().to_string(),
        zip: asset(&|n| n.ends_with("-portable.zip")),
        setup: asset(&|n| n.ends_with("-setup.exe")),
        sums: asset(&|n| n == "SHA256SUMS.txt"),
    }))
}

/// Downloads, verifies and installs the offered Lumen release, then restarts into it.
pub async fn install_app(app: &AppHandle) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let rel = {
        let mut u = state.updates.write();
        if u.installing {
            return Err(anyhow!("an update is already being installed"));
        }
        let rel = u.app.clone().ok_or_else(|| anyhow!("no update available - check again"))?;
        u.installing = true;
        rel
    };
    publish(app);
    match install_inner(app, &state, &rel).await {
        Ok(()) => {
            state.log(format!("[update] installing Lumen {}, restarting", rel.version));
            crate::shutdown(app).await;
            app.exit(0);
            Ok(())
        }
        Err(e) => {
            state.log(format!("[update] failed: {e}"));
            state.updates.write().installing = false;
            publish(app);
            Err(e)
        }
    }
}

async fn install_inner(app: &AppHandle, state: &AppState, rel: &AppRelease) -> Result<()> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| anyhow!("cannot locate Lumen.exe"))?.to_path_buf();
    // the NSIS installer leaves uninstall.exe next to the exe; portable copies have none
    let installed = dir.join("uninstall.exe").exists();
    let url = if installed { &rel.setup } else { &rel.zip };
    if url.is_empty() || rel.sums.is_empty() {
        return Err(anyhow!("the release is missing its {} or SHA256SUMS.txt", if installed { "installer" } else { "portable zip" }));
    }
    let name = url.rsplit('/').next().unwrap_or("update.bin").to_string();
    let work = state.paths.root.join("updates");
    std::fs::create_dir_all(&work)?;
    let file = work.join(&name);
    let cancel = Arc::new(AtomicBool::new(false));
    crate::download::download(app, &state.http, "app-update", &format!("Lumen {}", rel.version), url, &file, cancel).await?;

    // integrity: the checksum list published with the release must match the download
    let sums = state.http.get(&rel.sums).header("User-Agent", LUMEN_UA).send().await?.error_for_status()?.text().await?;
    let expected = expected_hash(&sums, &name).ok_or_else(|| anyhow!("{name} is not listed in SHA256SUMS.txt"))?;
    let f2 = file.clone();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&f2)).await??;
    if actual != expected {
        let _ = std::fs::remove_file(&file);
        return Err(anyhow!("checksum mismatch for {name} - the download was discarded"));
    }

    if installed {
        // passive install (progress only), then start the updated app
        relaunch_after(&format!("\"{}\" /P", file.display()), &exe)
    } else {
        let f2 = file.clone();
        let new_exe = work.join("Lumen.exe.new");
        let n2 = new_exe.clone();
        tokio::task::spawn_blocking(move || extract_exe(&f2, &n2)).await??;
        swap_exe(&exe, &new_exe)?;
        let _ = std::fs::remove_file(&file);
        relaunch_after("", &exe)
    }
}

/// Hash for `name` in a `sha256sum`-style list (`<hex>  <file>`, optional `*` binary marker).
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (hash, n) = (it.next()?, it.next()?.trim_start_matches('*'));
        (n == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Pulls `Lumen/Lumen.exe` out of the portable zip.
fn extract_exe(zip_path: &Path, out: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let idx = (0..z.len())
        .find(|&i| z.by_index(i).map(|e| e.name().to_ascii_lowercase().ends_with("lumen.exe")).unwrap_or(false))
        .ok_or_else(|| anyhow!("Lumen.exe not found in the update zip"))?;
    let mut entry = z.by_index(idx)?;
    let mut o = std::fs::File::create(out)?;
    std::io::copy(&mut entry, &mut o)?;
    Ok(())
}

/// Windows lets a running exe be renamed but not overwritten: move it aside, put the new one in place.
fn swap_exe(exe: &Path, new_exe: &Path) -> Result<()> {
    let old = old_exe_path(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new_exe, exe).or_else(|_| std::fs::copy(new_exe, exe).map(|_| ())) {
        let _ = std::fs::rename(&old, exe); // put the running version back
        return Err(anyhow!("could not replace Lumen.exe: {e}"));
    }
    Ok(())
}

fn old_exe_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Starts `cmd` that waits for this process to exit, runs `before` (e.g. the installer) and launches
/// the exe again. Needed because the single-instance plugin would hand a too-early start back to us.
fn relaunch_after(before: &str, exe: &Path) -> Result<()> {
    let step = if before.is_empty() { String::new() } else { format!("{before} & ") };
    let line = format!("/d /s /c \"ping -n 3 127.0.0.1 >nul & {step}start \"\" \"{}\"\"", exe.display());
    const NO_WINDOW: u32 = 0x08000000;
    const BREAKAWAY_FROM_JOB: u32 = 0x01000000;
    let spawn = |flags: u32| std::process::Command::new("cmd").raw_arg(&line).creation_flags(flags).spawn();
    // leave any job Lumen itself runs in, so the relauncher survives our exit; not every job allows it
    spawn(NO_WINDOW | BREAKAWAY_FROM_JOB).or_else(|_| spawn(NO_WINDOW))?;
    Ok(())
}

/// Removes leftovers of a previous update (the replaced exe and downloaded files).
pub fn cleanup(state: &AppState) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe_path(&exe));
    }
    let _ = std::fs::remove_dir_all(state.paths.root.join("updates"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_tags_compare_by_build_number() {
        assert!(newer_runtime("b12061", "b12100"));
        assert!(!newer_runtime("b12061", "b12061"));
        assert!(!newer_runtime("b12100", "b9999"));
        assert!(newer_runtime("unknown", "b12100"));
    }

    #[test]
    fn finds_hash_in_release_sums() {
        // format of the v0.3.0 SHA256SUMS.txt (CRLF, two spaces)
        let sums = "1fa1c6a24789a0a55ab9265706 4fe6fbd0b312c185f2c4b006df6f58bd7517f827  Lumen_0.3.0_x64-setup.exe\r\n\
                    b774e08051dda5228007f80eb2dbb3ca22408f4b2f83450c468603b32f73819e  Lumen-v0.3.0-windows-x64-portable.zip\r\n";
        assert_eq!(
            expected_hash(sums, "Lumen-v0.3.0-windows-x64-portable.zip").as_deref(),
            Some("b774e08051dda5228007f80eb2dbb3ca22408f4b2f83450c468603b32f73819e")
        );
        assert_eq!(expected_hash(sums, "Lumen_0.3.0_x64-setup.exe"), None, "malformed hash is rejected");
        assert_eq!(expected_hash(sums, "other.zip"), None);
    }

    /// Exercises the real pipeline against the latest GitHub release: download, checksum, extract,
    /// exe swap, and the relauncher's quoting. Network: run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn install_pipeline_against_latest_release() {
        let tmp = std::env::temp_dir().join(format!("lumen update test {}", crate::state::new_id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let http = reqwest::Client::new();
        let (zip_url, sums_url) = rt.block_on(async {
            let v: Value = http
                .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
                .header("User-Agent", LUMEN_UA)
                .send().await.unwrap().json().await.unwrap();
            let url = |suffix: &str| -> String {
                v["assets"].as_array().unwrap().iter()
                    .find(|a| a["name"].as_str().unwrap().ends_with(suffix))
                    .unwrap()["browser_download_url"].as_str().unwrap().to_string()
            };
            (url("-portable.zip"), url("SHA256SUMS.txt"))
        });
        let name = zip_url.rsplit('/').next().unwrap().to_string();
        let zip_path = tmp.join(&name);
        let sums = rt.block_on(async {
            let bytes = http.get(&zip_url).header("User-Agent", LUMEN_UA).send().await.unwrap().bytes().await.unwrap();
            std::fs::write(&zip_path, &bytes).unwrap();
            http.get(&sums_url).header("User-Agent", LUMEN_UA).send().await.unwrap().text().await.unwrap()
        });
        assert_eq!(sha256_file(&zip_path).unwrap(), expected_hash(&sums, &name).expect("zip listed in sums"));

        let new_exe = tmp.join("Lumen.exe.new");
        extract_exe(&zip_path, &new_exe).unwrap();
        assert_eq!(&std::fs::read(&new_exe).unwrap()[..2], b"MZ", "extracted file is a Windows executable");

        // swap: the "running" exe is moved aside and the new one takes its place
        let exe = tmp.join("Lumen.exe");
        std::fs::write(&exe, b"old build").unwrap();
        swap_exe(&exe, &new_exe).unwrap();
        assert_eq!(&std::fs::read(&exe).unwrap()[..2], b"MZ");
        assert_eq!(std::fs::read(old_exe_path(&exe)).unwrap(), b"old build");

        // relauncher: paths with spaces survive cmd quoting; `before` runs, then the target starts
        let marker = tmp.join("relaunched.txt");
        let step_marker = tmp.join("step.txt");
        let target = tmp.join("fake app.bat");
        std::fs::write(&target, format!("@echo ok> \"{}\"\r\n", marker.display())).unwrap();
        relaunch_after(&format!("echo step> \"{}\"", step_marker.display()), &target).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(step_marker.exists(), "the step before the relaunch ran");
        assert!(marker.exists(), "the target was started after the delay");
        std::thread::sleep(Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn sha256_matches_known_value() {
        let p = std::env::temp_dir().join(format!("lumen-sha-{}", crate::state::new_id()));
        std::fs::write(&p, b"abc").unwrap();
        let h = sha256_file(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
