//! Shared application state: portable paths, persisted settings, server status and the
//! per-chat / per-download cancellation flags. One Arc<AppState> is managed by Tauri.

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use crate::db::Db;
use crate::rag::ProjectIndex;
use crate::server::ServerProc;
use crate::updater::UpdateInfo;
use crate::platform::Job;

/// Where everything lives: `LumenData` next to the program (Windows install folder, the Linux install in
/// ~/.local/share/lumen, a portable copy), else the user's app-data folder. macOS always uses
/// ~/Library/Application Support/Lumen: an app bundle must not be written into.
#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub runtime: PathBuf,
    pub models: PathBuf,
    pub embed_models: PathBuf,
    pub data: PathBuf,
    pub backups: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let root = if cfg!(target_os = "macos") {
            user_data_dir().unwrap_or_else(|| exe_dir.join("LumenData"))
        } else {
            let beside = exe_dir.join("LumenData");
            if std::fs::create_dir_all(&beside).is_ok() && writable(&beside) {
                beside
            } else {
                user_data_dir().unwrap_or(beside)
            }
        };
        let _ = std::fs::create_dir_all(&root);
        let p = Paths {
            runtime: root.join("runtime"),
            models: root.join("models"),
            embed_models: root.join("models").join("embedding"),
            data: root.join("data"),
            backups: root.join("data").join("backups"),
            root,
        };
        for d in [&p.runtime, &p.models, &p.embed_models, &p.data, &p.backups] {
            let _ = std::fs::create_dir_all(d);
        }
        p
    }
}

/// The per-user app-data folder: %LOCALAPPDATA%\Lumen, ~/Library/Application Support/Lumen,
/// or $XDG_DATA_HOME/lumen (~/.local/share/lumen).
pub fn user_data_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Lumen"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        return Some(home.join("Library").join("Application Support").join("Lumen"));
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local").join("share"));
    Some(data.join("lumen"))
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".probe");
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: "en" | "ro"
    pub ui_lang: String,
    /// Reply language: "auto" | "en" | "ro"
    pub reply_lang: String,
    /// llama.cpp backend: "vulkan" | "rocm" | "cpu"
    pub backend: String,
    pub last_model: Option<String>,
    /// Context window of the model loaded last (set by the backend, kept by `set_settings`): the context
    /// meter's scale while no model is loaded. 0 = no model loaded yet.
    pub last_n_ctx: u32,
    /// 0 = auto (fit to VRAM)
    pub ctx_size: u32,
    pub kv_cache: String,
    pub n_cpu_moe: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub thinking: bool,
    /// fraction of the context window that triggers compression
    pub compress_at: f32,
    pub semantic_search: bool,
    pub close_to_tray: bool,
    pub autoload_model: bool,
    /// Unload the chat model after this many idle minutes to free VRAM (0 = never).
    /// The UI reloads it automatically on the next message.
    pub idle_unload_min: u32,
    /// Look for new Lumen / llama.cpp releases on GitHub in the background.
    pub check_updates: bool,
    /// "duckduckgo" (no key) | "wikipedia" (no key) | "brave" | "tavily" | "searxng"
    pub search_provider: String,
    pub search_key: String,
    pub searxng_url: String,
    /// Always-on guidance from the user, appended to every chat's system prompt.
    pub custom_instructions: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ui_lang: "en".into(),
            reply_lang: "auto".into(),
            backend: "vulkan".into(),
            last_model: None,
            last_n_ctx: 0,
            ctx_size: 0,
            kv_cache: "q8_0".into(),
            n_cpu_moe: 0,
            temperature: 0.7,
            top_p: 0.9,
            thinking: true,
            compress_at: 0.72,
            semantic_search: true,
            close_to_tray: true,
            autoload_model: true,
            idle_unload_min: 30,
            check_updates: true,
            search_provider: "duckduckgo".into(),
            search_key: String::new(),
            searxng_url: String::new(),
            custom_instructions: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LlmInfo {
    /// "stopped" | "runtime" | "starting" | "ready" | "error"
    pub state: String,
    /// Set when llama-server died on its own ("exit code 0xc0000005 (access violation)"); cleared on load.
    pub crash: String,
    pub model: Option<String>,
    pub n_ctx: u32,
    pub port: u16,
    pub pid: u32,
    pub message: String,
    pub progress: f32,
    pub vram_total: u64,
    pub gpu_name: String,
    /// Set when the model was unloaded for being idle (minutes), so the UI can say why; 0 otherwise.
    pub idle_unloaded_min: u32,
    /// When the current model became ready (ms), for the engine uptime.
    pub loaded_at: i64,
}

