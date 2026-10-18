//! Small Win32 helpers: single instance, job object, hotkey, foreground app, local time.

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, RECT, SYSTEMTIME};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, MB_OK, MSG, WM_HOTKEY,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Free bytes on the drive that holds `dir` (None if it cannot be asked).
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut probe = dir;
    while !probe.exists() {
        probe = probe.parent()?;
    }
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide(&probe.to_string_lossy()).as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

/// Raises this process to "above normal" priority. A game or a compile at full CPU otherwise starved the capture
/// and audio threads: the frame rate fell to 5-20 fps with whole stalls, and every source got a hole at once.
pub fn raise_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS};
    unsafe {
        SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
    }
}

/// Returns false when another instance already holds the named mutex.
/// The handle is intentionally leaked: it must live as long as the process.
pub fn claim_single_instance(name: &str) -> bool {
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, wide(name).as_ptr());
        if h.is_null() {
            return true; // could not create; do not block startup
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        true
    }
}

/// A job object that kills every assigned process when the recorder exits or crashes,
/// so a stray ffmpeg can never keep recording on its own.
pub struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> Option<Job> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            Some(Job(h))
        }
    }

    /// The raw handle value (for the CPU accounting queries in perf.rs).
    pub fn raw(&self) -> isize {
        self.0 as isize
    }

    pub fn assign(&self, child: &std::process::Child) {
        unsafe {
            AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE);
        }
    }
}

/// Parses "Ctrl+Alt+F9" into (modifiers, virtual-key).
fn parse_hotkey(spec: &str) -> Option<(u32, u32)> {
    let mut mods = MOD_NOREPEAT;
    let mut vk = None;
    for part in spec.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            k if k.len() == 1 => {
                let c = k.chars().next()?.to_ascii_uppercase();
                if c.is_ascii_alphanumeric() {
                    vk = Some(c as u32);
                }
            }
            k if k.starts_with('f') && k[1..].parse::<u32>().is_ok() => {
                let n: u32 = k[1..].parse().ok()?;
                if (1..=24).contains(&n) {
                    vk = Some(0x70 + n - 1); // VK_F1 = 0x70
                }
            }
            k if k.starts_with("num") && k[3..].parse::<u32>().is_ok_and(|n| n <= 9) => {
                vk = Some(0x60 + k[3..].parse::<u32>().ok()?); // VK_NUMPAD0 = 0x60
            }
            // the names the settings' shortcut field writes (EarTrumpet-style combos such as Ctrl+PgDn)
            k => {
                vk = Some(match k {
                    "pgup" | "pageup" => 0x21,
                    "pgdn" | "pagedown" => 0x22,
                    "end" => 0x23,
                    "home" => 0x24,
                    "left" => 0x25,
                    "up" => 0x26,
                    "right" => 0x27,
                    "down" => 0x28,
                    "ins" | "insert" => 0x2d,
                    "del" | "delete" => 0x2e,
                    "space" => 0x20,
                    "pause" => 0x13,
                    "scrolllock" => 0x91,
                    "numadd" => 0x6b,
                    "numsub" => 0x6d,
                    "nummul" => 0x6a,
                    "numdiv" => 0x6f,
                    "volmute" => 0xad,
                    "voldown" => 0xae,
                    "volup" => 0xaf,
                    "medianext" => 0xb0,
                    "mediaprev" => 0xb1,
                    "mediastop" => 0xb2,
                    "mediaplay" => 0xb3,
                    _ => return None,
                })
            }
        }
    }
    vk.map(|v| (mods, v))
}

/// Registers a thread-level global hotkey; its presses arrive as WM_HOTKEY in this thread's queue.
pub fn register_hotkey(spec: &str) -> Result<(), String> {
    register_hotkey_id(spec, 1)
}

/// Releases a shortcut registered with `register_hotkey_id` (must run on the thread that registered it).
pub fn unregister_hotkey_id(id: i32) {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey(std::ptr::null_mut(), id);
    }
}

