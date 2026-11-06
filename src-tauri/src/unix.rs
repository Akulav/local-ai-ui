//! Linux / macOS side of `crate::platform` (Windows: win.rs). Same functions, so the rest of the app
//! does not care which system it runs on:
//!
//! - child processes: no console to hide; on Linux they get SIGKILL when Lumen dies (parent-death
//!   signal), the Unix counterpart of the Windows job object. macOS has no such signal: children are
//!   stopped by the normal shutdown.
//! - process RAM / CPU through `sysinfo`; per-process VRAM and GPU load are not available (0).
//! - free memory (RAM + swap) for llama-server's RAM prompt cache cap (server.rs).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use sysinfo::{MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Nothing to do on Unix: see `quiet` for how children are tied to Lumen.
pub struct Job;

impl Job {
    pub fn new() -> Self {
        Job
    }

    pub fn adopt(&self, _pid: u32) {}
}

impl Default for Job {
    fn default() -> Self {
        Job::new()
    }
}

/// Prepares a child process: on Linux it is killed when Lumen dies (even on a crash).
/// (The signal follows the spawning *thread*: spawn from async code, whose worker threads live as long
/// as the app, never from `spawn_blocking`.)
pub fn quiet(cmd: &mut tokio::process::Command) {
    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// Puts a tool command in its own process group, so `kill_tree` can stop it with its children.
pub fn own_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

/// Kills a process started with `own_group` and everything it started.
pub fn kill_tree(pid: u32) {
    if pid != 0 {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
}

/// The shell for `run_command`: (program, leading args, the command as one more argument).
pub fn shell(command: &str) -> (PathBuf, Vec<String>, Option<String>) {
    (PathBuf::from("/bin/sh"), vec!["-c".into()], Some(command.to_string()))
}

fn system() -> &'static Mutex<System> {
    static SYS: OnceLock<Mutex<System>> = OnceLock::new();
    SYS.get_or_init(|| Mutex::new(System::new()))
}

/// (resident memory in bytes, accumulated CPU time in ms) of one process.
fn sample(pid: u32) -> Option<(u64, u64)> {
    let mut sys = system().lock().ok()?;
    let pid = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing().with_memory().with_cpu());
    sys.process(pid).map(|p| (p.memory(), p.accumulated_cpu_time()))
}

/// Resident memory of a process, in bytes.
pub fn process_ram(pid: u32) -> u64 {
    sample(pid).map(|s| s.0).unwrap_or(0)
}

/// Total CPU time a process has used so far, in 100 ns units (the Windows unit metrics.rs expects).
pub fn process_cpu_time(pid: u32) -> Option<u64> {
    sample(pid).map(|s| s.1.saturating_mul(10_000))
}

/// Memory a new allocation can still get, in MiB: available RAM plus free swap.
pub fn available_memory_mb() -> Option<u64> {
    let mut sys = system().lock().ok()?;
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
    Some((sys.available_memory() + sys.free_swap()) / (1024 * 1024))
}

/// Per-process GPU memory / load has no portable source on Linux and macOS: the engine panel shows 0.
pub struct GpuMemCounter {
    pid: u32,
}

impl GpuMemCounter {
    pub fn new(_pid: u32) -> Option<Self> {
        None
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// (dedicated bytes, shared bytes, GPU load %)
    pub fn sample(&self) -> (u64, u64, f64) {
        (0, 0, 0.0)
    }
}
