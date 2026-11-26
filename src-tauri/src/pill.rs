//! The on-screen indicator, drawn natively (a layered Win32 window, no WebView).
//! Costs about 1 MB instead of the ~260 MB a WebView2 window needed, and redraws only while it animates.
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_PILL: u32 = WM_APP + 7;
const TIMER: usize = 1;
// Window size in logical px. The pill is drawn centred at the bottom; the rest is transparent.
const WIN_W: f32 = 340.0;
const WIN_H: f32 = 60.0;

// Theme: black, white, red (matches SpotVault).
const BG: [f32; 4] = [0.067, 0.067, 0.067, 0.94];
const BORDER: [f32; 4] = [1.0, 1.0, 1.0, 0.10];
const FG: [f32; 4] = [0.95, 0.94, 0.91, 1.0];
const RED: [f32; 4] = [0.94, 0.38, 0.35, 1.0];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Idle,
    Loading,
    Listening,
    Locked,
    Processing,
    Done,
    Error,
}

struct State {
    kind: Kind,
    msg: String,
    idle_visible: bool,
    since: Instant,
    level: f32,
    bars: [f32; 7],
    // Animated pill size (logical px) and opacity.
    w: f32,
    h: f32,
    alpha: f32,
    shown: bool,
    text: Option<(String, u32, TextMask)>,
}

