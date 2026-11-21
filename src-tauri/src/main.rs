// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appctx;
mod audio;
mod cleanup;
mod engines;
mod hardware;
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
use tauri::{AppHandle, Emitter, Manager, Wry};

use engines::Engines;
use hotkey::HotkeyEvent;
use settings::Settings;

pub struct Shared {
    app: AppHandle,
    cfg_dir: PathBuf,
    settings: Mutex<Settings>,
    engines: Arc<Engines>,
    /// Only one dictation is processed at a time, so pastes stay in order.
    work: Mutex<()>,
    status: OnceLock<MenuItem<Wry>>,
}

#[derive(Clone, Serialize)]
struct PillState<'a> {
    state: &'a str,
    msg: String,
    idle_visible: bool,
}

impl Shared {
    fn pill(&self, state: &str, msg: impl Into<String>) {
        let idle_visible = self.settings.lock().unwrap().show_idle_pill;
        let _ = self.app.emit_to("pill", "pill", PillState { state, msg: msg.into(), idle_visible });
    }

    fn set_status(&self, text: &str) {
        if let Some(item) = self.status.get() {
            let _ = item.set_text(text);
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
                    me.set_status("Ready: hold Ctrl+Win to talk");
                    me.pill("idle", "");
                }
                Err(e) => {
                    log!("whisper failed: {e}");
                    me.set_status("Speech engine failed to start");
                    me.pill("error", "Speech engine failed");
                }
            }
            if s.use_ai() {
                if let Err(e) = me.engines.ensure_llama() {
                    log!("llama failed: {e}");
                }
            } else {
                me.engines.stop_llama();
            }
        });
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
            let raw = match me.engines.transcribe(s.whisper_model(), audio::to_wav(&padded), "") {
                Ok(t) => t,
                Err(e) => {
                    log!("transcribe error: {e}");
                    me.pill("error", "Transcription failed");
                    return;
                }
            };
            let t_stt = t0.elapsed();
            let tone = ctx.tone();
            let mut text = cleanup::rules(&raw, tone);
            let mut used_ai = false;
            if s.use_ai() && !text.is_empty() {
                if let Some(polished) = cleanup::ai_polish(&me.engines, &text, tone) {
                    text = polished;
                    used_ai = true;
                }
            }
            log!(
                "{secs:.1}s audio | stt {:?} | total {:?} | app {} ({tone:?}) | ai {used_ai} | raw {raw:?} -> {text:?}",
                t_stt,
                t0.elapsed(),
                ctx.exe
            );
            if text.is_empty() {
                me.pill("error", "Didn't catch that");
                return;
            }
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
    let fast = CheckMenuItem::with_id(app, "fast", "Fast Mode (tiny model, no AI)", true, s.fast_mode, None::<&str>)?;
    let ai = CheckMenuItem::with_id(app, "ai", "AI cleanup", !s.fast_mode, s.ai_cleanup, None::<&str>)?;
    let pill = CheckMenuItem::with_id(app, "pill", "Show indicator when idle", true, s.show_idle_pill, None::<&str>)?;
    let help = MenuItem::with_id(app, "help", "Hold Ctrl+Win to talk · +Space = hands-free · Esc = cancel", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Utter", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(app, &[&status, &help, &sep()?, &fast, &ai, &pill, &sep()?, &quit])?;
    let _ = shared.status.set(status);

    let (fast2, ai2, pill2) = (fast.clone(), ai.clone(), pill.clone());
    let sh = shared.clone();
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("icon"))
        .tooltip("Utter: hold Ctrl+Win to talk")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            if id == "quit" {
                sh.engines.shutdown();
                app.exit(0);
                return;
            }
            {
                let mut s = sh.settings.lock().unwrap();
                match id {
                    "fast" => s.fast_mode = !s.fast_mode,
                    "ai" => s.ai_cleanup = !s.ai_cleanup,
                    "pill" => s.show_idle_pill = !s.show_idle_pill,
                    _ => return,
                }
                s.save(&sh.cfg_dir);
                let _ = fast2.set_checked(s.fast_mode);
                let _ = ai2.set_checked(s.ai_cleanup);
                let _ = ai2.set_enabled(!s.fast_mode);
                let _ = pill2.set_checked(s.show_idle_pill);
            }
            if id == "pill" {
                sh.pill("idle", "");
            } else {
                sh.warm_up();
            }
        })
        .build(app)?;
    Ok(())
}

/// Test mode: `utter --transcribe file.wav [app.exe]` prints raw and cleaned text, then exits.
fn transcribe_cli(res: PathBuf, cfg_dir: PathBuf, wav: &str, exe: Option<String>) {
    let s = Settings::load_or_detect(&cfg_dir);
    let engines = Engines::new(res);
    let samples = audio::read_wav(&std::fs::read(wav).expect("read wav")).expect("decode wav");
    let t0 = Instant::now();
    let raw = engines.transcribe(s.whisper_model(), audio::to_wav(&samples), "").expect("transcribe");
    let t_stt = t0.elapsed();
    let ctx = appctx::AppCtx { exe: exe.unwrap_or_default(), title: String::new() };
    let tone = ctx.tone();
    let rules = cleanup::rules(&raw, tone);
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
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .setup(move |app| {
            let cfg_dir = app.path().app_config_dir()?;
            log::init(app.path().app_log_dir()?);
            let res = app.path().resource_dir()?.join("resources");

            if let Some(i) = args.iter().position(|a| a == "--transcribe") {
                transcribe_cli(res, cfg_dir, &args[i + 1], args.get(i + 2).cloned());
                std::process::exit(0);
            }

            let settings = Settings::load_or_detect(&cfg_dir);
            log!("start: res={} tier={} fast={} ai={}", res.display(), settings.tier, settings.fast_mode, settings.ai_cleanup);
            let shared = Arc::new(Shared {
                app: app.handle().clone(),
                cfg_dir,
                settings: Mutex::new(settings),
                engines: Arc::new(Engines::new(res)),
                work: Mutex::new(()),
                status: OnceLock::new(),
            });
            app.manage(shared.clone());

            if let Some(pill) = app.get_webview_window("pill") {
                place_pill(&pill);
            }
            build_tray(app, shared.clone())?;
            shared.warm_up();

            let (tx, rx) = mpsc::channel();
            hotkey::start(tx);
            let sh = shared.clone();
            std::thread::Builder::new().name("controller".into()).spawn(move || controller(sh, rx))?;
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
