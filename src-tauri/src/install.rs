//! Installing on Linux and macOS (Windows: the Inno Setup wizard, src-tauri/installer/lumen.iss).
//! One release file per system, and the same file installs and updates.
//!
//! Started from anywhere (Downloads…) the program asks for an **install folder** with the system's folder
//! picker (zenity / kdialog on Linux, Finder on macOS; `--install-dir <folder>` skips the question),
//! copies itself there and starts that copy. Everything Lumen keeps (models, chats, settings, runtime)
//! then lives in `<folder>/LumenData`.
//!
//! - Linux: `<folder>/lumen` (default ~/.local/share/lumen) + an app-menu entry (and a desktop shortcut
//!   when there is a desktop folder).
//! - macOS: `<folder>/Lumen.app` (default ~/Applications/Lumen; Info.plist from src-tauri/installer, the
//!   icon, an ad-hoc signature). An older `~/Applications/Lumen.app` or `/Applications/Lumen.app` and its
//!   data in ~/Library/Application Support/Lumen move into the folder.
//!
//! The folder is remembered (`<config dir>/lumen/install-dir`), so a newer download updates that copy
//! without asking. `--here` runs where it is instead, `--uninstall` removes the program and its entries
//! (the data only with `--delete-data`). A program built on this computer (…/target/…) and one an update
//! just swapped in place (`--after-update`) always run where they are.

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
    // the installed copy: keep its folder remembered and (Linux) its menu entry in place
    if let Some(folder) = installed_folder_of(&exe) {
        remember(&folder);
        #[cfg(not(target_os = "macos"))]
        let _ = menu_entries(&program_in(&folder));
        return true;
    }
    #[cfg(target_os = "macos")]
    let legacy_app = mac::legacy_app_of(&exe);
    #[cfg(not(target_os = "macos"))]
    let legacy_app: Option<PathBuf> = None;
    let explicit = args.iter().position(|a| a == "--install-dir").and_then(|i| args.get(i + 1)).map(|d| expand_home(d));
    let folder = match explicit.or_else(remembered_folder) {
        Some(f) => f,
        // a pre-0.15 app updated itself in place: move it without asking
        None if legacy_app.is_some() => default_folder(),
        None => match ask_folder(&default_folder()) {
            Some(f) => f,
            None => return true, // cancelled: just run it
        },
    };
    match install(&exe, &folder) {
        Ok(()) => {
            #[cfg(target_os = "macos")]
            mac::retire_legacy(&folder);
            let skip = args.iter().position(|a| a == "--install-dir");
            let rest: Vec<&String> =
                args.iter().enumerate().filter(|(i, _)| skip.is_none_or(|s| *i != s && *i != s + 1)).map(|(_, a)| a).collect();
            match launch(&folder, &rest) {
                Ok(()) => {
                    println!("Lumen is installed in {}.", folder.display());
                    false
                }
                Err(e) => {
                    eprintln!("Lumen: installed, but could not start it ({e}). Running from here.");
                    true
                }
            }
        }
        Err(e) => {
            eprintln!("Lumen: could not install into {} ({e}). Running from here.", folder.display());
            true
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------------------------

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn expand_home(text: &str) -> PathBuf {
    match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => home().unwrap_or_default().join(rest.trim_start_matches('/')),
        _ => PathBuf::from(text),
    }
}

/// ~/.local/share/lumen (`$XDG_DATA_HOME/lumen`, where Lumen before 0.15 installed) or ~/Applications/Lumen.
fn default_folder() -> PathBuf {
    if cfg!(target_os = "macos") {
        return home().unwrap_or_default().join("Applications").join("Lumen");
    }
    crate::state::user_data_dir().unwrap_or_else(|| home().unwrap_or_default().join(".local/share/lumen"))
}

fn pointer_file() -> Option<PathBuf> {
    let config = if cfg!(target_os = "macos") {
        home()?.join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|h| h.join(".config")))?
    };
    Some(config.join("lumen").join("install-dir"))
}

fn remembered_folder() -> Option<PathBuf> {
    let t = std::fs::read_to_string(pointer_file()?).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| PathBuf::from(t))
}

fn remember(folder: &Path) {
    if remembered_folder().is_some_and(|r| same_file(&r, folder)) {
        return;
    }
    if let Some(f) = pointer_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, folder.to_string_lossy().as_bytes());
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The program inside an install folder.
fn program_in(folder: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        folder.join("Lumen.app").join("Contents").join("MacOS").join("lumen")
    } else {
        folder.join("lumen")
    }
}

/// The install folder `exe` runs from: the remembered one, or (Linux) ~/.local/share/lumen where Lumen
/// before 0.15 installed. A macOS `Lumen.app` straight in an Applications folder is the old layout: no.
fn installed_folder_of(exe: &Path) -> Option<PathBuf> {
    let candidates = [remembered_folder(), (!cfg!(target_os = "macos")).then(default_folder)];
    candidates.into_iter().flatten().find(|f| same_file(exe, &program_in(f)))
}