struct TextMask {
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

static HWND_: AtomicIsize = AtomicIsize::new(0);
static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        Mutex::new(State {
            kind: Kind::Idle,
            msg: String::new(),
            idle_visible: true,
            since: Instant::now(),
            level: 0.0,
            bars: [4.0; 7],
            w: 44.0,
            h: 8.0,
            alpha: 0.55,
            shown: false,
            text: None,
        })
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn poke() {
    let h = HWND_.load(Ordering::Relaxed);
    if h != 0 {
        unsafe { PostMessageW(h as HWND, WM_PILL, 0, 0) };
    }
}

/// Start the indicator thread. Call once.
pub fn start(idle_visible: bool) {
    state().lock().unwrap().idle_visible = idle_visible;
    std::thread::Builder::new()
        .name("pill".into())
        .spawn(|| unsafe { run() })
        .ok();
}

/// Change what the indicator shows. `state` is one of idle, loading, listening, locked, processing, done, error.
pub fn set(kind: &str, msg: &str, idle_visible: bool) {
    let k = match kind {
        "loading" => Kind::Loading,
        "listening" => Kind::Listening,
        "locked" => Kind::Locked,
        "processing" => Kind::Processing,
        "done" => Kind::Done,
        "error" => Kind::Error,
        _ => Kind::Idle,
    };
    {
        let mut s = state().lock().unwrap();
        s.kind = k;
        s.msg = msg.to_string();
        s.idle_visible = idle_visible;
        s.since = Instant::now();
        if !matches!(k, Kind::Listening | Kind::Locked) {
            s.level = 0.0;
        }
    }
    poke();
}

/// Microphone level (RMS, 0..1) while listening.
pub fn level(rms: f32) {
    state().lock().unwrap().level = rms;
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PILL => {
            SetTimer(hwnd, TIMER, 33, None);
            tick(hwnd);
            0
        }
        WM_TIMER => {
            if !tick(hwnd) {
                KillTimer(hwnd, TIMER);
            }
            0
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            tick(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn run() {
    let inst = GetModuleHandleW(std::ptr::null());
    let class = wide("UtterPill");
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: inst,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    RegisterClassW(&wc);
    let title = wide("Utter");
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        inst,
        std::ptr::null(),
    );
    if hwnd.is_null() {
        crate::log!("indicator window failed");
        return;
    }
    HWND_.store(hwnd as isize, Ordering::Relaxed);
    PostMessageW(hwnd, WM_PILL, 0, 0);
    let mut m: MSG = std::mem::zeroed();
    while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
}

fn approach(cur: &mut f32, target: f32, k: f32) -> bool {
    let d = target - *cur;
    if d.abs() < 0.3 {
        *cur = target;
        false
    } else {
        *cur += d * k;
        true
    }
}

/// Advance the animation one frame and redraw. Returns true while more frames are needed.
unsafe fn tick(hwnd: HWND) -> bool {
    let dpi = GetDpiForWindow(hwnd).max(96);
    let sc = dpi as f32 / 96.0;
    let mut s = state().lock().unwrap();

    // Timed states fall back to idle.
    let age = s.since.elapsed().as_millis();
    if (s.kind == Kind::Done && age > 700) || (s.kind == Kind::Error && age > 2200) {
        s.kind = Kind::Idle;
        s.msg.clear();
        s.since = Instant::now();
    }
    let kind = s.kind;
    let text = match kind {
        Kind::Locked | Kind::Error | Kind::Loading => s.msg.clone(),
        _ => String::new(),
    };
    // Rebuild the text mask only when the text or DPI changes.
    if !text.is_empty() {
        let stale = s.text.as_ref().map(|(t, d, _)| t != &text || *d != dpi).unwrap_or(true);
        if stale {
            s.text = Some((text.clone(), dpi, text_mask(&text, sc)));
        }
    } else {
        s.text = None;
    }
    let text_w = s.text.as_ref().map(|(_, _, m)| m.w as f32 / sc).unwrap_or(0.0);

    // Target size for this state (logical px).
    const BARS_W: f32 = 7.0 * 3.0 + 6.0 * 3.0;
    let content = match kind {
        Kind::Idle => 0.0,
        Kind::Listening => 6.0 + 8.0 + BARS_W,
        Kind::Locked => 6.0 + 8.0 + BARS_W + 8.0 + text_w,
        Kind::Processing => 14.0,
        Kind::Loading => 14.0 + if text_w > 0.0 { 8.0 + text_w } else { 0.0 },
        Kind::Done => 14.0,
        Kind::Error => text_w,
    };
    let (tw, th, ta) = match kind {
        Kind::Idle => (44.0, 8.0, if s.idle_visible { 0.55 } else { 0.0 }),
        Kind::Done => (44.0f32.max(content + 28.0), 34.0, 1.0),
        _ => (96.0f32.max(content + 28.0).min(WIN_W - 8.0), 34.0, 1.0),
    };
    let mut busy = false;
    busy |= approach(&mut s.w, tw, 0.35);
    busy |= approach(&mut s.h, th, 0.35);
    let da = ta - s.alpha;
    if da.abs() > 0.02 {
        s.alpha += da * 0.35;
        busy = true;
    } else {
        s.alpha = ta;
    }

    // Level -> bar heights (centre bars react most), smoothed.
    const SHAPE: [f32; 7] = [0.45, 0.7, 0.9, 1.0, 0.9, 0.7, 0.45];
    let live = matches!(kind, Kind::Listening | Kind::Locked);
    let v = (s.level.sqrt() * 3.2).min(1.0);
    let t = s.since.elapsed().as_secs_f32();
    for i in 0..7 {
        let jitter = 0.8 + 0.4 * ((t * 9.0 + i as f32 * 1.7).sin() * 0.5 + 0.5);
        let target = if live { (18.0 * v * SHAPE[i] * jitter).max(4.0) } else { 4.0 };
        let cur = s.bars[i];
        s.bars[i] = cur + (target - cur) * 0.5;
    }
    let animating = busy || !matches!(kind, Kind::Idle);

    // Hide the window entirely when there is nothing to show.
    if s.alpha <= 0.0 && kind == Kind::Idle {
        if s.shown {
            ShowWindow(hwnd, SW_HIDE);
            s.shown = false;
        }
        return false;
    }

    let (ww, wh) = ((WIN_W * sc).round() as i32, (WIN_H * sc).round() as i32);
    let mut buf = vec![[0f32; 4]; (ww * wh) as usize];
    draw(&mut buf, ww, wh, sc, &s, kind, t);
    present(hwnd, &buf, ww, wh);
    if !s.shown {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        s.shown = true;
    }
    if age < 40 {
        // Stay above other topmost windows after state changes.
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    animating
}

// ---------- drawing (premultiplied RGBA in f32, then BGRA for the window) ----------

fn over(px: &mut [f32; 4], c: [f32; 4], cov: f32) {
    let a = c[3] * cov;
    if a <= 0.0 {
        return;
    }
    for k in 0..3 {
        px[k] = c[k] * a + px[k] * (1.0 - a);
    }
    px[3] = a + px[3] * (1.0 - a);
}

/// Fill a shape given by a signed-distance function, anti-aliased, within a bounding box.
fn fill(buf: &mut [[f32; 4]], ww: i32, wh: i32, bbox: (f32, f32, f32, f32), c: [f32; 4], sdf: impl Fn(f32, f32) -> f32) {
    let (x0, y0, x1, y1) = bbox;
    let (x0, y0) = ((x0.floor() as i32 - 1).max(0), (y0.floor() as i32 - 1).max(0));
    let (x1, y1) = ((x1.ceil() as i32 + 1).min(ww), (y1.ceil() as i32 + 1).min(wh));
    for y in y0..y1 {
        for x in x0..x1 {
            let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                over(&mut buf[(y * ww + x) as usize], c, cov);
            }
        }
    }
}

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (pax, pay, bax, bay) = (px - ax, py - ay, bx - ax, by - ay);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    let (dx, dy) = (pax - bax * h, pay - bay * h);
    (dx * dx + dy * dy).sqrt()
}

fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

fn draw(buf: &mut [[f32; 4]], ww: i32, wh: i32, sc: f32, s: &State, kind: Kind, t: f32) {
    let a = s.alpha;
    let (pw, ph) = (s.w * sc, s.h * sc);
    let cx = ww as f32 / 2.0;
    let cy = wh as f32 - 6.0 * sc - ph / 2.0;
    let bb = (cx - pw / 2.0, cy - ph / 2.0, cx + pw / 2.0, cy + ph / 2.0);
    let r = ph / 2.0;
    // Border, then body.
    fill(buf, ww, wh, bb, with_alpha(BORDER, a), |x, y| sd_round_rect(x, y, cx, cy, pw / 2.0, ph / 2.0, r));
    fill(buf, ww, wh, bb, with_alpha(BG, a), |x, y| sd_round_rect(x, y, cx, cy, pw / 2.0 - sc, ph / 2.0 - sc, r - sc));
    if kind == Kind::Idle || s.h < 20.0 {
        return;
    }
    // Lay content out left to right, centred.
    let text_w = s.text.as_ref().map(|(_, _, m)| m.w as f32).unwrap_or(0.0);
    let bars_w = (7.0 * 3.0 + 6.0 * 3.0) * sc;
    let gap = 8.0 * sc;
    let parts: Vec<(&str, f32)> = match kind {
        Kind::Listening => vec![("dot", 6.0 * sc), ("bars", bars_w)],
        Kind::Locked => vec![("dot", 6.0 * sc), ("bars", bars_w), ("text", text_w)],
        Kind::Processing => vec![("spin", 14.0 * sc)],
        Kind::Loading if text_w > 0.0 => vec![("spin", 14.0 * sc), ("text", text_w)],
        Kind::Loading => vec![("spin", 14.0 * sc)],
        Kind::Done => vec![("check", 14.0 * sc)],
        Kind::Error => vec![("text", text_w)],
        Kind::Idle => vec![],
    };
    let total: f32 = parts.iter().map(|p| p.1).sum::<f32>() + gap * (parts.len().saturating_sub(1)) as f32;
    let mut x = cx - total / 2.0;
    for (what, w) in parts {
        match what {
            "dot" => {
                let blink = if kind == Kind::Locked { 0.25 + 0.75 * (((t * 5.2).cos() * 0.5) + 0.5) } else { 1.0 };
                let (dx, rr) = (x + w / 2.0, 3.0 * sc);
                fill(buf, ww, wh, (dx - rr, cy - rr, dx + rr, cy + rr), with_alpha(RED, a * blink), |px, py| {
                    ((px - dx).powi(2) + (py - cy).powi(2)).sqrt() - rr
                });
            }
            "bars" => {
                for i in 0..7 {
                    let bx = x + (i as f32 * 6.0 + 1.5) * sc;
                    let hh = s.bars[i] * sc / 2.0;
                    let hw = 1.5 * sc;
                    fill(buf, ww, wh, (bx - hw, cy - hh, bx + hw, cy + hh), with_alpha(FG, a), |px, py| {
                        sd_round_rect(px, py, bx, cy, hw, hh, hw)
                    });
                }
            }
            "spin" => {
                let (sx, rr, th) = (x + w / 2.0, 6.0 * sc, 1.0 * sc);
                let bbox = (sx - rr - th, cy - rr - th, sx + rr + th, cy + rr + th);
                fill(buf, ww, wh, bbox, with_alpha(FG, a * 0.2), |px, py| {
                    (((px - sx).powi(2) + (py - cy).powi(2)).sqrt() - rr).abs() - th
                });
                // A 100-degree arc that turns about 1.4 times a second, with round ends.
                let start = t * 9.0;
                let span = 1.75f32;
                fill(buf, ww, wh, bbox, with_alpha(FG, a), |px, py| {
                    let (dx, dy) = (px - sx, py - cy);
                    let ang = dy.atan2(dx);
                    let rel = (ang - start).rem_euclid(std::f32::consts::TAU);
                    if rel <= span {
                        ((dx * dx + dy * dy).sqrt() - rr).abs() - th
                    } else {
                        let e = |q: f32| ((dx - rr * q.cos()).powi(2) + (dy - rr * q.sin()).powi(2)).sqrt() - th;
                        e(start).min(e(start + span))
                    }
                });
            }
            "check" => {
                let (kx, th) = (x + w / 2.0, 1.1 * sc);
                let p = |ox: f32, oy: f32| (kx + ox * sc, cy + oy * sc);
                let (a1, b1, c1) = (p(-5.0, 0.5), p(-1.5, 4.0), p(5.5, -4.0));
                fill(buf, ww, wh, (kx - 8.0 * sc, cy - 6.0 * sc, kx + 8.0 * sc, cy + 6.0 * sc), with_alpha(FG, a), |px, py| {
                    sd_segment(px, py, a1.0, a1.1, b1.0, b1.1).min(sd_segment(px, py, b1.0, b1.1, c1.0, c1.1)) - th
                });
            }
            "text" => {
                if let Some((_, _, m)) = &s.text {
                    let col = if kind == Kind::Error { RED } else { FG };
                    let ox = x.round() as i32;
                    let oy = (cy - m.h as f32 / 2.0).round() as i32;
                    for ty in 0..m.h as i32 {
                        for tx in 0..m.w as i32 {
                            let cov = m.cov[(ty as usize) * m.w + tx as usize] as f32 / 255.0;
                            let (gx, gy) = (ox + tx, oy + ty);
                            if cov > 0.0 && gx >= 0 && gy >= 0 && gx < ww && gy < wh {
                                over(&mut buf[(gy * ww + gx) as usize], with_alpha(col, a), cov);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        x += w + gap;
    }
}

/// Render text with GDI (grayscale anti-aliasing) into a coverage mask.
fn text_mask(text: &str, sc: f32) -> TextMask {
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let face = wide("Segoe UI");
        let font = CreateFontW(
            -(12.0 * sc).round() as i32,
            0,
            0,
            0,
            500,
            0,
            0,
            0,
            DEFAULT_CHARSET as _,
            OUT_DEFAULT_PRECIS as _,
            CLIP_DEFAULT_PRECIS as _,
            ANTIALIASED_QUALITY as _,
            (DEFAULT_PITCH as u32 | FF_SWISS as u32) as _,
            face.as_ptr(),
        );
        let old_font = SelectObject(dc, font as _);
        let w16: Vec<u16> = text.encode_utf16().collect();
        let mut sz: SIZE = std::mem::zeroed();
        GetTextExtentPoint32W(dc, w16.as_ptr(), w16.len() as i32, &mut sz);
        let (w, h) = (sz.cx.max(1) as usize + 2, sz.cy.max(1) as usize);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as _;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let old_bmp = SelectObject(dc, bmp as _);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, 0x00FF_FFFF);
        TextOutW(dc, 1, 0, w16.as_ptr(), w16.len() as i32);
        GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u32, w * h);
        let cov = px.iter().map(|p| ((p >> 8) & 0xFF) as u8).collect();
        SelectObject(dc, old_bmp);
        SelectObject(dc, old_font);
        DeleteObject(bmp as _);
        DeleteObject(font as _);
        DeleteDC(dc);
        TextMask { w, h, cov }
    }
}

/// Push the frame to the layered window, bottom-centre of the primary screen's work area.
unsafe fn present(hwnd: HWND, buf: &[[f32; 4]], ww: i32, wh: i32) {
    let mut wa: RECT = std::mem::zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut RECT as _, 0);
    let pos = POINT { x: wa.left + ((wa.right - wa.left) - ww) / 2, y: wa.bottom - wh };

    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = ww;
    bmi.bmiHeader.biHeight = -wh;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB as _;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if !bmp.is_null() {
        let out = std::slice::from_raw_parts_mut(bits as *mut u32, (ww * wh) as usize);
        for (o, p) in out.iter_mut().zip(buf) {
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            *o = (c(p[3]) << 24) | (c(p[0]) << 16) | (c(p[1]) << 8) | c(p[2]);
        }
        let old = SelectObject(dc, bmp as _);
        let size = SIZE { cx: ww, cy: wh };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, screen, &pos, &size, dc, &src, 0, &blend, ULW_ALPHA);
        SelectObject(dc, old);
        DeleteObject(bmp as _);
    }
    DeleteDC(dc);
    ReleaseDC(std::ptr::null_mut(), screen);
}