pub fn register_hotkey_id(spec: &str, id: i32) -> Result<(), String> {
    // a binding may also hold a mouse button or a gamepad chord; those are polled by input.rs
    let Some(spec) = crate::input::keyboard_part(spec) else { return Ok(()) };
    let (mods, vk) = parse_hotkey(spec).ok_or_else(|| format!("invalid hotkey '{spec}'"))?;
    unsafe {
        if RegisterHotKey(std::ptr::null_mut(), id, mods, vk) == 0 {
            return Err(format!("hotkey '{spec}' is already taken by another program"));
        }
    }
    Ok(())
}

/// What the message loop reports to its owner.
pub enum Pump {
    /// A registered global hotkey fired (the id given to `register_hotkey_id`).
    Hotkey(usize),
    /// A thread timer fired; the owner tells its own timers apart by id.
    Timer(usize),
    /// Another thread posted a wake-up (a popup is waiting).
    Wake,
}

/// Runs this thread's message loop. A 500 ms timer drives the owner's periodic work; dispatching messages
/// also services the tray icon's hidden window.
pub fn pump_messages(mut f: impl FnMut(Pump)) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, SetTimer, TranslateMessage, WM_APP, WM_TIMER};
    unsafe {
        SetTimer(std::ptr::null_mut(), 0, 500, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            match msg.message {
                WM_HOTKEY => f(Pump::Hotkey(msg.wParam as usize)),
                // a window's own timer (the flyout, tray-icon's hover tracking) goes to its window procedure
                WM_TIMER if msg.hwnd.is_null() => f(Pump::Timer(msg.wParam as usize)),
                WM_APP => f(Pump::Wake),
                _ => {}
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub struct Foreground {
    pub pid: u32,
    /// Full path of the executable.
    pub exe: String,
    /// Lower-case executable stem, e.g. "cs2".
    pub stem: String,
    /// The window covers its whole monitor (exclusive or borderless fullscreen).
    pub fullscreen: bool,
    /// Index of the monitor the window is on (same order as `monitors()` / the capture index).
    pub monitor: usize,
}

/// The program currently in the foreground, ignoring the desktop shell itself.
pub fn foreground() -> Option<Foreground> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&class.as_str()) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let stem = Path::new(&path).file_stem()?.to_string_lossy().to_lowercase();

        let mut r: RECT = std::mem::zeroed();
        let mut fullscreen = false;
        let mut monitor = 0usize;
        if GetWindowRect(hwnd, &mut r) != 0 {
            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            monitor = monitor_handles().iter().position(|h| *h == mon as isize).unwrap_or(0);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) != 0 {
                let m = mi.rcMonitor;
                fullscreen = r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom;
            }
        }
        Some(Foreground { pid, exe: path, stem, fullscreen, monitor })
    }
}

/// Full path of the executable of a running process.
pub fn exe_path_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// File stem of the executable owning the foreground window.
pub fn foreground_exe_stem() -> Option<String> {
    foreground().map(|f| f.stem)
}

/// Expands `%SystemRoot%` and friends.
pub fn expand_env(s: &str) -> String {
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
    if !s.contains('%') {
        return s.to_string();
    }
    let src = wide(s);
    let mut buf = vec![0u16; 1024];
    let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n > buf.len() {
        return s.to_string();
    }
    String::from_utf16_lossy(&buf[..n - 1])
}

/// An icon source as Windows writes it, "path" or "path,index" (a negative index is a resource id, as in
/// "@%SystemRoot%\System32\AudioSrv.Dll,-203"), split into an expanded path and the index.
pub fn icon_source(src: &str) -> (String, i32) {
    let src = src.trim().trim_start_matches('@');
    match src.rsplit_once(',') {
        Some((p, i)) if i.trim().parse::<i32>().is_ok() => (expand_env(p), i.trim().parse().unwrap_or(0)),
        _ => (expand_env(src), 0),
    }
}

/// The executable's large icon as a PNG (32 px), or None when it has none.
pub fn exe_icon_png(exe: &str) -> Option<Vec<u8>> {
    icon_png_at(exe, 0)
}

/// The icon of an icon source ("path" or "path,index") as a PNG.
pub fn icon_png(src: &str) -> Option<Vec<u8>> {
    let (path, idx) = icon_source(src);
    icon_png_at(&path, idx)
}

