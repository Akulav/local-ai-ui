//! Windows side of `crate::platform` (Linux / macOS: unix.rs, same functions): a kill-on-close job
//! object (child servers die with the app, even on crash), hidden console windows, per-process RAM,
//! and per-process VRAM / GPU load through the "GPU Process Memory" / "GPU Engine" counters.

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::JobObjects::*;
use windows::Win32::System::Performance::*;
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::*;

pub struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> Self {
        unsafe {
            let h = CreateJobObjectW(None, PCWSTR::null()).unwrap_or_default();
            if !h.is_invalid() {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
            }
            Job(h)
        }
    }

    pub fn adopt(&self, pid: u32) {
        unsafe {
            if let Ok(p) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) {
                let _ = AssignProcessToJobObject(self.0, p);
                let _ = CloseHandle(p);
            }
        }
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Child processes never open a console window.
pub fn quiet(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// `quiet` for std commands.
fn quiet_std(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// (Unix: own process group.) On Windows `kill_tree` finds the children by itself.
pub fn own_group(_cmd: &mut tokio::process::Command) {}

/// Kills a process and everything it started.
pub fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    let mut c = std::process::Command::new("taskkill");
    c.args(["/T", "/F", "/PID", &pid.to_string()]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    quiet_std(&mut c);
    let _ = c.status();
}

/// The shell for `run_command`: (program, leading args, the command line passed verbatim).
pub fn shell(command: &str) -> (PathBuf, Vec<String>, Option<String>) {
    let comspec = std::env::var_os("ComSpec").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cmd.exe"));
    (comspec, vec!["/D".into(), "/S".into(), "/C".into()], Some(format!("\"{command}\"")))
}

/// Memory a new allocation can still get, in MiB: the free part of the commit limit (RAM + page file).
/// Windows refuses an allocation past the commit limit even while physical RAM is free.
pub fn available_memory_mb() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    Some(m.ullAvailPageFile / (1024 * 1024))
}

/// The app icon, embedded so the window can get crisp big and small icons at any DPI.
const APP_ICO: &[u8] = include_bytes!("../icons/icon.ico");

/// Picks the .ico entry closest to (and preferably not smaller than) `size` pixels.
fn ico_entry(ico: &[u8], size: u32) -> Option<&[u8]> {
    let n = u16::from_le_bytes([*ico.get(4)?, *ico.get(5)?]) as usize;
    let mut best: Option<(u32, &[u8])> = None;
    for i in 0..n {
        let e = ico.get(6 + 16 * i..6 + 16 * i + 16)?;
        let w = if e[0] == 0 { 256 } else { e[0] as u32 };
        let len = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let data = ico.get(off..off + len)?;
        let better = match best {
            None => true,
            Some((bw, _)) => (w >= size && (bw < size || w < bw)) || (bw < size && w > bw),
        };
        if better {
            best = Some((w, data));
        }
    }
    best.map(|(_, d)| d)
}

/// Sets both window icons. Tauri only sets ICON_SMALL (tao's `set_window_icon`), so the taskbar,
/// Alt+Tab and Task Manager's app group had no big icon and showed a generic one.
pub fn set_window_icons(hwnd: isize) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;
    let h = HWND(hwnd as *mut _);
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        let size = unsafe { GetSystemMetrics(metric) }.max(16) as u32;
        // the taskbar draws the big icon larger than SM_CXICON on high-DPI screens
        let want = if kind == ICON_BIG { (size * 2).min(256) } else { size };
        let Some(data) = ico_entry(APP_ICO, want) else { continue };
        if let Ok(icon) = unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, want as i32, want as i32, LR_DEFAULTCOLOR) } {
            unsafe { SendMessageW(h, WM_SETICON, WPARAM(kind as usize), LPARAM(icon.0 as isize)) };
        }
    }
}

/// Resident memory (working set) of a process, in bytes.
pub fn process_ram(pid: u32) -> u64 {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return 0 };
        let mut c = PROCESS_MEMORY_COUNTERS::default();
        let ok = K32GetProcessMemoryInfo(h, &mut c, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).as_bool();
        let _ = CloseHandle(h);
        if ok { c.WorkingSetSize as u64 } else { 0 }
    }
}

/// Total CPU time (kernel + user) a process has used so far, in 100 ns units.
pub fn process_cpu_time(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        ok.then(|| t(k) + t(u))
    }
}

