//! Shared application state: portable paths, persisted settings, server status and the
//! per-chat / per-download cancellation flags. One Arc<AppState> is managed by Tauri.

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::db::Db;
use crate::rag::ProjectIndex;
use crate::server::ServerProc;
use crate::win::Job;

/// Portable layout: everything lives next to the exe (falls back to %LOCALAPPDATA% when not writable).
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
        let mut root = exe_dir.join("LumenData");
        if std::fs::create_dir_all(&root).is_err() || !writable(&root) {
            let base = std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or(exe_dir);
            root = base.join("Lumen");
        }
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
    /// "wikipedia" (no key) | "brave" | "tavily" | "searxng"
    pub search_provider: String,
    pub search_key: String,
    pub searxng_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ui_lang: "en".into(),
            reply_lang: "auto".into(),
            backend: "vulkan".into(),
            last_model: None,
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
            search_provider: "wikipedia".into(),
            search_key: String::new(),
            searxng_url: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LlmInfo {
    /// "stopped" | "runtime" | "starting" | "ready" | "error"
    pub state: String,
    pub model: Option<String>,
    pub n_ctx: u32,
    pub port: u16,
    pub pid: u32,
    pub message: String,
    pub progress: f32,
    pub vram_total: u64,
    pub gpu_name: String,
}

pub struct AppState {
    pub paths: Paths,
    pub db: Db,
    pub settings: RwLock<Settings>,
    pub llm: tokio::sync::Mutex<Option<ServerProc>>,
    pub embed: tokio::sync::Mutex<Option<ServerProc>>,
    pub info: RwLock<LlmInfo>,
    pub embed_port: RwLock<u16>,
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub generations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub indexes: Mutex<HashMap<String, Arc<ProjectIndex>>>,
    pub indexing: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub http: reqwest::Client,
    pub job: Job,
    pub logs: Arc<Mutex<Vec<String>>>,
    /// tokens/s of the generation in flight (for the live status bar)
    pub live_tps: RwLock<f64>,
    pub quitting: AtomicBool,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let paths = Paths::resolve();
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
            embed_port: RwLock::new(0),
            downloads: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            indexes: Mutex::new(HashMap::new()),
            indexing: Mutex::new(HashMap::new()),
            http,
            job: Job::new(),
            logs: Arc::new(Mutex::new(Vec::new())),
            live_tps: RwLock::new(0.0),
            quitting: AtomicBool::new(false),
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

    pub fn log(&self, line: impl Into<String>) {
        let mut l = self.logs.lock();
        l.push(line.into());
        if l.len() > 600 {
            let n = l.len() - 600;
            l.drain(0..n);
        }
    }
}

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
