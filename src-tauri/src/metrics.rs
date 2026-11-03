//! Background sampler for the status bar: llama-server RAM/VRAM plus live and average tokens/s.

use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;
use crate::win::{process_ram, GpuMemCounter};

/// Samples RAM/VRAM of the running llama-server once per second and emits `metrics`.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut counter: Option<GpuMemCounter> = None;
        let mut empty_samples = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let state = app.state::<Arc<AppState>>();
            if state.quitting.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let (pid, total, model) = {
                let i = state.info.read();
                (i.pid, i.vram_total, i.model.clone())
            };
            let (mut ram, mut vram, mut shared) = (0u64, 0u64, 0u64);
            if pid != 0 {
                if counter.as_ref().map(|c| c.pid()) != Some(pid) {
                    counter = GpuMemCounter::new(pid);
                    empty_samples = 0;
                }
                ram = process_ram(pid);
                if let Some(c) = &counter {
                    let (d, s) = c.sample();
                    vram = d;
                    shared = s;
                    // wildcard instances may not exist yet when the query was created; rebuild periodically
                    if d == 0 {
                        empty_samples += 1;
                        if empty_samples % 5 == 0 {
                            counter = GpuMemCounter::new(pid);
                        }
                    }
                }
            } else {
                counter = None;
            }
            let live = *state.live_tps.read();
            let avg = model.as_deref().map(|m| state.db.avg_tps(m)).unwrap_or(0.0);
            let _ = app.emit(
                "metrics",
                json!({ "ram": ram, "vram": vram, "shared": shared, "vram_total": total, "tps": live, "avg_tps": avg }),
            );
        }
    });
}
