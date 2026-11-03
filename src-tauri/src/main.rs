#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Lumen entry point (made by Akulav): plugins, tray icon, close-to-tray, background start-up
//! (runtime download + last-model autoload) and guaranteed shutdown of the llama-server children.

mod chat;
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
mod web;
mod win;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use state::AppState;

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Stops every child process. Safe to call more than once.
pub async fn shutdown(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    state.quitting.store(true, Ordering::Relaxed);
    for c in state.generations.lock().values() {
        c.store(true, Ordering::Relaxed);
    }
    server::stop_all(app).await;
}

fn main() {
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
            commands::get_logs,
            commands::list_chats,
            commands::create_chat,
            commands::search_chats,
            commands::export_chat,
            commands::get_chat,
            commands::rename_chat,
            commands::pin_chat,
            commands::set_chat_limit,
            commands::delete_chat,
            commands::send_message,
            commands::stop_generation,
            commands::regenerate,
            commands::edit_and_resend,
            commands::compress_chat,
            commands::undo_change,
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
            let show = MenuItem::with_id(app, "show", "Open Lumen", true, None::<&str>)?;
            let unload = MenuItem::with_id(app, "unload", "Unload model (free VRAM)", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Lumen", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &unload, &sep, &quit])?;
            TrayIconBuilder::with_id("lumen-tray")
                .icon(app.default_window_icon().cloned().expect("icon"))
                .tooltip("Lumen – local AI · made by Akulav")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_main(app),
                    "unload" => {
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
                .build(app)?;

            metrics::spawn(handle.clone());

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
                if state.settings.read().close_to_tray && !state.quitting.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Lumen");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let app = app.clone();
            tauri::async_runtime::block_on(async move { shutdown(&app).await });
        }
    });
}
