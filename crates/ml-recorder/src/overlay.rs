//! Event popup "2a Signal": a small always-on-top, click-through, non-activating card in the top-right
//! corner with the animated logo, an accent strip, and a lifetime bar. Spec: 2a-signal-spec.md.
//!
//! Everything runs on the tray thread's message loop: other threads queue a `Popup` on a channel and
//! call `wake()`. While a popup is visible a 16 ms timer redraws it (~60 fps) into a memory bitmap that is
//! blitted to the window; the window slides/fades through its layered alpha and `SetWindowPos`.
//! Like any overlay it cannot appear above a game in *exclusive* fullscreen (borderless is fine).

use std::{
    f64::consts::PI,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Instant,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetStockObject, GetTextExtentPoint32W, HFONT, ReleaseDC, RoundRect, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, HBITMAP, HDC, NULL_PEN,
    PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, KillTimer, PostThreadMessageW, RegisterClassW,
    SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, LWA_ALPHA,
    SM_CXSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WM_APP, WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// What happened. It decides the accent colour, the logo's behaviour and how long the card stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A game came to the foreground and everything is recording.
    Game,
    /// A game came to the foreground but a sound source is down (the last bar blinks).
    GameWarn,
    /// The save shortcut was pressed; the clip is saved after `roll_ms` (the card counts down).
    PostRoll { roll_ms: u64 },
    Saved,
    SaveFailed,
    RecordingDown,
    /// A newer Buffer is out (the recorder checks the release feed; the app installs it).
    Update,
}

impl Event {
    pub fn is_error(self) -> bool {
        matches!(self, Event::SaveFailed | Event::RecordingDown)
    }

    fn accent(self) -> (u8, u8, u8) {
        match self {
            Event::GameWarn => (0xff, 0xb4, 0x54),
            Event::SaveFailed | Event::RecordingDown => (0xff, 0x5d, 0x5d),
            _ => (0xc6, 0xf4, 0x32),
        }
    }

    fn total_ms(self) -> f64 {
        match self {
            // the live countdown (3 2 1 0), then a moment for the save to finish and the card to turn into "saved"
            Event::PostRoll { roll_ms } => roll_ms as f64 + 2500.0,
            Event::SaveFailed | Event::RecordingDown => 6500.0,
            Event::Update => 8000.0,
            _ => 4000.0,
        }
    }
}

#[derive(Clone)]
pub struct Popup {
    pub event: Event,
    pub title: String,
    pub sub: String,
}

/* ───────── animation helpers (spec) ───────── */

fn cl(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}
fn eo(x: f64) -> f64 {
    1.0 - (1.0 - cl(x)).powi(3)
}
fn eb(x: f64) -> f64 {
    let x = cl(x);
    1.0 + 2.70158 * (x - 1.0).powi(3) + 1.70158 * (x - 1.0).powi(2)
}
fn decay(e: f64, len: f64) -> f64 {
    if (0.0..len).contains(&e) {
        1.0 - e / len
    } else {
        0.0
    }
}

// Card colours follow the app theme (the app writes `theme` into recorder.json): Ink raised surface, OLED panel.
static OLED: AtomicBool = AtomicBool::new(false);

/// Called after every config load/reload with the app's theme name.
pub fn set_theme(name: &str) {
    OLED.store(name == "oled", Ordering::Relaxed);
    THEME.store(match name { "oled" => 1, "slate" => 2, _ => 0 }, Ordering::Relaxed);
}

/// The app theme for the mixer flyout, which uses the app's own surfaces: 0 ink, 1 oled, 2 slate.
static THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn theme() -> u8 {
    THEME.load(Ordering::Relaxed)
}

fn bg() -> (u8, u8, u8) {
    if OLED.load(Ordering::Relaxed) { (0x0a, 0x0b, 0x08) } else { (0x1a, 0x1d, 0x16) }
}

fn track() -> (u8, u8, u8) {
    if OLED.load(Ordering::Relaxed) { (0x20, 0x23, 0x1a) } else { (0x26, 0x2a, 0x20) }
}

const TEXT: (u8, u8, u8) = (0xee, 0xf0, 0xe6);
const MUTED: (u8, u8, u8) = (0x8e, 0x94, 0x84);

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> u32 {
    let t = cl(t);
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u32;
    f(a.0, b.0) | (f(a.1, b.1) << 8) | (f(a.2, b.2) << 16)
}

