use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::hardware;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Tiny speech model + rules-only cleanup. Near-instant on slow machines.
    pub fast_mode: bool,
    /// Polish text with the local language model (ignored in Fast Mode).
    pub ai_cleanup: bool,
    /// Show the small indicator at the bottom of the screen while idle.
    pub show_idle_pill: bool,
    /// What the hardware check decided on first launch ("low", "mid", "high").
    pub tier: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { fast_mode: false, ai_cleanup: true, show_idle_pill: true, tier: String::new() }
    }
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Load saved settings, or run the hardware check on first launch.
    pub fn load_or_detect(dir: &Path) -> Settings {
        if let Ok(raw) = std::fs::read_to_string(Self::path(dir)) {
            if let Ok(s) = serde_json::from_str::<Settings>(&raw) {
                return s;
            }
        }
        let hw = hardware::detect();
        let mut s = Settings::default();
        s.tier = hw.tier.to_string();
        match hw.tier {
            "low" => { s.fast_mode = true; s.ai_cleanup = false; }
            "mid" => { s.fast_mode = false; s.ai_cleanup = false; }
            _ => { s.fast_mode = false; s.ai_cleanup = true; }
        }
        crate::log!("first launch: {:.1} GB RAM, {} threads -> tier {}", hw.ram_gb, hw.threads, hw.tier);
        s.save(dir);
        s
    }

    pub fn save(&self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(dir), json);
        }
    }

    pub fn whisper_model(&self) -> &'static str {
        if self.fast_mode { "ggml-tiny.en-q5_1.bin" } else { "ggml-base.en-q5_1.bin" }
    }

    pub fn use_ai(&self) -> bool {
        self.ai_cleanup && !self.fast_mode
    }
}
