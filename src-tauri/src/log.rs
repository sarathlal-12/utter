//! Tiny append-only log file (no extra crates). Lives in the app log dir.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("utter.log");
    // Keep the log small: start fresh when it grows past 1 MB.
    if std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let _ = PATH.set(path);
}

pub fn write(msg: &str) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("[{ts}] {msg}\n");
    #[cfg(debug_assertions)]
    eprint!("{line}");
    if let Some(p) = PATH.get() {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::log::write(&format!($($t)*)) };
}