const fn rgb(c: (u8, u8, u8)) -> u32 {
    (c.0 as u32) | ((c.1 as u32) << 8) | ((c.2 as u32) << 16)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn make_font(size: f64, weight: i32, s: f64) -> HFONT {
    CreateFontW(-((size * s).round() as i32), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr())
}

/// Card width (logical px) that fits both lines without an ellipsis, within a sane range.
fn card_width(popup: &Popup, s: f64) -> f64 {
    const TEXT_LEFT: f64 = 84.0;
    const RIGHT_PAD: f64 = 22.0;
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let extent = |text: &str, size: f64, weight: i32| {
            let f = make_font(size, weight, s);
            let old = SelectObject(dc, f);
            let w = wide(text);
            let mut sz = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, w.as_ptr(), (w.len() - 1) as i32, &mut sz);
            SelectObject(dc, old);
            DeleteObject(f);
            sz.cx as f64 / s
        };
        let need = extent(&popup.title, 19.0 * 1.06, 700).max(extent(&popup.sub, 14.0, 400));
        ReleaseDC(std::ptr::null_mut(), dc);
        (TEXT_LEFT + need + RIGHT_PAD).clamp(400.0, 820.0)
    }
}

/* ───────── window plumbing ───────── */

static TRAY_THREAD: AtomicU32 = AtomicU32::new(0);

/// Call once on the thread that runs the message loop.
pub fn bind_thread() {
    TRAY_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
}

