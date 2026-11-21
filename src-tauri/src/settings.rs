use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::hardware;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    /// What you say, e.g. "my email".
    pub trigger: String,
    /// What gets typed.
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppTone {
    /// Program name ("slack.exe") or a word in the window title ("gmail").
    pub app: String,
    /// "casual" | "formal" | "neutral" | "code"
    pub tone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Tiny speech model + rules-only cleanup. Near-instant on slow machines.
    pub fast_mode: bool,
    /// Polish text with the local language model (ignored in Fast Mode).
    pub ai_cleanup: bool,
    /// Show the small indicator at the bottom of the screen while idle.
    pub show_idle_pill: bool,
    /// Start Utter when you sign in to Windows.
    pub autostart: bool,
    /// Push-to-talk keys, e.g. "Ctrl+Win".
    pub hotkey: String,
    /// Names and jargon. Helps recognition and fixes their spelling.
    pub words: Vec<String>,
    /// Always replace one spelling with another.
    pub replacements: Vec<Replacement>,
    pub snippets: Vec<Snippet>,
    /// Your own tone rules; they win over the built-in ones.
    pub app_tones: Vec<AppTone>,
    /// What the hardware check decided on first launch ("low", "mid", "high").
    pub tier: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fast_mode: false,
            ai_cleanup: true,
            show_idle_pill: true,
            autostart: true,
            hotkey: "Ctrl+Win".into(),
            words: Vec::new(),
            replacements: Vec::new(),
            snippets: Vec::new(),
            app_tones: Vec::new(),
            tier: String::new(),
        }
    }
}

impl Settings {
    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    /// Load saved settings, or run the hardware check on first launch.
    /// The bool is true on first launch.
    pub fn load_or_detect(dir: &Path) -> (Settings, bool) {
        if let Ok(raw) = std::fs::read_to_string(Self::path(dir)) {
            if let Ok(s) = serde_json::from_str::<Settings>(&raw) {
                return (s, false);
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
        (s, true)
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

    /// Hint for Whisper so it spells your names and jargon right.
    pub fn whisper_prompt(&self) -> String {
        let words: Vec<&str> = self.words.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).take(60).collect();
        if words.is_empty() { String::new() } else { format!("Vocabulary: {}.", words.join(", ")) }
    }
}
