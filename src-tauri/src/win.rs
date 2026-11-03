//! Windows specifics: a kill-on-close job object (child servers die with the app, even on crash),
//! per-process RAM, and per-process VRAM through the "GPU Process Memory" performance counters.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
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

/// Reads dedicated + shared GPU memory used by one process (any vendor, WDDM counters).
pub struct GpuMemCounter {
    pid: u32,
    query: isize,
    dedicated: isize,
    shared: isize,
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
            let _ = PdhCollectQueryData(query);
            Some(GpuMemCounter { pid, query, dedicated, shared })
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// (dedicated, shared) bytes
    pub fn sample(&self) -> (u64, u64) {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return (0, 0);
            }
            (sum_array(self.dedicated), sum_array(self.shared))
        }
    }
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
