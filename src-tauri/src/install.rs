//! Installing on Linux and macOS (Windows: the Inno Setup wizard, src-tauri/installer/lumen.iss).
//! One release file per system, and the same file installs and updates:
//!
//! - Linux: the release file `lumen-linux-x64` is the whole program. Started from anywhere (Downloads…)
//!   it copies itself to ~/.local/share/lumen/lumen, adds an app-menu entry (and a desktop shortcut when
//!   there is a desktop folder) and starts that copy; its data then lives in ~/.local/share/lumen/LumenData.
//! - macOS: the release file `lumen-macos-arm64` is the whole program too. Started outside an app bundle
//!   (Downloads, Terminal) it builds ~/Applications/Lumen.app around a copy of itself (Info.plist from
//!   src-tauri/installer, the icon, an ad-hoc signature) and opens it; an existing /Applications/Lumen.app
//!   (older disk-image installs) is updated instead. Its data lives in ~/Library/Application Support/Lumen.
//!
//! `--here` runs where it is instead, `--uninstall` removes the program and its entries (the data only with
//! `--delete-data`). A program built on this computer (cargo / tauri: …/target/…) and one an update just
//! swapped in place (`--after-update`) always run where they are.

use std::path::{Path, PathBuf};

/// Returns true when this process should go on and start Lumen, false when it has done its job.
pub fn first_run() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--uninstall") {
        uninstall(has("--delete-data"));
        return false;
    }
    if has("--here") || has("--after-update") {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return true };
    if crate::updater::self_built(&exe) {
        return true;
    }
    place(&exe, &args)
}

/// Linux: install into ~/.local/share/lumen and start that copy (false), or run here (true).
#[cfg(not(target_os = "macos"))]
fn place(exe: &Path, args: &[String]) -> bool {
    let Some(dir) = crate::state::user_data_dir() else { return true };
    let target = dir.join("lumen");
    if same_file(exe, &target) {
        // the installed copy: keep its menu entry in place (it may have been deleted by hand)
        let _ = menu_entries(&target);
        return true;
    }
    match install(exe, &target) {
        Ok(()) => match std::process::Command::new(&target).args(args).spawn() {
            Ok(_) => {
                println!("Lumen is installed in {} and has an app menu entry.", dir.display());
                false
            }
            Err(e) => {
                eprintln!("Lumen: installed, but could not start {}: {e}. Running from here.", target.display());
                true
            }
        },
        Err(e) => {
            eprintln!("Lumen: could not install into {} ({e}). Running from here.", dir.display());
            true
        }
    }
}

