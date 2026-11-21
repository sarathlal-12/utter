// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appctx;
mod audio;
mod cleanup;
mod engines;
mod hardware;
mod history;
mod hotkey;
mod log;
mod paste;
mod settings;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, Wry};

use engines::Engines;
use history::History;
use hotkey::HotkeyEvent;
use settings::Settings;

struct TrayItems {
    status: MenuItem<Wry>,
    help: MenuItem<Wry>,
    fast: CheckMenuItem<Wry>,
    ai: CheckMenuItem<Wry>,
    pill: CheckMenuItem<Wry>,
}

pub struct Shared {
    app: AppHandle,
    cfg_dir: PathBuf,
    settings: Mutex<Settings>,
    engines: Arc<Engines>,
    history: History,
    /// Only one dictation is processed at a time, so pastes stay in order.
    work: Mutex<()>,
    tray: OnceLock<TrayItems>,
    /// Last time the AI model was needed. It's unloaded after AI_IDLE without use.
    ai_used: Mutex<Instant>,
}

/// Free the AI model's ~500 MB after this long without dictation.
const AI_IDLE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Serialize)]
struct PillState<'a> {
    state: &'a str,
    msg: String,
    idle_visible: bool,
}

impl Shared {
    fn pill(&self, state: &str, msg: impl Into<String>) {
        let s = self.settings.lock().unwrap();
        let mut msg = msg.into();
        if state == "locked" && msg.is_empty() {
            msg = format!("Hands-free · {} to stop", s.hotkey);
        }
        let _ = self.app.emit_to("pill", "pill", PillState { state, msg, idle_visible: s.show_idle_pill });
    }

    fn set_status(&self, text: &str) {
        if let Some(t) = self.tray.get() {
            let _ = t.status.set_text(text);
        }
    }

    fn ready_text(&self) -> String {
        format!("Ready: hold {} to talk", self.settings.lock().unwrap().hotkey)
    }

    /// Keep tray checkmarks and labels in step with the settings.
    fn sync_tray(&self) {
        let s = self.settings.lock().unwrap().clone();
        if let Some(t) = self.tray.get() {
            let _ = t.fast.set_checked(s.fast_mode);
            let _ = t.ai.set_checked(s.ai_cleanup);
            let _ = t.ai.set_enabled(!s.fast_mode);
            let _ = t.pill.set_checked(s.show_idle_pill);
            let _ = t.help.set_text(format!("Hold {} to talk · +Space = hands-free · Esc = cancel", s.hotkey));
        }
    }