fn icon_png_at(file: &str, index: i32) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::Shell::ExtractIconExW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    if file.is_empty() {
        return None;
    }
    unsafe {
        let mut large = std::ptr::null_mut();
        if ExtractIconExW(wide(file).as_ptr(), index, &mut large, std::ptr::null_mut(), 1) < 1 || large.is_null() {
            return None;
        }
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(large, &mut ii) == 0 {
            DestroyIcon(large);
            return None;
        }
        let mut out = None;
        let mut bm: BITMAP = std::mem::zeroed();
        if !ii.hbmColor.is_null()
            && GetObjectW(ii.hbmColor, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut c_void) != 0
            && bm.bmWidth > 0
            && bm.bmHeight > 0
        {
            let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..std::mem::zeroed()
            };
            let mut px = vec![0u8; w * h * 4];
            let got = GetDIBits(dc, ii.hbmColor, 0, h as u32, px.as_mut_ptr() as *mut c_void, &mut bmi, DIB_RGB_COLORS);
            if got != 0 {
                // icons without an alpha channel: transparency comes from the AND mask
                if px.chunks_exact(4).all(|p| p[3] == 0) {
                    let mut ok = false;
                    if !ii.hbmMask.is_null() {
                        let mut mask = vec![0u8; w * h * 4];
                        let mut mi = bmi;
                        if GetDIBits(dc, ii.hbmMask, 0, h as u32, mask.as_mut_ptr() as *mut c_void, &mut mi, DIB_RGB_COLORS) != 0 {
                            for (p, m) in px.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                                p[3] = if m[0] == 0 { 255 } else { 0 };
                            }
                            ok = true;
                        }
                    }
                    if !ok {
                        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                    }
                }
                for p in px.chunks_exact_mut(4) {
                    p.swap(0, 2); // BGRA -> RGBA
                }
                out = Some(png_rgba(w as u32, h as u32, &px));
            }
            DeleteDC(dc);
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        DestroyIcon(large);
        out
    }
}

/// Minimal PNG writer (uncompressed deflate blocks): enough for a 32 px icon, no extra dependency.
pub(crate) fn png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks_exact(w as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(b) = blocks.next() {
        z.push(if blocks.peek().is_none() { 1 } else { 0 });
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend(b);
    }
    let (mut a, mut s) = (1u32, 0u32);
    for &b in &raw {
        a = (a + b as u32) % 65521;
        s = (s + a) % 65521;
    }
    z.extend(((s << 16) | a).to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn local_stamp() -> String {
    unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        format!(
            "{:04}-{:02}-{:02}__{:02}-{:02}-{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        )
    }
}

/// The cues Buffer can play. Each is a WAV in `assets/sounds/`, embedded in the exe (replace the file to change
/// the sound; mono 16-bit PCM, small).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// A clip was saved, or the shortcut was pressed and the countdown started.
    Saved,
    /// A save failed, or the recording / a source stopped working.
    Failed,
    /// A long recording started.
    RecStart,
    /// A long recording ended.
    RecStop,
    /// A game came to the foreground and Buffer is active.
    Game,
}

impl Sound {
    fn wav(self) -> &'static [u8] {
        match self {
            Sound::Saved => include_bytes!("../assets/sounds/saved.wav"),
            Sound::Failed => include_bytes!("../assets/sounds/failed.wav"),
            Sound::RecStart => include_bytes!("../assets/sounds/rec-start.wav"),
            Sound::RecStop => include_bytes!("../assets/sounds/rec-stop.wav"),
            Sound::Game => include_bytes!("../assets/sounds/game.wav"),
        }
    }
}

#[cfg(test)]
impl Sound {
    pub fn wav_for_test(self) -> &'static [u8] {
        self.wav()
    }
}

/// A copy of a 16-bit PCM WAV with its samples scaled by `gain_db` (None when the file is not that format).
fn scaled(wav: &[u8], gain_db: f32) -> Option<Vec<u8>> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut pcm16) = (12usize, false);
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= wav.len() {
            let format = u16::from_le_bytes([wav[body], wav[body + 1]]);
            let bits = u16::from_le_bytes([wav[body + 14], wav[body + 15]]);
            pcm16 = format == 1 && bits == 16;
        } else if id == b"data" {
            if !pcm16 {
                return None;
            }
            let end = (body + size).min(wav.len());
            let k = 10f32.powf(gain_db / 20.0);
            let mut out = wav.to_vec();
            for s in out[body..end].chunks_exact_mut(2) {
                let v = (i16::from_le_bytes([s[0], s[1]]) as f32 * k).round().clamp(-32768.0, 32767.0) as i16;
                s.copy_from_slice(&v.to_le_bytes());
            }
            return Some(out);
        }
        pos = body + size + (size & 1);
    }
    None
}

