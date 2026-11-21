//! Global push-to-talk via a low-level keyboard hook (Windows).
//!
//! Hold Ctrl+Win          -> talk; release to finish.
//! Ctrl+Win, then Space   -> hands-free (keeps listening after you let go).
//! Ctrl+Win again         -> finish hands-free.
//! Esc while listening    -> cancel.
//! Any other key while holding Ctrl+Win (e.g. Ctrl+Win+Left) cancels, so normal shortcuts keep working.
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum HotkeyEvent {
    Start,
    Lock,
    Stop,
    Cancel,
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Holding,
    Locked,
    /// Finished or cancelled while keys are still down: wait for full release.
    Done,
}

struct State {
    ctrl: bool,
    win: bool,
    phase: Phase,
    combo_used: bool,
    swallow_space_up: bool,
}

static TX: OnceLock<Mutex<Sender<HotkeyEvent>>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State { ctrl: false, win: false, phase: Phase::Idle, combo_used: false, swallow_space_up: false });

fn send(ev: HotkeyEvent) {
    if let Some(tx) = TX.get() {
        let _ = tx.lock().unwrap().send(ev);
    }
}

/// Tell the hook the app finished a hands-free session some other way (e.g. error).
pub fn reset() {
    let mut s = STATE.lock().unwrap();
    s.phase = if s.ctrl || s.win { Phase::Done } else { Phase::Idle };
}

#[cfg(windows)]
pub fn start(tx: Sender<HotkeyEvent>) {
    let _ = TX.set(Mutex::new(tx));
    std::thread::Builder::new()
        .name("hotkey-hook".into())
        .spawn(|| unsafe { win::run() })
        .expect("hotkey thread");
}

#[cfg(not(windows))]
pub fn start(_tx: Sender<HotkeyEvent>) {}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const VK_LCTRL: u32 = 0xA2;
    const VK_RCTRL: u32 = 0xA3;
    const VK_LWIN_: u32 = 0x5B;
    const VK_RWIN_: u32 = 0x5C;
    const VK_SPACE_: u32 = 0x20;
    const VK_ESC: u32 = 0x1B;
    /// Unassigned key code: tapping it stops Windows from opening Start when Win is released.
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
                if handle(kb.vkCode, down) {
                    return 1; // swallow
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Returns true when the key should be swallowed.
    fn handle(vk: u32, down: bool) -> bool {
        let mut s = STATE.lock().unwrap();
        let is_ctrl = vk == VK_LCTRL || vk == VK_RCTRL;
        let is_win = vk == VK_LWIN_ || vk == VK_RWIN_;

        if down {
            if is_ctrl || is_win {
                if is_ctrl { s.ctrl = true; } else { s.win = true; }
                if s.ctrl && s.win {
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
                return false;
            }
            match s.phase {
                Phase::Holding => {
                    if vk == VK_SPACE_ {
                        s.phase = Phase::Locked;
                        s.swallow_space_up = true;
                        send(HotkeyEvent::Lock);
                        return true;
                    }
                    s.phase = Phase::Done;
                    send(HotkeyEvent::Cancel);
                    return vk == VK_ESC;
                }
                Phase::Locked if vk == VK_ESC => {
                    s.phase = Phase::Idle;
                    send(HotkeyEvent::Cancel);
                    return true;
                }
                _ => {}
            }
            return false;
        }

        // key up
        if vk == VK_SPACE_ && s.swallow_space_up {
            s.swallow_space_up = false;
            return true;
        }
        if is_ctrl || is_win {
            if is_win && s.combo_used {
                mask_key();
            }
            if is_ctrl { s.ctrl = false; } else { s.win = false; }
            match s.phase {
                Phase::Holding => {
                    s.phase = Phase::Done;
                    send(HotkeyEvent::Stop);
                }
                Phase::Done if !s.ctrl && !s.win => s.phase = Phase::Idle,
                _ => {}
            }
            if !s.ctrl && !s.win {
                s.combo_used = false;
                if s.phase == Phase::Done {
                    s.phase = Phase::Idle;
                }
            }
        }
        false
    }
}
