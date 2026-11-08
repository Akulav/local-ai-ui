//! Updates from the GitHub releases (Akulav/local-ai-ui): checked 20 s after start and every 6 hours
//! (Settings → About can turn that off), or when asked (the Check for updates button / tray entry).
//! Also looks for a newer llama.cpp build (runtime.rs installs it).
//!
//! A release has ONE file per system, and the same file installs and updates
//! (.github/workflows/release.yml), always checked against the release's SHA256SUMS.txt first -
//! nothing changes when the download or its checksum fails:
//!
//! - Windows: `Lumen-setup.exe` (Inno Setup, src-tauri/installer/lumen.iss). It is started on this
//!   install's folder (/SILENT: a progress window, no questions) and Lumen ends; the setup uninstalls the
//!   old version (LumenData - models, chats, settings - stays), installs the new one and starts it.
//! - Linux: `lumen-linux-x64`, macOS: `lumen-macos-arm64` - the program itself. It is downloaded next to
//!   this one, swapped in (this one becomes `.old`, removed at the next start) and started again; on
//!   macOS inside Lumen.app, which is re-signed (ad hoc) and reopened.
//!
//! A copy built on this computer (…/target/… or the repository's dist folder) is never replaced.

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
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

/// The release file for this computer (Windows: the setup, which installs and updates alike).
pub fn asset_name() -> &'static str {
    if cfg!(windows) {
        "Lumen-setup.exe"
    } else if cfg!(target_os = "macos") {
        "lumen-macos-arm64"
    } else {
        "lumen-linux-x64"
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AppRelease {
    pub version: String,
    /// release page on GitHub ("What's new")
    pub url: String,
    /// this computer's file (`asset_name`) and the checksum list
    pub file: String,
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
    /// this copy can't update itself (built here): the UI says so instead of offering the button
    pub self_built: bool,
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
    let current = app.package_info().version.to_string();
    let backend = crate::runtime::effective_backend(&state.settings.read().backend);
    let (app_rel, rt) = tokio::join!(latest_app(&state, &current), crate::runtime::latest_release(&state, &backend));
    let runtime_current = crate::runtime::installed_version(&state, &backend);
    {
        let mut u = state.updates.write();
        u.current = current;
        u.checked_at = now_ms();
        u.checking = false;
        u.error.clear();
        u.self_built = std::env::current_exe().map(|e| self_built(&e)).unwrap_or(false);
        match app_rel {
            Ok(r) => u.app = r,
            Err(e) => u.error = format!("Lumen: {e}"),
        }
        match rt {
            // only offered once a runtime is installed (the first one is fetched when a model loads)
            Ok(r) => u.runtime = runtime_current.as_deref().filter(|cur| newer_runtime(cur, &r.tag)).map(|_| r.tag),
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

/// "v2.10.1" -> [2, 10, 1] (a pre-release suffix is ignored for the numbers).
fn numbers(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("").split('.').map(|x| x.parse().unwrap_or(0)).collect()
}

/// Is `latest` newer than `current`?
pub fn newer(latest: &str, current: &str) -> bool {
    let (a, b) = (numbers(latest), numbers(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The release as GitHub's API describes it ("latest release" JSON), with this computer's file.
fn parse_release(v: &Value) -> Result<AppRelease> {
    let tag = v["tag_name"].as_str().ok_or_else(|| anyhow!("GitHub's answer has no version"))?;
    let asset = |name: &str| -> String {
        v["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["browser_download_url"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(AppRelease {
        version: tag.trim_start_matches('v').to_string(),
        url: v["html_url"].as_str().unwrap_or_default().to_string(),
        file: asset(asset_name()),
        sums: asset("SHA256SUMS.txt"),
    })
}

async fn latest_app(state: &AppState, current: &str) -> Result<Option<AppRelease>> {
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
    let rel = parse_release(&resp.error_for_status()?.json().await?)?;
    // (a release without a file for this computer is not offered: nothing could be installed)
    Ok((newer(&rel.version, current) && !rel.file.is_empty()).then_some(rel))
}

/// A program built on this computer (cargo: …/target/…, run.bat: <repo>/dist/Lumen) is never replaced
/// by a download.
pub fn self_built(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
        || exe.ancestors().nth(3).is_some_and(|repo| repo.join("src-tauri").join("Cargo.toml").is_file())
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
    if self_built(&exe) {
        return Err(anyhow!("this copy of Lumen was built on this computer: update it from the source (git pull, run.bat) instead"));
    }
    if rel.file.is_empty() || rel.sums.is_empty() {
        return Err(anyhow!("the release {} has no {} with a checksum", rel.version, asset_name()));
    }
    let work = state.paths.root.join("updates");
    std::fs::create_dir_all(&work)?;
    let file = work.join(format!("{}-{}", rel.version, asset_name()));
    let cancel = Arc::new(AtomicBool::new(false));
    crate::download::download(app, &state.http, "app-update", &format!("Lumen {}", rel.version), &rel.file, &file, cancel).await?;

    // integrity: the checksum list published with the release must match the download
    let sums = state.http.get(&rel.sums).header("User-Agent", LUMEN_UA).send().await?.error_for_status()?.text().await?;
    let expected = expected_hash(&sums, asset_name()).ok_or_else(|| anyhow!("{} is not listed in SHA256SUMS.txt", asset_name()))?;
    let f2 = file.clone();
    let actual = tokio::task::spawn_blocking(move || sha256_file(&f2)).await??;
    if actual != expected {
        let _ = std::fs::remove_file(&file);
        return Err(anyhow!("the download doesn't match the release's checksum - nothing was installed"));
    }
    let app_dir = exe.parent().ok_or_else(|| anyhow!("cannot locate Lumen's folder"))?;

    #[cfg(windows)]
    {
        run_setup(&file, app_dir, &state.paths.root.join("logs").join("setup.log"))
    }
    #[cfg(not(windows))]
    {
        let _ = app_dir;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))?;
        swap(&exe, &file)?;
        relaunch(&exe)
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

/// The setup's options for an update (lumen.iss): on this folder, a progress window but no questions,
/// close a running Lumen, start Lumen when done (LAUNCH=1), and its log in LumenData/logs/setup.log.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn setup_args(app_dir: &Path, log: &Path) -> Vec<String> {
    vec![
        "/SILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/CLOSEAPPLICATIONS".into(),
        "/LAUNCH=1".into(),
        format!("/DIR={}", app_dir.display()),
        format!("/LOG={}", log.display()),
    ]
}

/// Starts the setup on its own (it outlives Lumen, which ends so its files can be replaced).
#[cfg(windows)]
fn run_setup(setup: &Path, app_dir: &Path, log: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    if let Some(d) = log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let spawn = |flags: u32| std::process::Command::new(setup).args(setup_args(app_dir, log)).creation_flags(flags).spawn();
    // leave any job Lumen itself runs in, so the setup survives Lumen's exit; not every job allows it
    spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP))
        .map_err(|e| anyhow!("couldn't start the setup: {e}"))?;
    Ok(())
}

/// lumen -> lumen.old / lumen.exe -> lumen.exe.old
pub fn sibling(exe: &Path, ext: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(format!(".{ext}"));
    PathBuf::from(s)
}

/// Puts the new program in this one's place (this one is kept as .old until the next start removes it).
/// (Linux, macOS: on Windows the setup replaces the program.)
#[cfg_attr(windows, allow(dead_code))]
pub fn swap(exe: &Path, new: &Path) -> Result<()> {
    let old = sibling(exe, "old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| anyhow!("couldn't move this program aside: {e}"))?;
    if let Err(e) = std::fs::rename(new, exe).or_else(|_| std::fs::copy(new, exe).map(|_| ())) {
        let _ = std::fs::rename(&old, exe); // put it back: nothing changed
        return Err(anyhow!("couldn't put the new program in place: {e}"));
    }
    Ok(())
}

/// The Lumen.app a macOS program runs from (…/Lumen.app/Contents/MacOS/lumen), if any.
#[cfg_attr(windows, allow(dead_code))]
pub fn app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Starts the (new) program once this one has ended (the single-instance guard would otherwise hand
/// the start back to us). macOS: re-sign the bundle (its seal covers the program) and reopen it.
#[cfg(not(windows))]
fn relaunch(exe: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    match app_bundle(exe).filter(|_| cfg!(target_os = "macos")) {
        Some(bundle) => {
            let _ = std::process::Command::new("codesign").args(["--force", "--deep", "--sign", "-"]).arg(&bundle).status();
            cmd.arg("-c").arg("sleep 2; open -n \"$0\"").arg(&bundle);
        }
        None => {
            cmd.arg("-c").arg("sleep 2; exec \"$0\" --after-update").arg(exe);
        }
    }
    // its own process group: it must outlive Lumen
    cmd.process_group(0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map_err(|e| anyhow!("couldn't start the new version: {e} - start Lumen again to run it"))?;
    Ok(())
}

/// (At start.) Removes what the last update left: the previous program (.old), an unfinished
/// download (.new) and the downloaded setup / program.
pub fn cleanup(state: &AppState) {
    if let Ok(exe) = std::env::current_exe() {
        for p in [sibling(&exe, "old"), sibling(&exe, "new")] {
            let _ = std::fs::remove_file(p);
        }
        // (pre-0.9 Windows updates left lumen.exe.old)
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
    let _ = std::fs::remove_dir_all(state.paths.root.join("updates"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("v0.9.1", "0.9.0"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.9", "0.9.0"));
        assert!(newer("1", "0.99.99"));
        assert!(!newer("0.9.0-beta", "0.9.0"));
    }

    #[test]
    fn runtime_tags_compare_by_build_number() {
        assert!(newer_runtime("b12061", "b12100"));
        assert!(!newer_runtime("b12061", "b12061"));
        assert!(!newer_runtime("b12100", "b9999"));
        assert!(newer_runtime("unknown", "b12100"));
    }

    #[test]
    fn release_json_gives_this_computers_file() {
        let v = serde_json::json!({"tag_name": "v0.9.0", "html_url": "https://github.com/x/y/releases/tag/v0.9.0", "assets": [
            {"name": "SHA256SUMS.txt", "browser_download_url": "https://dl/sums"},
            {"name": asset_name(), "browser_download_url": "https://dl/prog"},
            {"name": "other", "browser_download_url": "https://dl/other"}]});
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, "0.9.0");
        assert_eq!(r.file, "https://dl/prog");
        assert_eq!(r.sums, "https://dl/sums");
        assert!(parse_release(&serde_json::json!({})).is_err());
        // an older release layout (no file for this computer) has nothing to install
        let old = parse_release(&serde_json::json!({"tag_name": "v0.8.0", "assets": [{"name": "Lumen_0.8.0_x64-setup.exe", "browser_download_url": "x"}]})).unwrap();
        assert!(old.file.is_empty());
    }

    #[test]
    fn finds_hash_in_release_sums() {
        // format of a real SHA256SUMS.txt (CRLF, two spaces)
        let sums = "1fa1c6a24789a0a55ab9265706 4fe6fbd0b312c185f2c4b006df6f58bd7517f827  Lumen-setup.exe\r\n\
                    b774e08051dda5228007f80eb2dbb3ca22408f4b2f83450c468603b32f73819e  lumen-linux-x64\r\n";
        assert_eq!(expected_hash(sums, "lumen-linux-x64").as_deref(), Some("b774e08051dda5228007f80eb2dbb3ca22408f4b2f83450c468603b32f73819e"));
        assert_eq!(expected_hash(sums, "Lumen-setup.exe"), None, "malformed hash is rejected");
        assert_eq!(expected_hash(sums, "other.zip"), None);
    }

    #[test]
    fn windows_updates_with_the_setup_on_this_folder() {
        if cfg!(windows) {
            assert_eq!(asset_name(), "Lumen-setup.exe");
        }
        let args = setup_args(Path::new("C:/Users/u/AppData/Local/Lumen"), Path::new("C:/Users/u/AppData/Local/Lumen/LumenData/logs/setup.log"));
        assert!(args.contains(&"/SILENT".to_string()) && args.contains(&"/LAUNCH=1".to_string()) && args.contains(&"/CLOSEAPPLICATIONS".to_string()));
        assert!(args.iter().any(|a| a == "/DIR=C:/Users/u/AppData/Local/Lumen"), "{args:?}");
        assert!(args.iter().any(|a| a.starts_with("/LOG=") && a.ends_with("setup.log")));
    }

    #[test]
    fn swap_puts_the_new_program_in_place_and_keeps_the_old() {
        let dir = std::env::temp_dir().join(format!("lumen-swap-{}", crate::state::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("lumen");
        std::fs::write(&exe, b"old").unwrap();
        let new = dir.join("download");
        std::fs::write(&new, b"new").unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(sibling(&exe, "old")).unwrap(), b"old");
        // a missing download: nothing changes
        assert!(swap(&exe, &dir.join("missing")).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copies_built_here_are_never_replaced() {
        assert!(self_built(Path::new("D:/src/local-ai-ui/src-tauri/target/release/lumen.exe")));
        let repo = std::env::current_dir().unwrap(); // (tests run in src-tauri)
        assert!(self_built(&repo.parent().unwrap().join("dist").join("Lumen").join("Lumen.exe")));
        assert!(!self_built(Path::new("C:/Users/u/AppData/Local/Lumen/lumen.exe")));
        assert_eq!(app_bundle(Path::new("/Applications/Lumen.app/Contents/MacOS/lumen")), Some(PathBuf::from("/Applications/Lumen.app")));
        assert_eq!(app_bundle(Path::new("/home/u/.local/share/lumen/lumen")), None);
    }

    /// The real Windows update step: starts a setup exactly as the Update button does (detached, the
    /// same arguments) on a test install folder. Opt-in, with a test build of the setup:
    /// `set LUMEN_TEST_SETUP=…\Lumen-setup.exe`, `set LUMEN_TEST_DIR=<installed folder>`, then
    /// `cargo test live_windows_update -- --ignored`. It returns once the setup has started; the
    /// caller checks the result (scripts\test-installer.ps1 in the scratchpad did).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn live_windows_update() {
        let (Ok(setup), Ok(dir)) = (std::env::var("LUMEN_TEST_SETUP"), std::env::var("LUMEN_TEST_DIR")) else {
            eprintln!("LUMEN_TEST_SETUP / LUMEN_TEST_DIR not set - skipped");
            return;
        };
        let log = Path::new(&dir).join("LumenData").join("logs").join("setup.log");
        run_setup(Path::new(&setup), Path::new(&dir), &log).unwrap();
        // the setup runs on its own; wait for its log to say it finished
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if text.contains("Installation process succeeded") || text.contains("Log closed") {
                println!("{}", text.lines().filter(|l| l.contains("Uninstall") || l.contains("succeeded") || l.contains("Run") || l.contains("Dest filename")).collect::<Vec<_>>().join("\n"));
                assert!(text.contains("Installation process succeeded"), "setup failed:\n{text}");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the setup did not finish:\n{text}");
            std::thread::sleep(Duration::from_millis(500));
        }
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