/// The reply being written in a chat (or the tool it is running): sent with `get_chat` so the window
/// can pick the reply up live after you were in another chat or page.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LiveReply {
    /// the id of the streaming message (the `chat-event`s that follow carry it)
    pub temp_id: String,
    pub content: String,
    pub reasoning: String,
    pub tokens: u32,
    pub tps: f64,
    /// a tool being run between two replies: {id, name, arguments}
    pub tool: Option<serde_json::Value>,
}

pub struct AppState {
    pub paths: Paths,
    pub db: Db,
    pub settings: RwLock<Settings>,
    pub llm: tokio::sync::Mutex<Option<ServerProc>>,
    pub embed: tokio::sync::Mutex<Option<ServerProc>>,
    pub info: RwLock<LlmInfo>,
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub generations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// What a running reply has produced so far, per chat: a chat opened mid-reply shows it live.
    pub live: Mutex<HashMap<String, LiveReply>>,
    /// Chats named from their first message, waiting for the model's title after the reply.
    pub provisional_titles: Mutex<std::collections::HashSet<String>>,
    pub indexes: Mutex<HashMap<String, Arc<ProjectIndex>>>,
    pub indexing: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub http: reqwest::Client,
    pub job: Job,
    pub logs: Arc<Mutex<Vec<String>>>,
    /// tokens/s of the generation in flight (for the live status bar)
    pub live_tps: RwLock<f64>,
    /// Last time the chat model was used (ms), for the idle auto-unload in metrics.rs.
    pub last_activity: AtomicI64,
    pub updates: RwLock<UpdateInfo>,
    pub quitting: AtomicBool,
    /// No tray icon on this desktop (Linux without an app indicator): closing the window quits.
    pub no_tray: AtomicBool,
    /// Tool calls that wait for the user to allow a process (git, checks): confirm id → answer.
    pub approvals: Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>,
    /// Per chat, the process scopes ("git", "run") the user allowed for the rest of the session.
    pub allowed_tools: Mutex<HashMap<String, std::collections::HashSet<String>>>,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let paths = Paths::resolve();
        init_log_file(&paths.root.join("logs"));
        let db = Db::open(&paths.data.join("lumen.db"))?;
        let settings = load_settings(&paths);
        let http = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36")
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(AppState {
            paths,
            db,
            settings: RwLock::new(settings),
            llm: tokio::sync::Mutex::new(None),
            embed: tokio::sync::Mutex::new(None),
            info: RwLock::new(LlmInfo { state: "stopped".into(), ..Default::default() }),
            downloads: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            provisional_titles: Mutex::new(Default::default()),
            indexes: Mutex::new(HashMap::new()),
            indexing: Mutex::new(HashMap::new()),
            http,
            job: Job::new(),
            logs: Arc::new(Mutex::new(Vec::new())),
            live_tps: RwLock::new(0.0),
            last_activity: AtomicI64::new(now_ms()),
            updates: RwLock::new(UpdateInfo::default()),
            quitting: AtomicBool::new(false),
            no_tray: AtomicBool::new(false),
            approvals: Mutex::new(HashMap::new()),
            allowed_tools: Mutex::new(HashMap::new()),
        })
    }

    pub fn save_settings(&self) {
        let s = self.settings.read().clone();
        if let Ok(txt) = serde_json::to_string_pretty(&s) {
            let _ = std::fs::write(self.paths.data.join("settings.json"), txt);
        }
    }

    pub fn llm_url(&self) -> Option<String> {
        let info = self.info.read();
        (info.state == "ready").then(|| format!("http://127.0.0.1:{}", info.port))
    }

    /// Marks the chat model as used now (resets the idle auto-unload timer).
    pub fn touch(&self) {
        self.last_activity.store(now_ms(), Ordering::Relaxed);
    }

    pub fn log(&self, line: impl Into<String>) {
        push_log(&self.logs, line.into());
    }
}

