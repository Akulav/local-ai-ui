//! Tauri commands invoked from the UI (invoke(...) in ui/app.js). Kept thin: they validate input
//! and delegate to the modules; long-running work is spawned and reports back through events.

use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::chat::{self, SendOpts};
use crate::state::{default_title, AppState, Settings};
use crate::{models, rag, runtime, server, tools, updater};

type S<'a> = State<'a, Arc<AppState>>;
type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// Everything the UI needs on boot: settings, server status, catalog, version.
#[tauri::command]
pub fn get_state(app: AppHandle, state: S) -> Value {
    let s = state.settings.read().clone();
    json!({
        "settings": s,
        "info": *state.info.read(),
        "root": state.paths.root.to_string_lossy(),
        "runtime": runtime::installed_version(&state, &s.backend),
        "catalog": models::catalog(),
        "version": app.package_info().version.to_string(),
        // windows | linux | macos: the UI shows the GPU backends this system has
        "os": std::env::consts::OS,
        "updates": *state.updates.read(),
    })
}

#[tauri::command]
pub fn set_settings(app: AppHandle, state: S, settings: Settings) {
    *state.settings.write() = settings;
    state.save_settings();
    let updates = state.updates.read().clone();
    crate::refresh_tray(&app, &updates); // tray labels follow the UI language
}

#[tauri::command]
pub fn list_models(state: S) -> Value {
    json!(models::local_models(&state))
}

#[tauri::command]
pub async fn delete_model(app: AppHandle, file: String) -> R<()> {
    models::delete_model(&app, &file).await.map_err(e)
}

#[tauri::command]
pub async fn download_model(app: AppHandle, repo: String, path: String) -> R<String> {
    models::download_model(&app, &repo, &path).await.map_err(e)
}

#[tauri::command]
pub fn cancel_download(state: S, id: String) {
    models::cancel_download(&state, &id);
}

#[tauri::command]
pub async fn list_repo(state: S<'_>, input: String) -> R<Value> {
    models::list_repo(&state, &input).await.map_err(e)
}

#[tauri::command]
pub async fn search_hf(state: S<'_>, query: String) -> R<Value> {
    models::search_hf(&state, &query).await.map_err(e)
}

#[tauri::command]
pub async fn load_model(app: AppHandle, file: String) -> R<()> {
    server::start_llm(&app, &file).await.map_err(e)
}

#[tauri::command]
pub async fn unload_model(app: AppHandle) {
    server::stop_llm(&app).await;
}

#[tauri::command]
pub async fn update_runtime(app: AppHandle) -> R<String> {
    runtime::update(&app).await.map_err(e)
}

/// Checks GitHub for a newer Lumen and llama.cpp now (the background check runs every few hours).
#[tauri::command]
pub async fn check_updates(app: AppHandle) -> Value {
    json!(updater::check(&app).await)
}

/// Downloads, verifies and installs the offered Lumen release; the app restarts when it succeeds.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> R<()> {
    updater::install_app(&app).await.map_err(e)
}

#[tauri::command]
pub fn get_logs(state: S) -> Vec<String> {
    state.logs.lock().clone()
}

// ---------------- chats ----------------

#[tauri::command]
pub fn list_chats(state: S, project_id: Option<String>) -> R<Value> {
    state.db.chats(project_id.as_deref()).map(|v| json!(v)).map_err(e)
}

#[tauri::command]
pub fn create_chat(state: S, project_id: Option<String>) -> R<Value> {
    let title = default_title(&state.settings.read().ui_lang);
    state.db.create_chat(project_id.as_deref(), title).map(|c| json!(c)).map_err(e)
}

/// Ids of chats whose messages contain `query` (the sidebar search also matches titles itself).
#[tauri::command]
pub fn search_chats(state: S, query: String) -> R<Vec<String>> {
    let q = query.trim();
    if q.chars().count() < 2 {
        return Ok(vec![]);
    }
    state.db.search_chats(q).map_err(e)
}

/// Asks where to save, then writes the chat as Markdown. Returns the saved path (None if cancelled).
#[tauri::command]
pub async fn export_chat(app: AppHandle, id: String) -> R<Option<String>> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let chat = state.db.chat(&id).map_err(e)?.ok_or("chat not found")?;
    let md = chat::export_markdown(&chat, &state.db.messages(&id).map_err(e)?);
    let name: String = chat
        .title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim()
        .to_string();
    let name = if name.is_empty() { "chat".to_string() } else { name };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(format!("{name}.md"))
        .add_filter("Markdown", &["md"])
        .save_file(move |f| {
            let _ = tx.send(f.and_then(|p| p.into_path().ok()));
        });
    let Some(path) = rx.await.ok().flatten() else { return Ok(None) };
    std::fs::write(&path, md).map_err(e)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn get_chat(state: S, id: String) -> R<Value> {
    let chat = state.db.chat(&id).map_err(e)?.ok_or("chat not found")?;
    let messages = state.db.messages(&id).map_err(e)?;
    let budget = chat::budget(&state, &chat);
    let busy = state.generations.lock().contains_key(&id);
    Ok(json!({"chat": chat, "messages": messages, "budget": budget, "busy": busy}))
}

#[tauri::command]
pub fn rename_chat(state: S, id: String, title: String) -> R<()> {
    state.db.rename_chat(&id, title.trim()).map_err(e)
}