    /// Load the engines the current settings need (in the background).
    fn warm_up(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let s = me.settings.lock().unwrap().clone();
            me.set_status("Loading speech model…");
            me.pill("loading", "Loading…");
            match me.engines.ensure_whisper(s.whisper_model()) {
                Ok(_) => {
                    me.set_status(&me.ready_text());
                    me.pill("idle", "");
                }
                Err(e) => {
                    log!("whisper failed: {e}");
                    me.set_status("Speech engine failed to start");
                    me.pill("error", "Speech engine failed");
                }
            }
            // The AI model is not loaded here: it loads when you start talking (see prewarm_ai).
            if !s.use_ai() {
                me.engines.stop_llama();
            }
        });
    }

    /// Start loading the AI model while the user is still speaking, so it's ready by the time they stop.
    fn prewarm_ai(self: &Arc<Self>) {
        if !self.settings.lock().unwrap().use_ai() {
            return;
        }
        *self.ai_used.lock().unwrap() = Instant::now();
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = me.engines.ensure_llama() {
                log!("llama failed: {e}");
            }
        });
    }

    /// Unload the AI model after AI_IDLE without dictation.
    fn start_idle_reaper(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            let idle = me.ai_used.lock().unwrap().elapsed();
            // UTTER_AI_IDLE_SECS overrides the timeout (handy for testing).
            let limit = std::env::var("UTTER_AI_IDLE_SECS").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs).unwrap_or(AI_IDLE);
            if idle > limit && me.engines.llama_ready().is_some() && me.work.try_lock().is_ok() {
                me.engines.stop_llama();
                log!("AI model unloaded after {} min idle", idle.as_secs() / 60);
            }
        });
    }

    /// Apply settings saved from the settings window or the tray.
    fn apply(self: &Arc<Self>, new: Settings) -> Result<(), String> {
        let combo = hotkey::parse(&new.hotkey).ok_or("That hotkey isn't valid")?;
        let mut new = new;
        new.hotkey = hotkey::format(&combo);
        let old = std::mem::replace(&mut *self.settings.lock().unwrap(), new.clone());
        new.save(&self.cfg_dir);
        hotkey::set_combo(combo);
        if old.autostart != new.autostart {
            set_autostart(&self.app, new.autostart);
        }
        self.sync_tray();
        if old.fast_mode != new.fast_mode || old.ai_cleanup != new.ai_cleanup {
            self.warm_up();
        } else {
            self.set_status(&self.ready_text());
            self.pill("idle", "");
        }
        let _ = self.app.emit_to("settings", "config-changed", ());
        Ok(())
    }

    fn process(self: &Arc<Self>, samples: Vec<f32>, ctx: appctx::AppCtx) {
        let me = self.clone();
        std::thread::spawn(move || {
            let _guard = me.work.lock().unwrap();
            let secs = samples.len() as f32 / audio::RATE as f32;
            if audio::peak_rms(&samples) < 0.004 {
                me.pill("error", "Didn't hear anything");
                return;
            }
            me.pill("processing", "");
            let s = me.settings.lock().unwrap().clone();
            let t0 = Instant::now();
            // Pad with a little silence: Whisper is more reliable on clips over 1 s.
            let mut padded = samples;
            if padded.len() < audio::RATE as usize * 3 / 2 {
                padded.resize(audio::RATE as usize * 3 / 2, 0.0);
            }
            let raw = match me.engines.transcribe(s.whisper_model(), audio::to_wav(&padded), &s.whisper_prompt()) {
                Ok(t) => t,
                Err(e) => {
                    log!("transcribe error: {e}");
                    me.pill("error", "Transcription failed");
                    return;
                }
            };
            let t_stt = t0.elapsed();
            let tone = ctx.tone_with(&s.app_tones);
            let mut used_ai = false;
            let text = if let Some(snip) = cleanup::snippet(&raw, &s.snippets) {
                snip
            } else {
                let mut text = cleanup::apply_dictionary(&cleanup::rules(&raw, tone), &s.words, &s.replacements);
                if s.use_ai() && !text.is_empty() {
                    // Usually already loaded by prewarm_ai; if not, this waits for it (about 1-2 s).
                    *me.ai_used.lock().unwrap() = Instant::now();
                    if let Err(e) = me.engines.ensure_llama() {
                        log!("llama failed: {e}");
                    }
                    if let Some(polished) = cleanup::ai_polish(&me.engines, &text, tone) {
                        text = cleanup::apply_dictionary(&polished, &s.words, &s.replacements);
                        used_ai = true;
                    }
                }
                text
            };
            log!(
                "{secs:.1}s audio | stt {:?} | total {:?} | app {} ({tone:?}) | ai {used_ai} | {} chars",
                t_stt,
                t0.elapsed(),
                ctx.exe,
                text.len()
            );
            if text.is_empty() {
                me.pill("error", "Didn't catch that");
                return;
            }
            me.history.add(&ctx.exe, &raw, &text, secs);
            let _ = me.app.emit_to("settings", "history-changed", ());
            match paste::insert(&text) {
                Ok(()) => me.pill("done", ""),
                Err(e) => {
                    log!("paste error: {e}");
                    me.pill("error", "Couldn't paste (text is on clipboard)");
                }
            }
        });
    }
}