/// The buffer of the cue that is playing: PlaySound reads it asynchronously, so it has to stay alive until the next
/// cue replaces it.
static PLAYING: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// Plays a cue without blocking (a new one replaces one still playing), `gain_db` louder or quieter than the file;
/// falls back to the system beep. Whether a cue is wanted at all is decided by `Shared::sound`, not here.
pub fn play(sound: Sound, gain_db: f32) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    let original = sound.wav();
    let adjusted = if gain_db.abs() < 0.1 { None } else { scaled(original, gain_db) };
    let mut playing = PLAYING.lock().unwrap();
    let wav: &[u8] = adjusted.as_deref().unwrap_or(original);
    unsafe {
        // SND_ASYNC with SND_MEMORY needs the buffer to outlive the call: it is 'static, or kept in PLAYING below
        if PlaySoundW(wav.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) == 0 {
            MessageBeep(MB_OK);
        }
    }
    *playing = adjusted; // moving the Vec does not move its heap buffer; the previous one is freed only now
}

#[cfg(test)]
mod sound_scale_tests {
    use super::*;

    /// Where the samples start (ffmpeg adds a LIST chunk, so it is not always at 44).
    fn data_start(wav: &[u8]) -> usize {
        wav.windows(4).position(|w| w == b"data").expect("data chunk") + 8
    }

    fn peak(wav: &[u8]) -> i32 {
        wav[data_start(wav)..].chunks_exact(2).map(|s| (i16::from_le_bytes([s[0], s[1]]) as i32).abs()).max().unwrap_or(0)
    }

    #[test]
    fn six_db_down_halves_the_peak() {
        let wav = Sound::Saved.wav();
        let quiet = scaled(wav, -6.0).expect("pcm16");
        let ratio = peak(&quiet) as f32 / peak(wav) as f32;
        assert!((ratio - 0.501).abs() < 0.02, "ratio {ratio}");
        assert_eq!(quiet.len(), wav.len());
        let start = data_start(wav);
        assert_eq!(&quiet[..start], &wav[..start], "header untouched");
    }

    #[test]
    fn loud_gain_clips_instead_of_wrapping() {
        let wav = Sound::Saved.wav();
        let loud = scaled(wav, 60.0).expect("pcm16");
        assert!(peak(&loud) <= 32768);
    }

    #[test]
    fn not_a_wav_is_left_alone() {
        assert!(scaled(b"hello", 3.0).is_none());
    }
}

/// Make this process DPI aware so the popup is crisp and sized correctly on scaled displays.
pub fn make_dpi_aware() {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
    }
}

/// One attached monitor, in the order used by the capture index and the "monitor" setting.
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
    /// The panel's own name (from its EDID, e.g. "PHL 34M2C8600"); may be generic or empty.
    pub name: String,
}

pub fn monitors() -> Vec<MonitorInfo> {
    use windows_sys::Win32::Foundation::{BOOL, LPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW, HDC, HMONITOR, MONITORINFOEXW,
    };
    fn text(w: &[u16]) -> String {
        let n = w.iter().position(|c| *c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..n])
    }
    unsafe extern "system" fn cb(mon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        let out = &mut *(data as *mut Vec<MonitorInfo>);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut _) != 0 {
            let r = mi.monitorInfo.rcMonitor;
            let mut dd: DISPLAY_DEVICEW = std::mem::zeroed();
            dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            let name = if EnumDisplayDevicesW(mi.szDevice.as_ptr(), 0, &mut dd, 0) != 0 { text(&dd.DeviceString) } else { String::new() };
            out.push(MonitorInfo {
                left: r.left,
                top: r.top,
                width: r.right - r.left,
                height: r.bottom - r.top,
                primary: mi.monitorInfo.dwFlags & 1 != 0,
                name,
            });
        }
        1
    }
    let mut v: Vec<MonitorInfo> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(cb), &mut v as *mut _ as LPARAM);
    }
    v
}

