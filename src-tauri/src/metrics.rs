//! Background sampler for the status bar and the engine panel: llama-server RAM/VRAM, CPU and GPU
//! load, live/average tokens/s and the server's own counters (`/metrics`). Everything stays local.

use serde_json::{json, Map, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{now_ms, AppState};
use crate::platform::{process_cpu_time, process_ram, GpuMemCounter};

/// Unloads the chat model once it has been ready and unused for `idle_unload_min` minutes.
/// Never while a chat is generating / compressing / researching (those register in `generations`).
fn check_idle(app: &AppHandle, state: &AppState) {
    let minutes = state.settings.read().idle_unload_min;
    if minutes == 0 || state.info.read().state != "ready" || !state.generations.lock().is_empty() {
        return;
    }
    let idle_ms = now_ms() - state.last_activity.load(Ordering::Relaxed);
    if idle_ms < minutes as i64 * 60_000 {
        return;
    }
    state.touch(); // do not fire again while the unload is in progress
    let app = app.clone();
    tauri::async_runtime::spawn(async move { crate::server::unload_idle(&app, minutes).await });
}

/// llama-server's Prometheus text (`--metrics`) → {"prompt_tokens_total": 15, …} (prefix dropped).
pub fn parse_prometheus(text: &str) -> Map<String, Value> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (name, value) = l.trim().rsplit_once(' ')?;
            let v: f64 = value.parse().ok()?;
            let name = name.split('{').next()?.trim_start_matches("llamacpp:");
            Some((name.to_string(), json!(v)))
        })
        .collect()
}

/// Fetches the server counters; quick timeout so a busy server never stalls the sampler.
fn engine_counters(state: &AppState, port: u16) -> Option<Map<String, Value>> {
    let http = state.http.clone();
    let text = tauri::async_runtime::block_on(async move {
        http.get(format!("http://127.0.0.1:{port}/metrics")).timeout(Duration::from_millis(800)).send().await.ok()?.text().await.ok()
    })?;
    let m = parse_prometheus(&text);
    (!m.is_empty()).then_some(m)
}

/// CPU share of one process since the previous sample, normalised to all cores (like Task Manager).
struct CpuMeter {
    pid: u32,
    last: Option<(Instant, u64)>,
    cores: f64,
}

impl CpuMeter {
    fn sample(&mut self, pid: u32) -> f64 {
        if pid != self.pid {
            *self = CpuMeter { pid, last: None, cores: self.cores };
        }
        let Some(t) = process_cpu_time(pid) else { return 0.0 };
        let now = Instant::now();
        let pct = match self.last {
            Some((at, prev)) => {
                let wall = now.duration_since(at).as_secs_f64() * 1e7; // 100 ns units
                if wall > 0.0 { (t.saturating_sub(prev) as f64 / wall / self.cores * 100.0).min(100.0) } else { 0.0 }
            }
            None => 0.0,
        };
        self.last = Some((now, t));
        pct
    }
}