/// Owns the microphone stream (cpal streams must stay on one thread).
fn controller(shared: Arc<Shared>, rx: mpsc::Receiver<HotkeyEvent>) {
    let mut rec: Option<(audio::Recording, Instant, appctx::AppCtx)> = None;
    while let Ok(ev) = rx.recv() {
        match ev {
            HotkeyEvent::Start => {
                if rec.is_some() {
                    continue;
                }
                let ctx = appctx::foreground();
                let app = shared.app.clone();
                match audio::start(move |lvl| {
                    let _ = app.emit_to("pill", "level", lvl);
                }) {
                    Ok(r) => {
                        rec = Some((r, Instant::now(), ctx));
                        shared.pill("listening", "");
                        shared.prewarm_ai();
                    }
                    Err(e) => {
                        log!("mic error: {e}");
                        shared.pill("error", e);
                        hotkey::reset();
                    }
                }
            }
            HotkeyEvent::Lock => {
                if rec.is_some() {
                    shared.pill("locked", "");
                }
            }
            HotkeyEvent::Cancel => {
                if let Some((r, _, _)) = rec.take() {
                    drop(r.finish());
                    shared.pill("idle", "");
                }
            }
            HotkeyEvent::Stop => {
                if let Some((r, started, ctx)) = rec.take() {
                    let samples = r.finish();
                    if started.elapsed() < Duration::from_millis(350) {
                        shared.pill("idle", ""); // accidental tap
                    } else {
                        shared.process(samples, ctx);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ commands for the settings window

#[tauri::command]
fn get_config(state: State<Arc<Shared>>) -> serde_json::Value {
    let hw = hardware::detect();
    serde_json::json!({
        "settings": *state.settings.lock().unwrap(),
        "recent_apps": state.history.recent_apps(),
        "version": env!("CARGO_PKG_VERSION"),
        "hardware": { "ram_gb": (hw.ram_gb * 10.0).round() / 10.0, "threads": hw.threads, "tier": hw.tier },
    })
}

#[tauri::command]
fn save_config(state: State<Arc<Shared>>, settings: Settings) -> Result<(), String> {
    state.inner().apply(settings)
}

#[tauri::command]
fn get_history(state: State<Arc<Shared>>) -> Vec<history::Entry> {
    state.history.list()
}

#[tauri::command]
fn delete_history(state: State<Arc<Shared>>, id: u64) {
    state.history.delete(id);
}

#[tauri::command]
fn clear_history(state: State<Arc<Shared>>) {
    state.history.clear();
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).map_err(|e| e.to_string())
}

/// Hide the settings window so focus returns to the previous app, then paste there.
#[tauri::command]
fn paste_text(window: tauri::WebviewWindow, text: String) -> Result<(), String> {
    let _ = window.hide();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        if let Err(e) = paste::insert(&text) {
            log!("re-paste error: {e}");
        }
    });
    Ok(())
}

/// Record the next key combination. Returns "" if cancelled or timed out.
#[tauri::command]
async fn capture_hotkey() -> String {
    let (tx, rx) = mpsc::channel();
    hotkey::capture(tx);
    let got = tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)).ok())
        .await
        .ok()
        .flatten();
    if got.is_none() {
        hotkey::cancel_capture();
    }
    got.unwrap_or_default()
}

#[tauri::command]
fn cancel_capture() {
    hotkey::cancel_capture();
}

// ------------------------------------------------------------------ windows, tray, autostart

fn open_settings(app: &AppHandle, tab: &str) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.emit("open-tab", tab);
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let url = WebviewUrl::App("settings.html".into());
    if let Err(e) = WebviewWindowBuilder::new(app, "settings", url)
        .initialization_script(&format!("window.__UTTER_TAB = {:?};", tab))
        // Must match the pill window's args in tauri.conf.json (WebView2 shares one browser per app).
        .additional_browser_args("--disable-gpu --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --renderer-process-limit=1")
        .title("Utter")
        .inner_size(760.0, 580.0)
        .min_inner_size(560.0, 420.0)
        .center()
        .build()
    {
        log!("settings window error: {e}");
    }
}

fn set_autostart(app: &AppHandle, on: bool) {
    // Dev builds would register target\debug\utter.exe, so only release builds touch it.
    if cfg!(debug_assertions) {
        return;
    }
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let res = if on { al.enable() } else { al.disable() };
    if let Err(e) = res {
        log!("autostart change failed: {e}");
    }
}

#[cfg(windows)]
fn make_pill_passive(win: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if let Ok(hwnd) = win.hwnd() {
        let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT) as isize);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