fn monitor_handles() -> Vec<isize> {
    use windows_sys::Win32::Foundation::{BOOL, LPARAM};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    unsafe extern "system" fn cb(mon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        (*(data as *mut Vec<isize>)).push(mon as isize);
        1
    }
    let mut v: Vec<isize> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(cb), &mut v as *mut _ as LPARAM);
    }
    v
}

/// True while the process still exists.
pub fn pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == 259 // STILL_ACTIVE
    }
}

/// The top-most process of an executable (the one whose parent is not the same executable),
/// so that "include process tree" covers helper/child processes as well (Electron apps, browsers).
pub fn find_root_pid(stem: &str) -> Option<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let want = format!("{}.exe", stem.trim_end_matches(".exe").to_lowercase());
    let mut same: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            if name == want {
                same.push((e.th32ProcessID, e.th32ParentProcessID));
            }
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    same.iter()
        .find(|(_, parent)| !same.iter().any(|(pid, _)| pid == parent))
        .or(same.first())
        .map(|(pid, _)| *pid)
}

#[cfg(test)]
mod hotkey_tests {
    #[test]
    fn named_keys_parse() {
        use super::parse_hotkey;
        let ctrl = windows_sys::Win32::UI::Input::KeyboardAndMouse::MOD_CONTROL | windows_sys::Win32::UI::Input::KeyboardAndMouse::MOD_NOREPEAT;
        assert_eq!(parse_hotkey("Ctrl+PgDn"), Some((ctrl, 0x22)));
        assert_eq!(parse_hotkey("Ctrl+PgUp"), Some((ctrl, 0x21)));
        assert_eq!(parse_hotkey("Ctrl+Num5"), Some((ctrl, 0x65)));
        assert_eq!(parse_hotkey("Ctrl+VolDown"), Some((ctrl, 0xae)), "a key remapped to volume down");
        assert_eq!(parse_hotkey("Ctrl+F13"), Some((ctrl, 0x7c)));
        assert_eq!(parse_hotkey("Ctrl+Q").map(|p| p.1), Some('Q' as u32));
        assert_eq!(parse_hotkey("Ctrl+Nope"), None);
        assert_eq!(parse_hotkey("Ctrl+F"), Some((ctrl, 'F' as u32)), "a lone F is the letter");
    }

    #[test]
    fn windows_key_combos_parse() {
        use super::parse_hotkey;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, MOD_SHIFT, MOD_WIN};
        assert_eq!(parse_hotkey("Win+F9"), Some((MOD_WIN | MOD_NOREPEAT, 0x78)));
        assert_eq!(parse_hotkey("Win+Shift+R"), Some((MOD_WIN | MOD_SHIFT | MOD_NOREPEAT, 'R' as u32)));
        assert_eq!(parse_hotkey("Super+PgDn"), Some((MOD_WIN | MOD_NOREPEAT, 0x22)));
    }

    /// Takes a Windows-key shortcut for a moment (no other program uses Win+Shift+F9) and gives it back.
    #[test]
    #[ignore = "registers a real global hotkey"]
    fn windows_key_combo_registers() {
        super::register_hotkey_id("Win+Shift+F9", 77).expect("Win+Shift+F9 can be registered");
        super::unregister_hotkey_id(77);
    }

    #[test]
    fn icon_sources_split_path_and_index() {
        assert_eq!(super::icon_source(r"C:\a\b.exe"), (r"C:\a\b.exe".to_string(), 0));
        let (p, i) = super::icon_source(r"@%SystemRoot%\System32\AudioSrv.Dll,-203");
        assert_eq!(i, -203);
        assert!(p.to_lowercase().ends_with(r"\system32\audiosrv.dll") && !p.contains('%'), "{p}");
        assert!(super::icon_png(r"@%SystemRoot%\System32\AudioSrv.Dll,-203").is_some(), "the system sounds icon");
    }
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn exe_icon_is_png() {
        let png = super::exe_icon_png(r"C:\Windows\System32\notepad.exe").expect("icon");
        assert_eq!(&png[1..4], b"PNG");
        std::fs::write(std::env::temp_dir().join("icon_test.png"), png).unwrap();
    }
}
