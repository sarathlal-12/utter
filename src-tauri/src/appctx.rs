//! Which app is the user typing into? Used to pick a writing tone.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Casual,
    Formal,
    Neutral,
    Code,
}

#[derive(Clone, Debug, Default)]
pub struct AppCtx {
    pub exe: String,
    pub title: String,
}

impl AppCtx {
    pub fn tone(&self) -> Tone {
        let exe = self.exe.to_lowercase();
        let exe = exe.trim_end_matches(".exe");
        let title = self.title.to_lowercase();
        const CASUAL: &[&str] = &["slack", "discord", "whatsapp", "telegram", "signal", "ms-teams", "teams", "messenger", "skype", "zoom", "element"];
        const FORMAL: &[&str] = &["outlook", "olk", "thunderbird", "winword", "mailspring", "hxoutlook", "notion", "obsidian"];
        const CODE: &[&str] = &["code", "cursor", "windsurf", "devenv", "idea64", "pycharm64", "webstorm64", "rider64", "clion64", "goland64", "sublime_text", "notepad++", "windowsterminal", "wt", "powershell", "pwsh", "cmd", "alacritty", "wezterm-gui", "zed"];
        if CODE.contains(&exe) {
            return Tone::Code;
        }
        if CASUAL.contains(&exe) {
            return Tone::Casual;
        }
        if FORMAL.contains(&exe) {
            return Tone::Formal;
        }
        // Browsers: guess from the tab title.
        let in_title = |list: &[&str]| list.iter().any(|w| title.contains(w));
        if in_title(&["gmail", "outlook", "mail", "proton", "google docs", "linkedin"]) {
            return Tone::Formal;
        }
        if in_title(&["slack", "discord", "whatsapp", "telegram", "messenger", "x.com", "instagram", "chat"]) {
            return Tone::Casual;
        }
        if in_title(&["github", "gitlab", "codepen", "stack overflow"]) {
            return Tone::Neutral;
        }
        Tone::Neutral
    }
}

#[cfg(windows)]
pub fn foreground() -> AppCtx {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return AppCtx::default();
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let mut exe = String::new();
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !h.is_null() {
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(h, 0, path.as_mut_ptr(), &mut len) != 0 {
                let full = String::from_utf16_lossy(&path[..len as usize]);
                exe = full.rsplit(['\\', '/']).next().unwrap_or("").to_string();
            }
            CloseHandle(h);
        }
        AppCtx { exe, title }
    }
}

#[cfg(not(windows))]
pub fn foreground() -> AppCtx {
    AppCtx::default()
}
