#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Lumen entry point (made by Akulav): plugins, tray icon, close-to-tray, background start-up
//! (runtime download + last-model autoload) and guaranteed shutdown of the llama-server children.

mod audit;
mod calc;
mod chat;
mod codetools;
mod commands;
mod db;
mod download;
mod extract;
mod llm;
mod metrics;
mod models;
mod prompts;
mod rag;
mod research;
mod runtime;
mod server;
mod state;
mod tools;
mod updater;
mod web;
// system specifics behind one interface: win.rs on Windows, unix.rs on Linux and macOS
#[cfg_attr(windows, path = "win.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod platform;
#[cfg(not(windows))]
mod install;

use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent, Wry};

use state::AppState;
use updater::UpdateInfo;

const TRAY_ID: &str = "lumen-tray";

/// Tray labels follow the UI language (the tray cannot use ui/i18n.js).
fn tray_text(lang: &str, key: &str) -> &'static str {
    let ro = lang == "ro";
    match key {
        "show" => if ro { "Deschide Lumen" } else { "Open Lumen" },
        "check" => if ro { "Caută actualizări" } else { "Check for updates" },
        "unload" => if ro { "Descarcă modelul (eliberează VRAM)" } else { "Unload model (free VRAM)" },
        "quit" => if ro { "Închide Lumen" } else { "Quit Lumen" },
        "app_update" => if ro { "Actualizează Lumen la v" } else { "Update Lumen to v" },
        "rt_update" => if ro { "Actualizează llama.cpp la " } else { "Update llama.cpp to " },
        "reply_ready" => if ro { "Lumen – un răspuns este gata" } else { "Lumen – a reply is ready" },
        _ => "",
    }
}

/// Tray menu; update entries appear only when a newer Lumen / llama.cpp exists.
fn tray_menu(app: &AppHandle, u: &UpdateInfo) -> tauri::Result<Menu<Wry>> {
    let lang = app.state::<Arc<AppState>>().settings.read().ui_lang.clone();
    let item = |id: &str, text: String| MenuItem::with_id(app, id, text, true, None::<&str>);
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = vec![Box::new(item("show", tray_text(&lang, "show").into())?)];
    if let Some(a) = &u.app {
        items.push(Box::new(item("app_update", format!("⬆ {}{}", tray_text(&lang, "app_update"), a.version))?));
    }
    if let Some(r) = &u.runtime {
        items.push(Box::new(item("rt_update", format!("⬆ {}{r}", tray_text(&lang, "rt_update")))?));
    }
    items.push(Box::new(item("check", tray_text(&lang, "check").into())?));
    items.push(Box::new(item("unload", tray_text(&lang, "unload").into())?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(item("quit", tray_text(&lang, "quit").into())?));
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

/// Rebuilds the tray menu (called whenever the update state changes).
pub fn refresh_tray(app: &AppHandle, u: &UpdateInfo) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), tray_menu(app, u)) {
        let _ = tray.set_menu(Some(menu));
        let tip = match &u.app {
            Some(a) => format!("Lumen – update v{} available", a.version),
            None => "Lumen – local AI · made by Akulav".to_string(),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    // (clears a "reply is ready" tooltip)
    let updates = app.state::<Arc<AppState>>().updates.read().clone();
    refresh_tray(app, &updates);
}

/// A reply finished. If Lumen is in the background, make that noticeable: the taskbar button flashes
/// (Dock icon bounces on macOS, urgency hint on Linux) until the window is focused; when the window is
/// hidden in the tray, the tray tooltip says a reply is ready until it is opened again.
pub fn reply_finished(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    if !w.is_visible().unwrap_or(true) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let lang = app.state::<Arc<AppState>>().settings.read().ui_lang.clone();
            let _ = tray.set_tooltip(Some(tray_text(&lang, "reply_ready")));
        }
    } else if !w.is_focused().unwrap_or(true) {
        let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
}

/// Stops every child process. Safe to call more than once.
pub async fn shutdown(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    state.quitting.store(true, Ordering::Relaxed);
    chat::stop_all(&state);
    server::stop_all(app).await;
}

