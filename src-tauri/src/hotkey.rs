//! Global push-to-talk via a low-level keyboard hook (Windows).
//!
//! Hold the hotkey (default Ctrl+Win)  -> talk; release to finish.
//! Hotkey, then Space                  -> hands-free (keeps listening after you let go).
//! Hotkey again                        -> finish hands-free.
//! Esc while listening                 -> cancel.
//! Any other key while holding the hotkey cancels, so normal shortcuts (Ctrl+Win+Left) keep working.
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum HotkeyEvent {
    Start,
    Lock,
    Stop,
    Cancel,
}

/// A logical key. Left/right modifiers count as the same key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Ctrl,
    Alt,
    Shift,
    Win,
    Vk(u32),
}

impl Key {
    pub fn from_vk(vk: u32) -> Key {
        match vk {
            0x11 | 0xA2 | 0xA3 => Key::Ctrl,
            0x12 | 0xA4 | 0xA5 => Key::Alt,
            0x10 | 0xA0 | 0xA1 => Key::Shift,
            0x5B | 0x5C => Key::Win,
            v => Key::Vk(v),
        }
    }

    pub fn is_modifier(self) -> bool {
        !matches!(self, Key::Vk(_))
    }

    fn name(self) -> String {
        match self {
            Key::Ctrl => "Ctrl".into(),
            Key::Alt => "Alt".into(),
            Key::Shift => "Shift".into(),
            Key::Win => "Win".into(),
            Key::Vk(v) => vk_name(v),
        }
    }
}

const NAMED: &[(u32, &str)] = &[
    (0x20, "Space"), (0x09, "Tab"), (0x0D, "Enter"), (0x14, "CapsLock"), (0x2D, "Insert"), (0x2E, "Delete"),
    (0x24, "Home"), (0x23, "End"), (0x21, "PageUp"), (0x22, "PageDown"), (0x13, "Pause"), (0x91, "ScrollLock"),
    (0x5D, "Menu"), (0xC0, "`"),
];

fn vk_name(v: u32) -> String {
    if let Some((_, n)) = NAMED.iter().find(|(k, _)| *k == v) {
        return n.to_string();
    }
    match v {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(v).unwrap().to_string(),
        0x70..=0x87 => format!("F{}", v - 0x6F),
        _ => format!("Key{v}"),
    }
}

fn vk_from_name(s: &str) -> Option<u32> {
    if let Some((k, _)) = NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)) {
        return Some(*k);
    }
    let up = s.to_uppercase();
    if up.len() == 1 {
        let c = up.chars().next().unwrap();
        if c.is_ascii_alphanumeric() {
            return Some(c as u32);
        }
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    up.strip_prefix("KEY").and_then(|n| n.parse().ok())
}

/// "Ctrl+Win" -> keys. Returns None for an empty or unknown combo.
pub fn parse(s: &str) -> Option<Vec<Key>> {
    let mut keys = Vec::new();
    for part in s.split('+').map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let k = match part.to_lowercase().as_str() {
            "ctrl" | "control" => Key::Ctrl,
            "alt" => Key::Alt,
            "shift" => Key::Shift,
            "win" | "windows" | "super" => Key::Win,
            _ => Key::Vk(vk_from_name(part)?),
        };
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    (!keys.is_empty()).then_some(keys)
}

pub fn format(keys: &[Key]) -> String {
    let order = |k: &Key| match k { Key::Ctrl => 0, Key::Alt => 1, Key::Shift => 2, Key::Win => 3, Key::Vk(_) => 4 };
    let mut v = keys.to_vec();
    v.sort_by_key(order);
    v.iter().map(|k| k.name()).collect::<Vec<_>>().join("+")
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Holding,
    Locked,
    /// Finished or cancelled while keys are still down: wait for release.
    Done,
}

struct State {
    combo: Vec<Key>,
    down: Vec<Key>,
    phase: Phase,
    combo_used: bool,
    /// Non-modifier keys whose key-up we must also swallow.
    swallowed: Vec<Key>,
    /// Some(..) while the settings window is recording a new hotkey.
    capture: Option<Vec<Key>>,
}

static TX: OnceLock<Mutex<Sender<HotkeyEvent>>> = OnceLock::new();
static CAPTURE_TX: Mutex<Option<Sender<String>>> = Mutex::new(None);
static STATE: Mutex<State> = Mutex::new(State {
    combo: Vec::new(),
    down: Vec::new(),
    phase: Phase::Idle,
    combo_used: false,
    swallowed: Vec::new(),
    capture: None,
});

fn send(ev: HotkeyEvent) {
    if let Some(tx) = TX.get() {
        let _ = tx.lock().unwrap().send(ev);
    }
}

pub fn set_combo(keys: Vec<Key>) {
    let mut s = STATE.lock().unwrap();
    s.combo = keys;
    s.phase = Phase::Idle;
}

/// Tell the hook the app ended a session some other way (e.g. a mic error).
pub fn reset() {
    let mut s = STATE.lock().unwrap();
    s.phase = if s.down.is_empty() { Phase::Idle } else { Phase::Done };
}

/// Record the next key combination the user presses. Sends its name when all keys are released.
/// Esc on its own cancels (sends an empty string).
pub fn capture(tx: Sender<String>) {
    *CAPTURE_TX.lock().unwrap() = Some(tx);
    STATE.lock().unwrap().capture = Some(Vec::new());
}

pub fn cancel_capture() {
    STATE.lock().unwrap().capture = None;
    *CAPTURE_TX.lock().unwrap() = None;
}