/// The system's folder picker; `None` when cancelled. Without one, the default folder is used.
fn ask_folder(default: &Path) -> Option<PathBuf> {
    use std::process::Command;
    let _ = std::fs::create_dir_all(default.parent().unwrap_or(default));
    let title = "Choose where to install Lumen (models and chats go there too)";
    let out = if cfg!(target_os = "macos") {
        let start = default.parent().unwrap_or(default).display().to_string().replace('"', "");
        Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(format!(
                "POSIX path of (choose folder with prompt \"{title}. A Lumen folder is created inside the one you pick.\" default location POSIX file \"{start}\")"
            ))
            .output()
            .ok()
    } else {
        Command::new("zenity")
            .args(["--file-selection", "--directory", "--title", title, "--filename"])
            .arg(format!("{}/", default.display()))
            .output()
            .ok()
            .or_else(|| Command::new("kdialog").arg("--getexistingdirectory").arg(default).args(["--title", title]).output().ok())
    };
    match out {
        None => Some(default.to_path_buf()),
        Some(o) if o.status.success() => {
            let picked = String::from_utf8_lossy(&o.stdout).trim().trim_end_matches('/').to_string();
            if picked.is_empty() {
                return None;
            }
            let p = PathBuf::from(picked);
            Some(if cfg!(target_os = "macos") && !p.ends_with("Lumen") { p.join("Lumen") } else { p })
        }
        Some(_) => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Installing
// ---------------------------------------------------------------------------------------------

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

/// Copies this program into `folder` (unless a newer one is there already), remembers the folder and adds
/// the menu entry (Linux) or builds the app around it (macOS).
fn install(exe: &Path, folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    #[cfg(target_os = "macos")]
    mac::install(exe, &folder.join("Lumen.app"))?;
    #[cfg(not(target_os = "macos"))]
    {
        let target = program_in(folder);
        if needs_copy(&target) && !same_file(exe, &target) {
            copy_program(exe, &target)?;
        }
        menu_entries(&target)?;
    }
    remember(folder);
    Ok(())
}

fn launch(folder: &Path, args: &[&String]) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        let ok = std::process::Command::new("/usr/bin/open").arg("-n").arg(folder.join("Lumen.app")).status()?.success();
        if !ok {
            return Err(std::io::Error::other("open failed"));
        }
    } else {
        std::process::Command::new(program_in(folder)).args(args).current_dir(folder).spawn()?;
    }
    Ok(())
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

    pub fn info_plist() -> String {
        PLIST.replace("@VERSION@", env!("CARGO_PKG_VERSION"))
    }

    /// Before 0.15: `~/Applications/Lumen.app` or `/Applications/Lumen.app`.
    fn legacy_apps() -> Vec<PathBuf> {
        let mut v = vec![PathBuf::from("/Applications/Lumen.app")];
        if let Some(h) = super::home() {
            v.push(h.join("Applications").join("Lumen.app"));
        }
        v.retain(|a| a.join("Contents").join("MacOS").join("lumen").is_file());
        v
    }

    /// The pre-0.15 app this program runs from, if any.
    pub fn legacy_app_of(exe: &Path) -> Option<PathBuf> {
        let app = crate::updater::app_bundle(exe)?;
        legacy_apps().into_iter().find(|a| super::same_file(a, &app))
    }

    /// After installing into `folder`: the old apps go, and their data
    /// (~/Library/Application Support/Lumen) moves into `<folder>/LumenData`.
    pub fn retire_legacy(folder: &Path) {
        let new_app = folder.join("Lumen.app");
        for app in legacy_apps() {
            if !super::same_file(&app, &new_app) {
                let _ = std::fs::remove_dir_all(&app);
            }
        }
        let data = folder.join("LumenData");
        if let Some(old) = crate::state::user_data_dir().filter(|d| d.join("data").is_dir()) {
            if !data.join("data").exists() {
                let _ = crate::state::move_dir_contents(&old, &data);
            }
        }
    }

    /// Puts this program in `app` (unless a newer one is there already), with Info.plist and icon, then
    /// signs the bundle ad hoc (its seal covers the program; macOS refuses to open a broken seal).
    pub fn install(exe: &Path, app: &Path) -> std::io::Result<()> {
        let contents = app.join("Contents");
        let res = contents.join("Resources");
        std::fs::create_dir_all(&res)?;
        let mut changed = false;
        let bin = contents.join("MacOS").join("lumen");
        if super::needs_copy(&bin) && !super::same_file(exe, &bin) {
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
}

fn uninstall(delete_data: bool) {
    let folder = remembered_folder().or_else(|| {
        let d = default_folder();
        program_in(&d).is_file().then_some(d)
    });
    let Some(dir) = folder else {
        println!("Lumen is not installed for this user.");
        return;
    };
    #[cfg(not(target_os = "macos"))]
    {
        for p in entry_paths() {
            let _ = std::fs::remove_file(p);
        }
        for f in ["lumen", "lumen.png", "lumen.old", "lumen.new"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
    #[cfg(target_os = "macos")]
    match std::fs::remove_dir_all(dir.join("Lumen.app")) {
        Ok(()) => println!("Removed {}.", dir.join("Lumen.app").display()),
        Err(e) => eprintln!("Lumen: could not remove the app ({e}): drag it to the Bin."),
    }
    if let Some(p) = pointer_file() {
        let _ = std::fs::remove_file(p);
    }
    if delete_data {
        let _ = std::fs::remove_dir_all(dir.join("LumenData"));
        println!("Lumen and its data (models, chats, settings) were removed from {}.", dir.display());
    } else {
        println!(
            "Lumen was removed. Its data stays in {} (run with --uninstall --delete-data to remove it too).",
            dir.join("LumenData").display()
        );
    }
    let _ = std::fs::remove_dir(&dir); // only when empty
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
        assert_eq!(program_in(Path::new("/srv/l")).file_name().unwrap(), "lumen");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
