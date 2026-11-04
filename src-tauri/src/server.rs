//! Lifecycle of the llama-server child processes (chat model + optional embedding model).

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::state::{push_log, AppState};

pub struct ServerProc {
    pub child: Child,
    pub port: u16,
    pub pid: u32,
}

impl ServerProc {
    pub async fn kill(mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(39281)
}

pub fn emit_status(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let info = state.info.read().clone();
    let _ = app.emit("server-status", info);
}

fn set_status(app: &AppHandle, f: impl FnOnce(&mut crate::state::LlmInfo)) {
    {
        let state = app.state::<Arc<AppState>>();
        let mut i = state.info.write();
        f(&mut i);
    }
    emit_status(app);
}

/// Spawns llama-server without a console window, tied to the app's job object, with piped output.
fn spawn_server(state: &AppState, exe: &Path, args: &[String]) -> Result<(Child, u32)> {
    let child = Command::new(exe)
        .args(args)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().unwrap_or(0);
    state.job.adopt(pid);
    Ok((child, pid))
}

fn spawn_logger(child: &mut Child, logs: Arc<Mutex<Vec<String>>>, tag: &'static str, progress: Option<AppHandle>) {
    let push = move |logs: &Mutex<Vec<String>>, line: String| push_log(logs, format!("[{tag}] {line}"));
    if let Some(out) = child.stdout.take() {
        let logs = logs.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(out).lines();
            while let Ok(Some(line)) = r.next_line().await {
                push(&logs, line);
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(async move {
            let mut r = BufReader::new(err).lines();
            while let Ok(Some(line)) = r.next_line().await {
                if let Some(app) = &progress {
                    // model loading prints dots; surface a hint of activity
                    if line.contains("load_tensors") || line.contains("llama_context") || line.contains("fit") {
                        let short: String = line.chars().take(140).collect();
                        set_status(app, |i| i.message = short);
                    }
                }
                push(&logs, line);
            }
        });
    }
}

async fn wait_healthy(http: &reqwest::Client, port: u16, child: &mut Child, logs: &Arc<Mutex<Vec<String>>>) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/health");
    for _ in 0..1200 {
        if let Ok(Some(status)) = child.try_wait() {
            let tail: Vec<String> = logs.lock().iter().rev().take(14).rev().cloned().collect();
            return Err(anyhow!("llama-server exited ({status}).\n{}", tail.join("\n")));
        }
        if let Ok(r) = http.get(&url).timeout(Duration::from_secs(2)).send().await {
            if r.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(anyhow!("timed out waiting for llama-server"))
}

/// Stop the chat model server (if any).
pub async fn stop_llm(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    let proc = state.llm.lock().await.take();
    if let Some(p) = proc {
        state.log(format!("[lumen] stopping llama-server pid {}", p.pid));
        p.kill().await;
    }
    set_status(app, |i| {
        i.state = "stopped".into();
        i.model = None;
        i.n_ctx = 0;
        i.pid = 0;
        i.port = 0;
        i.message.clear();
        i.idle_unloaded_min = 0;
        i.loaded_at = 0;
    });
}

/// Frees VRAM after `minutes` without use (called by the metrics sampler). The status keeps the
/// reason so the UI can explain it; the next message reloads the model (`last_model`).
pub async fn unload_idle(app: &AppHandle, minutes: u32) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.log(format!("[lumen] unloading the model after {minutes} min idle"));
    stop_llm(app).await;
    set_status(app, |i| i.idle_unloaded_min = minutes);
}

/// Load `model_file` (relative to the models folder). Replaces the running model.
/// `--fit on` lets llama.cpp pick GPU layers and the largest context that fits the free VRAM
/// (never below 8k); the resulting n_ctx is read back from `/props` and drives the context meter.
pub async fn start_llm(app: &AppHandle, model_file: &str) -> Result<()> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let model_path = state.paths.models.join(model_file);
    if !model_path.exists() {
        return Err(anyhow!("model not found: {model_file}"));
    }
    stop_llm(app).await;
    let settings = state.settings.read().clone();

    set_status(app, |i| {
        i.state = "runtime".into();
        i.model = Some(model_file.into());
        i.message = "Preparing llama.cpp runtime…".into();
    });
    let exe = match crate::runtime::ensure(app, &settings.backend, false).await {
        Ok(e) => e,
        Err(e) => {
            set_status(app, |i| {
                i.state = "error".into();
                i.message = format!("Runtime download failed: {e}");
            });
            return Err(e);
        }
    };
    if state.info.read().vram_total == 0 {
        if let Some((name, total)) = crate::runtime::detect_gpu(&exe).await {
            let mut i = state.info.write();
            i.gpu_name = name;
            i.vram_total = total;
        }
    }

    let port = free_port();
    let mut args: Vec<String> = vec![
        "-m".into(),
        model_path.to_string_lossy().into(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--jinja".into(),
        "--reasoning-format".into(),
        "deepseek".into(),
        "--reasoning-budget".into(),
        "8192".into(),
        "--flash-attn".into(),
        "on".into(),
        "--cache-type-k".into(),
        settings.kv_cache.clone(),
        "--cache-type-v".into(),
        settings.kv_cache.clone(),
        "--parallel".into(),
        "1".into(),
        "--cache-reuse".into(),
        "256".into(),
        "--load-mode".into(),
        "none".into(),
        "--no-webui".into(),
        "--metrics".into(), // Prometheus counters for the engine panel (metrics.rs)
        "--fit".into(),
        "on".into(),
        "--fit-ctx".into(),
        "8192".into(),
    ];
    if settings.ctx_size > 0 {
        args.push("--ctx-size".into());
        args.push(settings.ctx_size.to_string());
    }
    if settings.n_cpu_moe > 0 {
        args.push("--n-cpu-moe".into());
        args.push(settings.n_cpu_moe.to_string());
    }
    if settings.backend == "cpu" {
        args.push("--gpu-layers".into());
        args.push("0".into());
    }

    state.log(format!("[lumen] starting llama-server {}", args.join(" ")));
    set_status(app, |i| {
        i.state = "starting".into();
        i.message = "Loading model into memory…".into();
    });

    let (mut child, pid) = spawn_server(&state, &exe, &args)?;
    spawn_logger(&mut child, state.logs.clone(), "llm", Some(app.clone()));
    set_status(app, |i| i.pid = pid);

    if let Err(e) = wait_healthy(&state.http, port, &mut child, &state.logs).await {
        let _ = child.start_kill();
        set_status(app, |i| {
            i.state = "error".into();
            i.pid = 0;
            i.message = e.to_string();
        });
        return Err(e);
    }

    let n_ctx = read_n_ctx(&state.http, port).await.unwrap_or(4096);
    *state.llm.lock().await = Some(ServerProc { child, port, pid });
    {
        let mut s = state.settings.write();
        s.last_model = Some(model_file.into());
    }
    state.save_settings();
    state.touch();
    set_status(app, |i| {
        i.state = "ready".into();
        i.port = port;
        i.n_ctx = n_ctx;
        i.message = String::new();
        i.loaded_at = crate::state::now_ms();
    });
    state.log(format!("[lumen] model ready on port {port}, n_ctx={n_ctx}"));
    Ok(())
}

async fn read_n_ctx(http: &reqwest::Client, port: u16) -> Option<u32> {
    let props: Value = http.get(format!("http://127.0.0.1:{port}/props")).send().await.ok()?.json().await.ok()?;
    let v = props["default_generation_settings"]["n_ctx"]
        .as_u64()
        .or_else(|| props["n_ctx"].as_u64());
    if let Some(v) = v {
        return Some(v as u32);
    }
    let slots: Value = http.get(format!("http://127.0.0.1:{port}/slots")).send().await.ok()?.json().await.ok()?;
    slots.get(0)?.get("n_ctx")?.as_u64().map(|v| v as u32)
}

// ---------------- embeddings server ----------------

pub const EMBED_REPO: &str = "Qwen/Qwen3-Embedding-0.6B-GGUF";
pub const EMBED_FILE: &str = "Qwen3-Embedding-0.6B-Q8_0.gguf";

/// Returns the port of a running embedding server, starting (and downloading) it if needed.
pub async fn ensure_embed(app: &AppHandle) -> Result<u16> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let mut guard = state.embed.lock().await;
    if let Some(p) = guard.as_mut() {
        if p.child.try_wait().ok().flatten().is_none() {
            return Ok(p.port);
        }
    }
    let model = state.paths.embed_models.join(EMBED_FILE);
    if !model.exists() {
        let url = format!("https://huggingface.co/{EMBED_REPO}/resolve/main/{EMBED_FILE}?download=true");
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::download::download(app, &state.http, "embed-model", "Embedding model (Qwen3-Embedding 0.6B)", &url, &model, cancel).await?;
    }
    let backend = state.settings.read().backend.clone();
    let exe = crate::runtime::ensure(app, &backend, false).await?;
    let port = free_port();
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).max(2)).unwrap_or(4);
    let args: Vec<String> = [
        "-m", &model.to_string_lossy(),
        "--host", "127.0.0.1", "--port", &port.to_string(),
        "--embedding", "--pooling", "last",
        "--ctx-size", "16384", "--parallel", "8", "--batch-size", "2048", "--ubatch-size", "2048",
        "--device", "none", "--gpu-layers", "0", "--fit", "off",
        "--threads", &threads.to_string(), "--no-webui",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let (mut child, pid) = spawn_server(&state, &exe, &args)?;
    spawn_logger(&mut child, state.logs.clone(), "embed", None);
    wait_healthy(&state.http, port, &mut child, &state.logs).await?;
    *guard = Some(ServerProc { child, port, pid });
    Ok(port)
}

pub async fn embed(app: &AppHandle, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    let port = ensure_embed(app).await?;
    let state = app.state::<Arc<AppState>>();
    let body = json!({ "input": inputs, "model": "embed" });
    let v: Value = state
        .http
        .post(format!("http://127.0.0.1:{port}/v1/embeddings"))
        .json(&body)
        .timeout(Duration::from_secs(600))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut out = vec![Vec::new(); inputs.len()];
    for d in v["data"].as_array().ok_or_else(|| anyhow!("bad embeddings response"))? {
        let idx = d["index"].as_u64().unwrap_or(0) as usize;
        let e: Vec<f32> = d["embedding"]
            .as_array()
            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
            .unwrap_or_default();
        if idx < out.len() {
            out[idx] = e;
        }
    }
    Ok(out)
}

pub async fn stop_all(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let llm = state.llm.lock().await.take();
    if let Some(p) = llm {
        p.kill().await;
    }
    let emb = state.embed.lock().await.take();
    if let Some(p) = emb {
        p.kill().await;
    }
}