fn main() {
    if std::env::args().any(|a| a == "--version") {
        println!("Lumen {} (made by Akulav)", env!("CARGO_PKG_VERSION"));
        return;
    }
    // Linux / macOS: the downloaded program installs itself (app menu entry / Lumen.app) and runs from there;
    // also handles --uninstall. Returns false when this process should end (the installed copy took over).
    #[cfg(not(windows))]
    if !install::first_run() {
        return;
    }
    let state = Arc::new(AppState::new().expect("failed to initialise Lumen data folder"));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_settings,
            commands::list_models,
            commands::delete_model,
            commands::download_model,
            commands::cancel_download,
            commands::list_repo,
            commands::search_hf,
            commands::load_model,
            commands::unload_model,
            commands::update_runtime,
            commands::check_updates,
            commands::install_update,
            commands::get_logs,
            commands::list_chats,
            commands::create_chat,
            commands::search_chats,
            commands::export_chat,
            commands::get_chat,
            commands::rename_chat,
            commands::pin_chat,
            commands::set_chat_limit,
            commands::set_chat_project,
            commands::delete_chat,
            commands::delete_exchange,
            commands::send_message,
            commands::stop_generation,
            commands::regenerate,
            commands::edit_and_resend,
            commands::compress_chat,
            commands::undo_change,
            commands::confirm_tool,
            commands::list_projects,
            commands::pick_folder,
            commands::create_project,
            commands::get_project,
            commands::update_project,
            commands::delete_project,
            commands::reindex_project,
            commands::open_url,
            commands::open_path,
            commands::quit_app,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            {
                let st = handle.state::<Arc<AppState>>();
                updater::cleanup(&st);
                st.updates.write().current = handle.package_info().version.to_string();
            }
            #[cfg(windows)]
            if let Some(w) = handle.get_webview_window("main") {
                if let Ok(h) = w.hwnd() {
                    platform::set_window_icons(h.0 as isize);
                }
            }
            let menu = tray_menu(&handle, &UpdateInfo::default())?;
            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("Lumen – local AI · made by Akulav")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_main(app),
                    // the window asks for confirmation (and shows progress) before anything is installed
                    id @ ("app_update" | "rt_update" | "check") => {
                        show_main(app);
                        let _ = app.emit("update-prompt", json!({ "kind": id }));
                    }
                    "unload" => {
                        chat::stop_all(&app.state::<Arc<AppState>>());
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move { server::stop_llm(&app).await });
                    }
                    "quit" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            shutdown(&app).await;
                            app.exit(0);
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app);
            // (a Linux desktop without an app indicator has no tray: Lumen still runs, and closing the
            // window then quits instead of hiding it where nothing could bring it back)
            if let Err(e) = tray {
                let st = handle.state::<Arc<AppState>>();
                st.log(format!("[lumen] no tray icon on this desktop ({e}); closing the window quits Lumen"));
                st.no_tray.store(true, Ordering::Relaxed);
            }

            metrics::spawn(handle.clone());
            updater::spawn_background(handle.clone());

            // hassle-free start: fetch the runtime and load the last model in the background
            let st = handle.state::<Arc<AppState>>().inner().clone();
            let (autoload, last, backend) = {
                let s = st.settings.read();
                (s.autoload_model, s.last_model.clone(), s.backend.clone())
            };
            let h2 = handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(exe) = runtime::ensure(&h2, &backend, false).await {
                    if let Some((name, total)) = runtime::detect_gpu(&exe).await {
                        let mut i = st.info.write();
                        i.gpu_name = name;
                        i.vram_total = total;
                    }
                    server::emit_status(&h2);
                }
                if autoload {
                    if let Some(m) = last.filter(|m| st.paths.models.join(m).exists()) {
                        let _ = server::start_llm(&h2, &m).await;
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<Arc<AppState>>();
                if state.settings.read().close_to_tray && !state.quitting.load(Ordering::Relaxed) && !state.no_tray.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Lumen");

    app.run(|app, event| match event {
        RunEvent::Exit => {
            let app = app.clone();
            tauri::async_runtime::block_on(async move { shutdown(&app).await });
        }
        // macOS: clicking the Dock icon brings back a window that was closed to the menu bar
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    });
}