#[tauri::command]
pub fn pin_chat(state: S, id: String, pinned: bool) -> R<()> {
    state.db.pin_chat(&id, pinned).map_err(e)
}

#[tauri::command]
pub fn set_chat_limit(state: S, id: String, limit: u32) -> R<u32> {
    state.db.set_chat_limit(&id, limit).map_err(e)?;
    let chat = state.db.chat(&id).map_err(e)?.ok_or("chat not found")?;
    Ok(chat::budget(&state, &chat))
}

/// Links a chat to a project (its files and tools become available from the next message) or unlinks it.
#[tauri::command]
pub fn set_chat_project(state: S, id: String, project_id: Option<String>) -> R<()> {
    if state.generations.lock().contains_key(&id) {
        return Err("Wait until the reply is finished, then move the chat.".into());
    }
    if let Some(p) = &project_id {
        state.db.project(p).map_err(e)?.ok_or("project not found")?;
    }
    state.db.set_chat_project(&id, project_id.as_deref()).map_err(e)
}

#[tauri::command]
pub fn delete_chat(state: S, id: String) -> R<()> {
    chat::stop(&state, &id);
    state.db.delete_chat(&id).map_err(e)
}

#[tauri::command]
pub fn send_message(app: AppHandle, chat_id: String, text: String, think: bool, web: bool, research: bool) {
    tauri::async_runtime::spawn(async move {
        let _ = chat::send(app, SendOpts { chat_id, text, think, web, research }).await;
    });
}

#[tauri::command]
pub fn stop_generation(state: S, chat_id: String) {
    chat::stop(&state, &chat_id);
}

#[tauri::command]
pub fn regenerate(app: AppHandle, chat_id: String, think: bool, web: bool) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = chat::regenerate(app.clone(), chat_id.clone(), think, web).await {
            chat::emit(&app, &chat_id, "error", json!({"error": err.to_string()}));
        }
    });
}

#[tauri::command]
pub fn edit_and_resend(app: AppHandle, state: S, chat_id: String, message_id: String, text: String, think: bool, web: bool) -> R<()> {
    let msgs = state.db.messages(&chat_id).map_err(e)?;
    let m = msgs.iter().find(|m| m.id == message_id).ok_or("message not found")?;
    let research = m.meta["research"].as_bool().unwrap_or(false);
    state.db.truncate_from(&chat_id, m.seq).map_err(e)?;
    tauri::async_runtime::spawn(async move {
        let _ = chat::send(app, SendOpts { chat_id, text, think, web, research }).await;
    });
    Ok(())
}

#[tauri::command]
pub fn compress_chat(app: AppHandle, chat_id: String) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) = chat::compress_now(app.clone(), chat_id.clone()).await {
            chat::emit(&app, &chat_id, "error", json!({"error": err.to_string()}));
        }
    });
}

#[tauri::command]
pub fn undo_change(state: S, backup_id: String) -> R<String> {
    tools::undo(&state, &backup_id).map_err(e)
}

/// The user's answer to a tool that wants to run a process: "allow", "always" (this chat) or "deny".
#[tauri::command]
pub fn confirm_tool(state: S, id: String, decision: String) {
    if let Some(tx) = state.approvals.lock().remove(&id) {
        let d = if matches!(decision.as_str(), "allow" | "always") { decision } else { "deny".into() };
        let _ = tx.send(d);
    }
}

// ---------------- projects ----------------

#[tauri::command]
pub fn list_projects(state: S) -> R<Value> {
    state.db.projects().map(|v| json!(v)).map_err(e)
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |f| {
        let _ = tx.send(f.map(|p| p.to_string()));
    });
    rx.await.ok().flatten()
}

#[tauri::command]
pub fn create_project(app: AppHandle, state: S, name: String, folder: String) -> R<Value> {
    if !std::path::Path::new(&folder).is_dir() {
        return Err("folder does not exist".into());
    }
    let name = if name.trim().is_empty() {
        std::path::Path::new(&folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("Project".into())
    } else {
        name.trim().to_string()
    };
    let p = state.db.create_project(&name, &folder).map_err(e)?;
    let id = p.id.clone();
    tauri::async_runtime::spawn(async move {
        let _ = rag::index_project(&app, &id).await;
    });
    Ok(json!(p))
}

#[tauri::command]
pub fn get_project(state: S, id: String) -> R<Value> {
    let p = state.db.project(&id).map_err(e)?.ok_or("project not found")?;
    let indexing = state.indexing.lock().contains_key(&id);
    Ok(json!({"project": p, "stats": rag::stats(&state, &id), "indexing": indexing}))
}

#[tauri::command]
pub fn update_project(state: S, id: String, name: String, instructions: String) -> R<()> {
    state.db.update_project(&id, &name, &instructions).map_err(e)
}

#[tauri::command]
pub fn delete_project(state: S, id: String) -> R<()> {
    state.indexes.lock().remove(&id);
    state.db.delete_project(&id).map_err(e)
}

#[tauri::command]
pub fn reindex_project(app: AppHandle, id: String) {
    tauri::async_runtime::spawn(async move {
        let _ = rag::index_project(&app, &id).await;
    });
}

// ---------------- misc ----------------

#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> R<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only web links can be opened".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(e)
}

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> R<()> {
    app.opener().open_path(path, None::<&str>).map_err(e)
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    crate::shutdown(&app).await;
    app.exit(0);
}