/// Reads dedicated + shared GPU memory and GPU engine load of one process (any vendor, WDDM counters).
pub struct GpuMemCounter {
    pid: u32,
    query: isize,
    dedicated: isize,
    shared: isize,
    util: isize,
}

impl GpuMemCounter {
    pub fn new(pid: u32) -> Option<Self> {
        unsafe {
            let mut query = 0isize;
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut dedicated = 0isize;
            let mut shared = 0isize;
            let p1: Vec<u16> = format!("\\GPU Process Memory(pid_{pid}_*)\\Dedicated Usage\0").encode_utf16().collect();
            let p2: Vec<u16> = format!("\\GPU Process Memory(pid_{pid}_*)\\Shared Usage\0").encode_utf16().collect();
            if PdhAddEnglishCounterW(query, PCWSTR(p1.as_ptr()), 0, &mut dedicated) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            let _ = PdhAddEnglishCounterW(query, PCWSTR(p2.as_ptr()), 0, &mut shared);
            let mut util = 0isize;
            let p3: Vec<u16> = format!("\\GPU Engine(pid_{pid}_*)\\Utilization Percentage\0").encode_utf16().collect();
            let _ = PdhAddEnglishCounterW(query, PCWSTR(p3.as_ptr()), 0, &mut util);
            let _ = PdhCollectQueryData(query);
            Some(GpuMemCounter { pid, query, dedicated, shared, util })
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// (dedicated bytes, shared bytes, GPU load %)
    pub fn sample(&self) -> (u64, u64, f64) {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return (0, 0, 0.0);
            }
            let util = if self.util != 0 { busiest_engine(self.util) } else { 0.0 };
            (sum_array(self.dedicated), sum_array(self.shared), util)
        }
    }
}

/// Load of the busiest engine type (3D, Compute, Copy…) like Task Manager's GPU column: the
/// instances of one type are summed, the highest type wins.
unsafe fn busiest_engine(counter: isize) -> f64 {
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let _ = PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
    if size == 0 {
        return 0.0;
    }
    let mut buf = vec![0u8; size as usize + 64];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
        return 0.0;
    }
    let mut by_type: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for i in 0..count as usize {
        let it = &*items.add(i);
        let name = it.szName.to_string().unwrap_or_default();
        let kind = name.rsplit("engtype_").next().unwrap_or("").to_string();
        *by_type.entry(kind).or_default() += it.FmtValue.Anonymous.doubleValue.max(0.0);
    }
    by_type.values().cloned().fold(0.0, f64::max).min(100.0)
}

unsafe fn sum_array(counter: isize) -> u64 {
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let _ = PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, None);
    if size == 0 {
        return 0;
    }
    let mut buf = vec![0u8; size as usize + 64];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) != 0 {
        return 0;
    }
    let mut total: u64 = 0;
    for i in 0..count as usize {
        let it = &*items.add(i);
        total += it.FmtValue.Anonymous.largeValue.max(0) as u64;
    }
    total
}

impl Drop for GpuMemCounter {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_right_icon_size() {
        let size = |want: u32| {
            let d = ico_entry(APP_ICO, want).unwrap();
            // every entry of icon.ico is a PNG: width is at bytes 16..20 of the IHDR
            u32::from_be_bytes([d[16], d[17], d[18], d[19]])
        };
        assert_eq!(size(16), 16);
        assert_eq!(size(20), 24);
        assert_eq!(size(32), 32);
        assert_eq!(size(40), 48);
        assert_eq!(size(96), 256);
        assert_eq!(size(512), 256);
    }

    /// A real (hidden) window gets both icons, which Tauri alone does not give it.
    #[test]
    fn window_gets_big_and_small_icons() {
        use windows::core::w;
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let h = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("icon test"), WINDOW_STYLE(0), 0, 0, 10, 10, None, None, None, None).unwrap();
            let get = |kind: u32| SendMessageW(h, WM_GETICON, WPARAM(kind as usize), LPARAM(0)).0;
            assert_eq!(get(ICON_BIG), 0);
            set_window_icons(h.0 as isize);
            assert_ne!(get(ICON_BIG), 0, "big icon (taskbar / Alt+Tab / Task Manager)");
            assert_ne!(get(ICON_SMALL), 0, "small icon (title bar)");
            let _ = DestroyWindow(h);
        }
    }
}