/// Wakes the message loop so a queued popup shows immediately instead of at the next tick.
pub fn wake() {
    let tid = TRAY_THREAD.load(Ordering::Relaxed);
    if tid != 0 {
        unsafe {
            PostThreadMessageW(tid, WM_APP, 0, 0);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            // The frame timer repaints within 16 ms; just validate the update region.
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

struct Active {
    popup: Popup,
    t0: Instant,
    total_ms: f64,
    /// Where the lifetime bar starts counting down (0, or the moment of the post-roll → saved morph).
    bar_start_ms: f64,
    /// Set when a post-roll card turned into the "saved" card: ms since `t0`.
    morph_at: Option<f64>,
}

/// Owns the popup window. Create and use it on the message-loop thread only.
pub struct Overlay {
    hwnd: HWND,
    mem: HDC,
    bmp: HBITMAP,
    size: (i32, i32),
    dpi: f64,
    timer: usize,
    active: Option<Active>,
}

impl Overlay {
    pub fn new() -> Overlay {
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let class = wide("BufferOverlay");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
        }
        Overlay { hwnd: std::ptr::null_mut(), mem: std::ptr::null_mut(), bmp: std::ptr::null_mut(), size: (0, 0), dpi: 1.0, timer: 0, active: None }
    }

    pub fn is_anim_timer(&self, id: usize) -> bool {
        self.timer != 0 && self.timer == id
    }

    /// Queues a popup. A saved-card arriving while the post-roll card is up morphs it instead of replacing it.
    pub fn push(&mut self, popup: Popup) {
        if let Some(a) = &mut self.active {
            let e = a.t0.elapsed().as_secs_f64() * 1000.0;
            let counting = matches!(a.popup.event, Event::PostRoll { .. }) && a.morph_at.is_none();
            if counting {
                match popup.event {
                    Event::Saved => {
                        a.popup = popup;
                        a.morph_at = Some(e);
                        a.bar_start_ms = e;
                        a.total_ms = e + 4000.0;
                        return;
                    }
                    // Failures replace the countdown; anything else waits its turn out of the way.
                    Event::SaveFailed | Event::RecordingDown => {}
                    _ => return,
                }
            }
        }
        self.start(popup);
    }

    fn start(&mut self, popup: Popup) {
        unsafe {
            self.dpi = (windows_sys::Win32::UI::HiDpi::GetDpiForSystem().max(96) as f64) / 96.0;
            let (w, h) = ((card_width(&popup, self.dpi) * self.dpi) as i32, (78.0 * self.dpi) as i32);
            if self.hwnd.is_null() || self.size != (w, h) {
                self.destroy_window();
                let hinst = GetModuleHandleW(std::ptr::null());
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                    wide("BufferOverlay").as_ptr(),
                    wide("Buffer").as_ptr(),
                    WS_POPUP,
                    -10_000, 0, w, h,
                    std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null(),
                );
                if hwnd.is_null() {
                    return;
                }
                SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
                SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, (16.0 * self.dpi) as i32, (16.0 * self.dpi) as i32), 1);
                let screen = GetDC(std::ptr::null_mut());
                self.mem = CreateCompatibleDC(screen);
                self.bmp = CreateCompatibleBitmap(screen, w, h);
                ReleaseDC(std::ptr::null_mut(), screen);
                SelectObject(self.mem, self.bmp);
                self.hwnd = hwnd;
                self.size = (w, h);
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        let total = popup.event.total_ms();
        self.active = Some(Active { popup, t0: Instant::now(), total_ms: total, bar_start_ms: 0.0, morph_at: None });
        if self.timer == 0 {
            self.timer = unsafe { SetTimer(std::ptr::null_mut(), 0, 16, None) };
        }
        self.frame();
    }

    fn destroy_window(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                DestroyWindow(self.hwnd);
                self.hwnd = std::ptr::null_mut();
            }
            if !self.mem.is_null() {
                DeleteDC(self.mem);
                self.mem = std::ptr::null_mut();
            }
            if !self.bmp.is_null() {
                DeleteObject(self.bmp);
                self.bmp = std::ptr::null_mut();
            }
        }
    }

    fn close(&mut self) {
        self.active = None;
        unsafe {
            if self.timer != 0 {
                KillTimer(std::ptr::null_mut(), self.timer);
                self.timer = 0;
            }
        }
        self.destroy_window();
    }

    /// One animation frame: draw, position and fade the window; closes it when the clock runs out.
    pub fn frame(&mut self) {
        let Some(a) = &self.active else { return };
        let e = a.t0.elapsed().as_secs_f64() * 1000.0;
        if e >= a.total_ms || self.hwnd.is_null() {
            self.close();
            return;
        }
        let (w, h) = self.size;
        let s = self.dpi;
        self.draw(a, e, w, h, s);

        let enter = eb(e / 420.0);
        let exit = eo((e - (a.total_ms - 320.0)) / 320.0);
        let mut off = (1.0 - enter) * 440.0 + exit * 40.0;
        if a.popup.event.is_error() {
            off += (e / 22.0).sin() * 7.0 * decay(e - 380.0, 520.0);
        }
        let alpha = (cl(e / 200.0) * (1.0 - exit) * 0.96 * 255.0).round() as u8;
        unsafe {
            let x = GetSystemMetrics(SM_CXSCREEN) - w - (24.0 * s) as i32 + (off * s) as i32;
            SetLayeredWindowAttributes(self.hwnd, 0, alpha, LWA_ALPHA);
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, (24.0 * s) as i32, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
            let dc = GetDC(self.hwnd);
            BitBlt(dc, 0, 0, w, h, self.mem, 0, 0, SRCCOPY);
            ReleaseDC(self.hwnd, dc);
        }
    }

    fn draw(&self, a: &Active, e: f64, w: i32, h: i32, s: f64) {
        let ev = a.popup.event;
        let accent = ev.accent();
        let local = e - a.morph_at.unwrap_or(0.0);
        let glow = decay(local, 700.0);
        let counting = matches!(ev, Event::PostRoll { .. }) && a.morph_at.is_none();
        let px = |v: f64| (v * s).round() as i32;
        unsafe {
            let dc = self.mem;
            // card background, tinted by the glow
            let card = CreateSolidBrush(mix(bg(), accent, 0.16 * glow));
            FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, card);
            DeleteObject(card);
            // accent strip
            let strip_brush = CreateSolidBrush(rgb(accent));
            FillRect(dc, &RECT { left: 0, top: 0, right: px(6.0 + 4.0 * glow), bottom: h }, strip_brush);
            DeleteObject(strip_brush);

            // logo: 11 scan-line bars
            const N: usize = 11;
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            for i in 0..N {
                let dist = (i as i32 - 5).abs() as f64;
                let base = ((8.0 + (5.0 - dist) * 5.4) * 1.1).floor();
                let mut wd = base * eo((e - 120.0 - i as f64 * 45.0) / 260.0);
                let k = eo((e - 500.0) / 900.0);
                let wave = 0.5 + 0.5 * (e / 700.0 - i as f64 * 0.55).sin();
                wd += 9.0 * k * wave * (0.4 + 0.6 * (1.0 - dist / 5.0));
                let mut alpha = 1.0;
                match ev {
                    Event::GameWarn if i == N - 1 => alpha = if ((e / 380.0).floor() as i64) % 2 == 0 { 1.0 } else { 0.25 },
                    Event::PostRoll { roll_ms } if counting => {
                        let lit = cl(1.0 - e / roll_ms.max(1) as f64) * N as f64;
                        let top = lit.ceil() as usize;
                        alpha = if i < top { 1.0 } else { 0.18 };
                        if top > 0 && i == top - 1 {
                            let f = lit - lit.floor();
                            alpha = if f == 0.0 { 1.0 } else { 0.45 + 0.55 * f };
                        }
                    }
                    _ => {}
                }
                if let Some(m) = a.morph_at {
                    let sm = cl((e - m - i as f64 * 40.0) / 200.0);
                    alpha = 0.18 + 0.82 * sm;
                    wd *= 1.0 + 0.5 * (PI * sm).sin();
                }
                let y = 13.0 + i as f64 * 5.0;
                let tail = CreateSolidBrush(mix(bg(), accent, 0.30 * alpha));
                let old = SelectObject(dc, tail);
                RoundRect(dc, px(14.0), px(y), px(22.0), px(y + 3.0), px(3.0), px(3.0));
                SelectObject(dc, old);
                DeleteObject(tail);
                let bar = CreateSolidBrush(mix(bg(), accent, alpha));
                let old = SelectObject(dc, bar);
                RoundRect(dc, px(22.0), px(y), px(22.0 + wd.max(0.0)) + 1, px(y + 3.0), px(3.0), px(3.0));
                SelectObject(dc, old);
                DeleteObject(bar);
            }
            SelectObject(dc, old_pen);

            // lifetime bar, 2 px at the bottom
            let pct = match ev {
                Event::PostRoll { roll_ms } if counting => cl(1.0 - e / roll_ms.max(1) as f64),
                _ => cl(1.0 - (e - a.bar_start_ms) / (a.total_ms - a.bar_start_ms).max(1.0)),
            };
            let (x0, y0) = (px(6.0), h - px(2.0));
            let track = CreateSolidBrush(rgb(track()));
            FillRect(dc, &RECT { left: x0, top: y0, right: w, bottom: h }, track);
            DeleteObject(track);
            let fill = CreateSolidBrush(rgb(accent));
            FillRect(dc, &RECT { left: x0, top: y0, right: x0 + ((w - x0) as f64 * pct) as i32, bottom: h }, fill);
            DeleteObject(fill);

            // text
            SetBkMode(dc, TRANSPARENT as i32);
            let title_scale = if counting { 1.0 + 0.06 * decay(e % 1000.0, 220.0) } else { 1.0 };
            let title = match ev {
                Event::PostRoll { roll_ms } if counting => {
                    let n = ((roll_ms as f64 / 1000.0) - e / 1000.0).ceil().max(0.0) as u64;
                    crate::lang::f("pop.postroll", &[("n", &n)])
                }
                _ => a.popup.title.clone(),
            };
            let mk = |size: f64, weight: i32| make_font(size, weight, s);
            let f1 = mk(19.0 * title_scale, 700);
            let f2 = mk(14.0, 400);
            let pad = px(84.0);
            let mut r1 = RECT { left: pad, top: px(8.0), right: w - px(18.0), bottom: px(42.0) };
            let old = SelectObject(dc, f1);
            SetTextColor(dc, rgb(TEXT));
            DrawTextW(dc, wide(&title).as_ptr(), -1, &mut r1, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
            let mut r2 = RECT { left: pad, top: px(42.0), right: w - px(18.0), bottom: px(68.0) };
            SelectObject(dc, f2);
            SetTextColor(dc, rgb(MUTED));
            DrawTextW(dc, wide(&a.popup.sub).as_ptr(), -1, &mut r2, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
            SelectObject(dc, old);
            DeleteObject(f1);
            DeleteObject(f2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_helpers_match_the_spec() {
        assert_eq!(eo(0.0), 0.0);
        assert_eq!(eo(1.0), 1.0);
        assert!(eb(1.0) == 1.0);
        assert!((0..100).any(|i| eb(i as f64 / 100.0) > 1.0), "ease-out-back overshoots");
        assert_eq!(decay(100.0, 200.0), 0.5);
        assert_eq!(decay(250.0, 200.0), 0.0);
    }

    #[test]
    fn lifetimes_follow_the_table() {
        assert_eq!(Event::Game.total_ms(), 4000.0);
        assert_eq!(Event::SaveFailed.total_ms(), 6500.0);
        assert_eq!(Event::PostRoll { roll_ms: 3_000 }.total_ms(), 5_500.0);
    }
}
