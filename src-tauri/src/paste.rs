//! Put text at the cursor: copy it to the clipboard (where it stays), then press Ctrl+V.
use std::time::{Duration, Instant};

pub fn insert(text: &str) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    cb.set_text(text.to_string()).map_err(|e| format!("clipboard: {e}"))?;
    drop(cb);
    #[cfg(windows)]
    win::paste();
    Ok(())
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

    fn key_down(vk: u16) -> bool {
        unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
    }

    fn input(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    pub fn paste() {
        // Wait until the user has let go of the hotkey, or Ctrl+V would become Win+Ctrl+V.
        let start = Instant::now();
        let mods = [VK_LWIN, VK_RWIN, VK_SHIFT, VK_MENU, VK_CONTROL];
        while mods.iter().any(|&m| key_down(m)) && start.elapsed() < Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(15));
        }
        std::thread::sleep(Duration::from_millis(30));
        let seq = [input(VK_CONTROL, false), input(0x56, false), input(0x56, true), input(VK_CONTROL, true)];
        unsafe {
            SendInput(seq.len() as u32, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }
}
