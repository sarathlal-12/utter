//! Past dictations, stored only on this computer. Entries older than 30 days are deleted.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const KEEP_SECS: u64 = 30 * 24 * 3600;
const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// Unix seconds.
    pub ts: u64,
    pub app: String,
    pub raw: String,
    pub text: String,
    pub secs: f32,
}

pub struct History {
    path: PathBuf,
    items: Mutex<Vec<Entry>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl History {
    pub fn load(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("history.json");
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Entry>>(&s).ok())
            .unwrap_or_default();
        let h = Self { path, items: Mutex::new(items) };
        h.prune_and_save();
        h
    }

    fn prune_and_save(&self) {
        let mut items = self.items.lock().unwrap();
        let cutoff = now().saturating_sub(KEEP_SECS);
        items.retain(|e| e.ts >= cutoff);
        let len = items.len();
        if len > MAX_ENTRIES {
            items.drain(..len - MAX_ENTRIES);
        }
        if let Ok(json) = serde_json::to_string(&*items) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    pub fn add(&self, app: &str, raw: &str, text: &str, secs: f32) {
        {
            let mut items = self.items.lock().unwrap();
            let ts = now();
            let id = items.last().map(|e| e.id + 1).unwrap_or(1).max(ts * 1000);
            items.push(Entry { id, ts, app: app.into(), raw: raw.into(), text: text.into(), secs });
        }
        self.prune_and_save();
    }

    /// Newest first.
    pub fn list(&self) -> Vec<Entry> {
        let mut v = self.items.lock().unwrap().clone();
        v.reverse();
        v
    }

    pub fn delete(&self, id: u64) {
        self.items.lock().unwrap().retain(|e| e.id != id);
        self.prune_and_save();
    }

    pub fn clear(&self) {
        self.items.lock().unwrap().clear();
        self.prune_and_save();
    }

    /// Apps you've dictated into recently (for the tone picker).
    pub fn recent_apps(&self) -> Vec<String> {
        let mut apps: Vec<String> = Vec::new();
        for e in self.items.lock().unwrap().iter().rev() {
            if !e.app.is_empty() && !apps.iter().any(|a| a.eq_ignore_ascii_case(&e.app)) {
                apps.push(e.app.clone());
            }
            if apps.len() >= 20 {
                break;
            }
        }
        apps
    }
}