/// macOS: a program inside an app bundle runs (true); a bare one installs Lumen.app and opens it (false).
#[cfg(target_os = "macos")]
fn place(exe: &Path, _args: &[String]) -> bool {
    if crate::updater::app_bundle(exe).is_some() {
        return true;
    }
    let Some(app) = mac::app_dir() else { return true };
    if let Err(e) = mac::install(exe, &app) {
        eprintln!("Lumen: could not install {} ({e}). Running from here.", app.display());
        return true;
    }
    match std::process::Command::new("/usr/bin/open").arg(&app).status() {
        Ok(s) if s.success() => {
            println!("Lumen is installed as {} (Launchpad, Spotlight).", app.display());
            false
        }
        _ => {
            eprintln!("Lumen: installed as {}, but could not open it. Running from here.", app.display());
            true
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The version an installed program reports (`lumen --version` → "Lumen 1.2.3 (made by Akulav)").
fn installed_version(target: &Path) -> Option<String> {
    if !target.is_file() {
        return None;
    }
    let out = std::process::Command::new(target).arg("--version").output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1).map(String::from)
}

/// True when the program at `target` is missing or older than this one (it gets replaced then).
fn needs_copy(target: &Path) -> bool {
    !installed_version(target).is_some_and(|v| !crate::updater::newer(env!("CARGO_PKG_VERSION"), &v))
}

/// Copies this program to `target` (executable). Written beside it and renamed: replacing a running
/// program in place fails ("text file busy").
fn copy_program(exe: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = target.with_extension("new");
    std::fs::copy(exe, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, target)
}

/// Copies this program into place (unless a newer one is there already) and adds the menu entries.
#[cfg(not(target_os = "macos"))]
fn install(exe: &Path, target: &Path) -> std::io::Result<()> {
    if needs_copy(target) {
        copy_program(exe, target)?;
    }
    menu_entries(target)
}

/// 256 px app icon for the menu entry.
#[cfg(not(target_os = "macos"))]
const ICON_PNG: &[u8] = include_bytes!("../icons/128x128@2x.png");

#[cfg(not(target_os = "macos"))]
fn desktop_file(target: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Lumen\nGenericName=Local AI chat\n\
         Comment=Chat with local AI models on this computer - made by Akulav\n\
         Exec=\"{}\"\nIcon={}\nTerminal=false\nCategories=Utility;Development;\nStartupWMClass=lumen\n",
        target.display(),
        icon.display()
    )
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// ~/.local/share/applications/lumen.desktop, plus ~/Desktop/lumen.desktop when a desktop folder exists.
#[cfg(not(target_os = "macos"))]
fn entry_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(data) = crate::state::user_data_dir().and_then(|d| d.parent().map(Path::to_path_buf)) {
        v.push(data.join("applications").join("lumen.desktop"));
    }
    if let Some(desk) = home().map(|h| h.join("Desktop")).filter(|d| d.is_dir()) {
        v.push(desk.join("lumen.desktop"));
    }
    v
}

#[cfg(not(target_os = "macos"))]
fn menu_entries(target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    let icon = dir.join("lumen.png");
    if !icon.exists() {
        std::fs::write(&icon, ICON_PNG)?;
    }
    let text = desktop_file(target, &icon);
    for p in entry_paths() {
        if std::fs::read_to_string(&p).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, &text)?;
        // a desktop launcher must be executable to be trusted
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Lumen.app, built by the program itself (no disk image).
#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// src-tauri/installer/Info.plist; @VERSION@ becomes this program's version.
    const PLIST: &str = include_str!("../installer/Info.plist");
    const ICNS: &[u8] = include_bytes!("../icons/icon.icns");

    /// An existing /Applications/Lumen.app (installed from the old disk image), else ~/Applications/Lumen.app
    /// (needs no administrator rights; Launchpad and Spotlight find it there too).
    pub fn app_dir() -> Option<PathBuf> {
        let system = PathBuf::from("/Applications/Lumen.app");
        if system.join("Contents").is_dir() {
            return Some(system);
        }
        Some(super::home()?.join("Applications").join("Lumen.app"))
    }

    pub fn info_plist() -> String {
        PLIST.replace("@VERSION@", env!("CARGO_PKG_VERSION"))
    }

    /// Puts this program in `app` (unless a newer one is there already), with Info.plist and icon, then
    /// signs the bundle ad hoc (its seal covers the program; macOS refuses to open a broken seal).
    pub fn install(exe: &Path, app: &Path) -> std::io::Result<()> {
        let contents = app.join("Contents");
        let res = contents.join("Resources");
        std::fs::create_dir_all(&res)?;
        let mut changed = false;
        let bin = contents.join("MacOS").join("lumen");
        if super::needs_copy(&bin) {
            super::copy_program(exe, &bin)?;
            std::fs::write(contents.join("Info.plist"), info_plist())?;
            changed = true;
        }
        let icon = res.join("icon.icns");
        if !icon.exists() {
            std::fs::write(&icon, ICNS)?;
            changed = true;
        }
        if changed {
            // the user chose to run this download: its copy must not be stopped by Gatekeeper's
            // quarantine check (the app is signed ad hoc, not with a paid Apple certificate)
            let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(app).output();
            let out = Command::new("/usr/bin/codesign").args(["--force", "--deep", "--sign", "-"]).arg(app).output()?;
            if !out.status.success() {
                return Err(std::io::Error::other(format!("codesign: {}", String::from_utf8_lossy(&out.stderr).trim())));
            }
        }
        Ok(())
    }

    /// Every Lumen.app this program may have installed or updated.
    pub fn installed_apps() -> Vec<PathBuf> {
        let mut v = vec![PathBuf::from("/Applications/Lumen.app")];
        if let Some(h) = super::home() {
            v.push(h.join("Applications").join("Lumen.app"));
        }
        v.retain(|a| a.join("Contents").join("MacOS").join("lumen").is_file());
        v
    }
}

fn uninstall(delete_data: bool) {
    #[cfg(not(target_os = "macos"))]
    {
        let Some(dir) = crate::state::user_data_dir() else { return };
        for p in entry_paths() {
            let _ = std::fs::remove_file(p);
        }
        for f in ["lumen", "lumen.png", "lumen.old", "lumen.new"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        if delete_data {
            let _ = std::fs::remove_dir_all(&dir);
            println!("Lumen and its data (models, chats, settings) were removed from {}.", dir.display());
        } else {
            let _ = std::fs::remove_dir(&dir); // only when empty
            println!(
                "Lumen was removed. Its data stays in {} (run with --uninstall --delete-data to remove it too).",
                dir.join("LumenData").display()
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        for app in mac::installed_apps() {
            match std::fs::remove_dir_all(&app) {
                Ok(()) => println!("Removed {}.", app.display()),
                Err(e) => eprintln!("Lumen: could not remove {} ({e}): drag it to the Bin.", app.display()),
            }
        }
        let Some(data) = crate::state::user_data_dir() else { return };
        if delete_data {
            let _ = std::fs::remove_dir_all(&data);
            println!("Lumen's data (models, chats, settings) was removed from {}.", data.display());
        } else {
            println!("Lumen's data stays in {} (run with --uninstall --delete-data to remove it too).", data.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn menu_entry_points_at_the_installed_program() {
        let t = desktop_file(Path::new("/home/u/.local/share/lumen/lumen"), Path::new("/home/u/.local/share/lumen/lumen.png"));
        assert!(t.contains("Exec=\"/home/u/.local/share/lumen/lumen\""));
        assert!(t.contains("Icon=/home/u/.local/share/lumen/lumen.png"));
        assert!(t.starts_with("[Desktop Entry]"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundle_carries_this_version() {
        let p = mac::info_plist();
        assert!(!p.contains("@VERSION@"));
        assert!(p.contains(&format!("<string>{}</string>", env!("CARGO_PKG_VERSION"))));
        assert!(p.contains("<key>CFBundleExecutable</key><string>lumen</string>"));
    }

    #[test]
    fn copy_program_makes_an_executable_copy() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lumen-install-{}", crate::state::new_id()));
        let src = dir.join("download");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&src, b"prog").unwrap();
        let target = dir.join("a").join("b").join("lumen");
        copy_program(&src, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"prog");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!target.with_extension("new").exists());
        // nothing installed yet: it gets copied
        assert!(needs_copy(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