/// Lines kept in the in-memory log shown under Settings → Server logs.
const MAX_LOG_LINES: usize = 600;

/// The on-disk copy of the log (LumenData/logs/lumen.log), so the reason for a crash survives the app
/// closing. Rotated to lumen.old.log at 8 MB.
struct LogFile {
    path: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
}
static LOG_FILE: std::sync::OnceLock<Mutex<LogFile>> = std::sync::OnceLock::new();
const LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Opens LumenData/logs/lumen.log for appending (once, at start-up).
pub fn init_log_file(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("lumen.log");
    let open = |p: &Path| std::fs::OpenOptions::new().create(true).append(true).open(p).ok();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let _ = LOG_FILE.set(Mutex::new(LogFile { file: open(&path), size, path }));
    push_log_file(&format!("----- Lumen {} started -----", env!("CARGO_PKG_VERSION")));
}

fn push_log_file(line: &str) {
    use std::io::Write;
    let Some(lf) = LOG_FILE.get() else { return };
    let mut lf = lf.lock();
    if lf.size > LOG_MAX_BYTES {
        lf.file = None;
        let _ = std::fs::rename(&lf.path, lf.path.with_file_name("lumen.old.log"));
        lf.file = std::fs::OpenOptions::new().create(true).append(true).open(&lf.path).ok();
        lf.size = 0;
    }
    let (now, ..) = crate::calc::local_now();
    let row = format!("{now} {line}\n");
    lf.size += row.len() as u64;
    if let Some(f) = lf.file.as_mut() {
        let _ = f.write_all(row.as_bytes());
    }
}

/// Appends a line to the shared log ring (also used by the llama-server output readers) and to the log file.
pub fn push_log(logs: &Mutex<Vec<String>>, line: String) {
    push_log_file(&line);
    let mut l = logs.lock();
    l.push(line);
    if l.len() > MAX_LOG_LINES {
        let n = l.len() - MAX_LOG_LINES;
        l.drain(0..n);
    }
}

/// Default title of a fresh chat; titles still equal to one of these get auto-named after the first turn.
pub const DEFAULT_TITLES: [&str; 2] = ["New chat", "Chat nou"];

pub fn default_title(ui_lang: &str) -> &'static str {
    if ui_lang == "ro" { DEFAULT_TITLES[1] } else { DEFAULT_TITLES[0] }
}

/// Modification time of a file in Unix seconds (0 when unknown).
pub fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Honest User-Agent for services whose API etiquette asks for one (Wikipedia).
pub const LUMEN_UA: &str = concat!("Lumen/", env!("CARGO_PKG_VERSION"), " (local desktop AI assistant; llama.cpp)");

fn load_settings(paths: &Paths) -> Settings {
    std::fs::read_to_string(paths.data.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Log lines reach LumenData/logs/lumen.log, not just the in-memory ring.
    #[test]
    fn log_lines_are_written_to_disk() {
        let state = AppState::new().unwrap();
        let marker = format!("[llm] test line {}", new_id());
        push_log(&state.logs, marker.clone());
        let text = std::fs::read_to_string(state.paths.root.join("logs").join("lumen.log")).unwrap();
        let line = text.lines().find(|l| l.ends_with(&marker)).expect("line in the log file");
        println!("{line}");
        assert!(line.len() > marker.len() + 18, "has a timestamp");
    }
}
