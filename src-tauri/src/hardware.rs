//! First-launch hardware check: picks models that will run smoothly.

pub struct Hardware {
    pub ram_gb: f64,
    pub threads: usize,
    pub tier: &'static str,
}

pub fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2)
}

/// Threads to give each engine: half the logical cores, between 2 and 8.
pub fn engine_threads() -> usize {
    (threads() / 2).clamp(2, 8)
}

#[cfg(windows)]
fn total_ram_gb() -> f64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            return m.ullTotalPhys as f64 / 1024f64.powi(3);
        }
    }
    4.0
}

#[cfg(not(windows))]
fn total_ram_gb() -> f64 {
    4.0
}

pub fn detect() -> Hardware {
    let ram_gb = total_ram_gb();
    let threads = threads();
    let tier = if ram_gb < 6.0 || threads <= 2 {
        "low"
    } else if ram_gb < 7.5 || threads < 4 {
        "mid"
    } else {
        "high"
    };
    Hardware { ram_gb, threads, tier }
}