/// Samples the running llama-server once per second and emits `metrics`.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut counter: Option<GpuMemCounter> = None;
        let mut empty_samples = 0u32;
        let cores = std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0);
        let mut cpu = CpuMeter { pid: 0, last: None, cores };
        let mut engine: Option<Map<String, Value>> = None;
        let mut tick = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            tick += 1;
            let state = app.state::<Arc<AppState>>();
            if state.quitting.load(Ordering::Relaxed) {
                break;
            }
            let (pid, total, model, port, ready) = {
                let i = state.info.read();
                (i.pid, i.vram_total, i.model.clone(), i.port, i.state == "ready")
            };
            let (mut ram, mut vram, mut shared, mut gpu, mut cpu_pct) = (0u64, 0u64, 0u64, 0f64, 0f64);
            if pid != 0 {
                if counter.as_ref().map(|c| c.pid()) != Some(pid) {
                    counter = GpuMemCounter::new(pid);
                    empty_samples = 0;
                }
                ram = process_ram(pid);
                cpu_pct = cpu.sample(pid);
                if let Some(c) = &counter {
                    let (d, s, u) = c.sample();
                    vram = d;
                    shared = s;
                    gpu = u;
                    // wildcard instances may not exist yet when the query was created; rebuild periodically
                    if d == 0 {
                        empty_samples += 1;
                        if empty_samples.is_multiple_of(5) {
                            counter = GpuMemCounter::new(pid);
                        }
                    }
                }
            } else {
                counter = None;
            }
            if !ready {
                engine = None;
            } else if tick.is_multiple_of(2) {
                engine = engine_counters(&state, port);
            }
            crate::server::check_alive(&app);
            check_idle(&app, &state);
            let live = *state.live_tps.read();
            let avg = model.as_deref().map(|m| state.db.avg_tps(m)).unwrap_or(0.0);
            let _ = app.emit(
                "metrics",
                json!({
                    "ram": ram, "vram": vram, "shared": shared, "vram_total": total,
                    "tps": live, "avg_tps": avg, "cpu": cpu_pct, "gpu": gpu, "engine": engine,
                }),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_llama_server_metrics() {
        // captured from llama-server b12061 --metrics
        let text = "# HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.\n\
                    # TYPE llamacpp:prompt_tokens_total counter\n\
                    llamacpp:prompt_tokens_total 15\n\
                    llamacpp:prompt_tokens_seconds 716.64\n\
                    llamacpp:requests_processing 0\n";
        let m = parse_prometheus(text);
        assert_eq!(m.len(), 3);
        assert_eq!(m["prompt_tokens_total"], json!(15.0));
        assert_eq!(m["prompt_tokens_seconds"], json!(716.64));
    }

    /// Real counters while llama-server generates on the GPU. Opt-in:
    /// `set LUMEN_TEST_SERVER=…\llama-server.exe` and `LUMEN_TEST_MODEL=…\small.gguf`, then `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_llama_server_telemetry() {
        let (Ok(exe), Ok(model)) = (std::env::var("LUMEN_TEST_SERVER"), std::env::var("LUMEN_TEST_MODEL")) else {
            eprintln!("LUMEN_TEST_SERVER / LUMEN_TEST_MODEL not set - skipped");
            return;
        };
        let port = 39556u16;
        let mut child = std::process::Command::new(&exe)
            .args(["-m", &model, "--port", &port.to_string(), "--host", "127.0.0.1", "--metrics", "--ctx-size", "4096", "--no-webui", "--fit", "off", "--gpu-layers", "99"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let http = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{port}");
        rt.block_on(async {
            for _ in 0..120 {
                if http.get(format!("{base}/health")).send().await.is_ok_and(|r| r.status().is_success()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
        let counter = GpuMemCounter::new(pid).expect("GPU counters available");
        let mut cpu = CpuMeter { pid: 0, last: None, cores: std::thread::available_parallelism().unwrap().get() as f64 };
        cpu.sample(pid);
        // keep the GPU busy while sampling
        let gen = rt.spawn({
            let (http, base) = (http.clone(), base.clone());
            async move {
                let body = json!({"messages": [{"role": "user", "content": "Count from 1 to 400, one number per line."}], "max_tokens": 1500});
                let _ = http.post(format!("{base}/v1/chat/completions")).json(&body).send().await;
            }
        });
        let (mut max_gpu, mut max_cpu, mut vram) = (0f64, 0f64, 0u64);
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(1000));
            let (d, _s, u) = counter.sample();
            vram = vram.max(d);
            max_gpu = max_gpu.max(u);
            max_cpu = max_cpu.max(cpu.sample(pid));
        }
        let _ = rt.block_on(gen);
        let text = rt.block_on(async { http.get(format!("{base}/metrics")).send().await.unwrap().text().await.unwrap() });
        let m = parse_prometheus(&text);
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("vram={vram} gpu_max={max_gpu:.1}% cpu_max={max_cpu:.1}% metrics={m:?}");
        assert!(vram > 0, "dedicated VRAM measured");
        assert!(max_gpu > 1.0, "GPU load measured while generating");
        assert!(m["tokens_predicted_total"].as_f64().unwrap() > 0.0, "server counters read");
    }
}