fn place_pill(win: &tauri::WebviewWindow) {
    let _ = win.set_ignore_cursor_events(true);
    if let Ok(Some(mon)) = win.primary_monitor() {
        let wa = mon.work_area();
        let size = win.outer_size().unwrap_or(tauri::PhysicalSize::new(220, 60));
        let x = wa.position.x + (wa.size.width as i32 - size.width as i32) / 2;
        let y = wa.position.y + wa.size.height as i32 - size.height as i32 - (6.0 * mon.scale_factor()) as i32;
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
    #[cfg(windows)]
    make_pill_passive(win);
    #[cfg(not(windows))]
    let _ = win.show();
}

fn build_tray(app: &tauri::App, shared: Arc<Shared>) -> tauri::Result<()> {
    let s = shared.settings.lock().unwrap().clone();
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let help = MenuItem::with_id(app, "help", "", false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let history = MenuItem::with_id(app, "history", "History…", true, None::<&str>)?;
    let fast = CheckMenuItem::with_id(app, "fast", "Fast Mode (tiny model, no AI)", true, s.fast_mode, None::<&str>)?;
    let ai = CheckMenuItem::with_id(app, "ai", "AI cleanup", !s.fast_mode, s.ai_cleanup, None::<&str>)?;
    let pill = CheckMenuItem::with_id(app, "pill", "Show indicator when idle", true, s.show_idle_pill, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Utter", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(
        app,
        &[&status, &help, &sep()?, &settings, &history, &sep()?, &fast, &ai, &pill, &sep()?, &quit],
    )?;
    let _ = shared.tray.set(TrayItems { status, help, fast, ai, pill });
    shared.sync_tray();

    let sh = shared.clone();
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icon"))
        .tooltip("Utter: offline dictation")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            match id {
                "quit" => {
                    sh.engines.shutdown();
                    app.exit(0);
                }
                "settings" => open_settings(app, "general"),
                "history" => open_settings(app, "history"),
                "fast" | "ai" | "pill" => {
                    let mut s = sh.settings.lock().unwrap().clone();
                    match id {
                        "fast" => s.fast_mode = !s.fast_mode,
                        "ai" => s.ai_cleanup = !s.ai_cleanup,
                        _ => s.show_idle_pill = !s.show_idle_pill,
                    }
                    let _ = sh.apply(s);
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

/// Test mode: `utter --transcribe file.wav [app.exe]` prints raw and cleaned text, then exits.
fn transcribe_cli(res: PathBuf, cfg_dir: PathBuf, wav: &str, exe: Option<String>) {
    let (s, _) = Settings::load_or_detect(&cfg_dir);
    let engines = Engines::new(res);
    let samples = audio::read_wav(&std::fs::read(wav).expect("read wav")).expect("decode wav");
    let t0 = Instant::now();
    let raw = engines.transcribe(s.whisper_model(), audio::to_wav(&samples), &s.whisper_prompt()).expect("transcribe");
    let t_stt = t0.elapsed();
    let ctx = appctx::AppCtx { exe: exe.unwrap_or_default(), title: String::new() };
    let tone = ctx.tone_with(&s.app_tones);
    let rules = cleanup::apply_dictionary(&cleanup::rules(&raw, tone), &s.words, &s.replacements);
    let mut ai = None;
    if s.use_ai() {
        engines.ensure_llama().expect("llama");
        let t1 = Instant::now();
        ai = cleanup::ai_polish(&engines, &rules, tone).map(|t| (t, t1.elapsed()));
    }
    println!("model : {}  (stt {:?})", s.whisper_model(), t_stt);
    println!("raw   : {raw}");
    println!("rules : {rules}");
    if let Some((t, d)) = ai {
        println!("ai    : {t}  ({d:?})");
    }
    engines.shutdown();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_settings(app, "general")))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_history,
            delete_history,
            clear_history,
            copy_text,
            paste_text,
            capture_hotkey,
            cancel_capture
        ])
        .setup(move |app| {
            let cfg_dir = app.path().app_config_dir()?;
            log::init(app.path().app_log_dir()?);
            let res = app.path().resource_dir()?.join("resources");

            if let Some(i) = args.iter().position(|a| a == "--transcribe") {
                transcribe_cli(res, cfg_dir, &args[i + 1], args.get(i + 2).cloned());
                std::process::exit(0);
            }

            let (mut settings, first_run) = Settings::load_or_detect(&cfg_dir);
            let combo = hotkey::parse(&settings.hotkey).unwrap_or_else(|| {
                settings.hotkey = "Ctrl+Win".into();
                hotkey::parse("Ctrl+Win").unwrap()
            });
            log!("start: tier={} fast={} ai={} hotkey={}", settings.tier, settings.fast_mode, settings.ai_cleanup, settings.hotkey);
            if first_run {
                set_autostart(app.handle(), settings.autostart);
            }
            let shared = Arc::new(Shared {
                app: app.handle().clone(),
                history: History::load(app.path().app_data_dir()?),
                cfg_dir,
                settings: Mutex::new(settings),
                engines: Arc::new(Engines::new(res)),
                work: Mutex::new(()),
                tray: OnceLock::new(),
                ai_used: Mutex::new(Instant::now()),
            });
            app.manage(shared.clone());
            shared.start_idle_reaper();
            // Debug builds: UTTER_TEST_PREWARM=1 loads the AI model as if a dictation started (tests load/unload).
            #[cfg(debug_assertions)]
            if std::env::var("UTTER_TEST_PREWARM").is_ok() {
                shared.prewarm_ai();
            }

            if let Some(pill) = app.get_webview_window("pill") {
                place_pill(&pill);
            }
            build_tray(app, shared.clone())?;
            shared.warm_up();

            let (tx, rx) = mpsc::channel();
            hotkey::start(tx, combo);
            let sh = shared.clone();
            std::thread::Builder::new().name("controller".into()).spawn(move || controller(sh, rx))?;
            if first_run {
                open_settings(app.handle(), "welcome");
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Utter")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
            tauri::RunEvent::Exit => {
                if let Some(sh) = app.try_state::<Arc<Shared>>() {
                    sh.engines.shutdown();
                }
            }
            _ => {}
        });
}