#[cfg(windows)]
pub fn start(tx: Sender<HotkeyEvent>, combo: Vec<Key>) {
    let _ = TX.set(Mutex::new(tx));
    set_combo(combo);
    std::thread::Builder::new()
        .name("hotkey-hook".into())
        .spawn(|| unsafe { win::run() })
        .expect("hotkey thread");
}

#[cfg(not(windows))]
pub fn start(_tx: Sender<HotkeyEvent>, _combo: Vec<Key>) {}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const SPACE: Key = Key::Vk(0x20);
    const ESC: Key = Key::Vk(0x1B);
    /// Unassigned key code: tapping it stops Windows opening Start (Win) or the menu bar (Alt) on release.
    const VK_MASK: u16 = 0xE8;

    pub unsafe fn run() {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), GetModuleHandleW(std::ptr::null()), 0);
        if hook.is_null() {
            crate::log!("could not install keyboard hook");
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(hook);
    }

    fn mask_key() {
        let mk = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
            },
        };
        let seq = [mk(false), mk(true)];
        unsafe {
            SendInput(2, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }

    unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let kb = &*(lparam as *const KBDLLHOOKSTRUCT);
            // Ignore our own injected keys (mask key, Ctrl+V paste).
            if kb.flags & LLKHF_INJECTED == 0 {
                let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
                if handle(Key::from_vk(kb.vkCode), down) {
                    return 1; // swallow
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    fn held(s: &State) -> bool {
        !s.combo.is_empty() && s.combo.iter().all(|k| s.down.contains(k))
    }

    fn handle_capture(s: &mut State, k: Key, down: bool) -> bool {
        let keys = s.capture.as_mut().unwrap();
        if down {
            if !keys.contains(&k) {
                keys.push(k);
            }
            if !s.down.contains(&k) {
                s.down.push(k);
            }
            return true;
        }
        s.down.retain(|d| *d != k);
        if s.down.is_empty() {
            let keys = s.capture.take().unwrap();
            let name = if keys == [ESC] { String::new() } else { format(&keys) };
            if let Some(tx) = CAPTURE_TX.lock().unwrap().take() {
                let _ = tx.send(name);
            }
            if keys.iter().any(|k| matches!(k, Key::Win | Key::Alt)) {
                mask_key();
            }
        }
        // Let modifier releases through so Windows never thinks they're stuck.
        !k.is_modifier()
    }

    /// Returns true when the key should be swallowed.
    fn handle(k: Key, down: bool) -> bool {
        let mut s = STATE.lock().unwrap();
        if s.capture.is_some() {
            return handle_capture(&mut s, k, down);
        }
        let in_combo = s.combo.contains(&k);

        if down {
            let repeat = s.down.contains(&k);
            if !repeat {
                s.down.push(k);
            }
            if in_combo {
                // Swallow non-modifier combo keys (e.g. Space in Ctrl+Space) while the combo is in use.
                let swallow = !k.is_modifier() && (held(&s) || s.phase != Phase::Idle || s.swallowed.contains(&k));
                if !repeat && held(&s) {
                    match s.phase {
                        Phase::Idle => {
                            s.phase = Phase::Holding;
                            s.combo_used = true;
                            send(HotkeyEvent::Start);
                        }
                        Phase::Locked => {
                            s.phase = Phase::Done;
                            s.combo_used = true;
                            send(HotkeyEvent::Stop);
                        }
                        _ => {}
                    }
                }
                if swallow && !s.swallowed.contains(&k) {
                    s.swallowed.push(k);
                }
                return swallow && (held(&s) || s.swallowed.contains(&k));
            }
            if repeat {
                return s.swallowed.contains(&k);
            }
            match s.phase {
                Phase::Holding => {
                    if k == SPACE {
                        s.phase = Phase::Locked;
                        s.swallowed.push(k);
                        send(HotkeyEvent::Lock);
                        return true;
                    }
                    s.phase = Phase::Done;
                    send(HotkeyEvent::Cancel);
                    if k == ESC {
                        s.swallowed.push(k);
                        return true;
                    }
                    return false;
                }
                Phase::Locked if k == ESC => {
                    s.phase = Phase::Idle;
                    s.swallowed.push(k);
                    send(HotkeyEvent::Cancel);
                    return true;
                }
                _ => {}
            }
            return false;
        }

        // key up
        s.down.retain(|d| *d != k);
        let was_swallowed = s.swallowed.contains(&k);
        s.swallowed.retain(|d| *d != k);
        if in_combo {
            if s.combo_used && matches!(k, Key::Win | Key::Alt) {
                mask_key();
            }
            if s.phase == Phase::Holding {
                s.phase = Phase::Done;
                send(HotkeyEvent::Stop);
            }
            if !s.combo.iter().any(|c| s.down.contains(c)) {
                s.combo_used = false;
                if s.phase == Phase::Done {
                    s.phase = Phase::Idle;
                }
            }
        }
        was_swallowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        assert_eq!(format(&parse("win+ctrl").unwrap()), "Ctrl+Win");
        assert_eq!(format(&parse("Ctrl+Alt").unwrap()), "Ctrl+Alt");
        assert_eq!(format(&parse("ctrl+shift+space").unwrap()), "Ctrl+Shift+Space");
        assert_eq!(format(&parse("F9").unwrap()), "F9");
        assert!(parse("").is_none());
        assert!(parse("Ctrl+Banana").is_none());
    }
}
