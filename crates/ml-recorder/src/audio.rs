//! WASAPI capture of one audio source (loopback of a playback device, or a microphone).
//! The recorder owns capture so it can meter levels and notice device loss itself.

use crate::config::{DeviceRef, SourceCfg};
use crate::pick::{self, Endpoint, Role};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use wasapi::{DeviceEnumerator, Direction, Role as WinRole, SampleType, StreamMode, WaveFormat};

/// Config value meaning "whichever device Windows currently uses" (speakers, headset, ...).
pub const FOLLOW_DEFAULT: &str = "@default";
pub const RATE: u32 = 48_000;
/// Bytes per frame of the interleaved stereo f32 stream sent to ffmpeg.
pub const FRAME_BYTES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Capture what a playback device is playing.
    Loopback,
    Microphone,
}

/// Where a source takes its sound from, besides the plain device-loopback default.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AppSource {
    /// Normal: loopback of the configured playback device / the microphone.
    None,
    /// Only the sound of this program (and its child processes), e.g. the chat app.
    IncludeStem(String),
    /// Everything except this program; falls back to the device while that program is not running.
    ExcludeStem(String),
    /// Only the sound of one running process (the detected game).
    IncludePid(u32),
}

pub struct Sink {
    pub name: &'static str,
    pub kind: Kind,
    /// The configured primary/secondary device. Only changes when the user changes settings.
    pub cfg: Mutex<SourceCfg>,
    /// Substring used to guess a device by name on a fresh, unconfigured install.
    pub name_hint: &'static str,
    pub buf: Mutex<VecDeque<u8>>,
    /// Peak of the last chunk, in thousandths of full scale.
    pub level: AtomicU32,
    pub live: AtomicBool,
    /// Down for a reason worth a warning (a chosen device is missing, the capture died). A source that only waits for
    /// an app that is not running, or for a kind of device this PC does not have, is down but not faulty.
    pub fault: AtomicBool,
    pub device_name: Mutex<String>,
    pub role: Mutex<&'static str>,
    /// A short code for the UI to translate: `live`, `starting`, `wait_device:<name>`, `wait_app:<stem>`, `wait_game`,
    /// `no_device` or `error:<details>`.
    pub status: Mutex<String>,
    /// Bumped when the configuration changes so the capture thread re-evaluates immediately.
    pub generation: AtomicU32,
    /// Per-application capture (see `AppSource`).
    pub app: Mutex<AppSource>,
}

impl Sink {
    pub fn new(name: &'static str, kind: Kind, name_hint: &'static str) -> Arc<Sink> {
        Arc::new(Sink {
            name,
            kind,
            cfg: Mutex::new(SourceCfg::default()),
            name_hint,
            buf: Mutex::new(VecDeque::new()),
            level: AtomicU32::new(0),
            live: AtomicBool::new(false),
            fault: AtomicBool::new(false),
            device_name: Mutex::new(String::new()),
            role: Mutex::new("waiting"),
            status: Mutex::new("starting".into()),
            generation: AtomicU32::new(0),
            app: Mutex::new(AppSource::None),
        })
    }

    /// Applies new settings; the capture thread follows only if something actually changed.
    pub fn set_cfg(&self, new: SourceCfg) {
        let mut cur = self.cfg.lock().unwrap();
        if *cur != new {
            *cur = new;
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Switches between device loopback and per-application capture; reconnects only on a real change.
    pub fn set_app(&self, new: AppSource) {
        let mut cur = self.app.lock().unwrap();
        if *cur != new {
            *cur = new;
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn set_live(&self) {
        self.live.store(true, Ordering::Relaxed);
        self.fault.store(false, Ordering::Relaxed);
        *self.status.lock().unwrap() = "live".into();
    }

    fn set_down(&self, err: &str) {
        let (code, fault) = down_status(err);
        self.live.store(false, Ordering::Relaxed);
        self.fault.store(fault, Ordering::Relaxed);
        *self.status.lock().unwrap() = code;
    }

    /// Down and worth a warning (see `fault`).
    pub fn faulty(&self) -> bool {
        !self.live.load(Ordering::Relaxed) && self.fault.load(Ordering::Relaxed)
    }

    fn direction(&self) -> Direction {
        match self.kind {
            Kind::Loopback => Direction::Render,
            Kind::Microphone => Direction::Capture,
        }
    }
}

/// Active endpoints of one direction.
pub fn list_endpoints(en: &DeviceEnumerator, dir: &Direction) -> Vec<Endpoint> {
    let mut out = Vec::new();
    if let Ok(coll) = en.get_device_collection(dir) {
        for d in &coll {
            if let Ok(d) = d {
                out.push(Endpoint {
                    id: d.get_id().unwrap_or_default(),
                    name: d.get_friendlyname().unwrap_or_default(),
                });
            }
        }
    }
    out
}

/// A first-run guess: a playback device whose name contains the hint, else the Windows default.
fn auto_guess(en: &DeviceEnumerator, sink: &Sink, list: &[Endpoint]) -> Option<Endpoint> {
    if sink.kind == Kind::Loopback && !sink.name_hint.is_empty() {
        let hint = sink.name_hint.to_lowercase();
        if let Some(e) = list.iter().find(|e| e.name.to_lowercase().contains(&hint)) {
            return Some(e.clone());
        }
    }
    let role = if sink.name == "Chat" { WinRole::Communications } else { WinRole::Console };
    let d = en.get_default_device_for_role(&sink.direction(), &role).ok()?;
    Some(Endpoint { id: d.get_id().ok()?, name: d.get_friendlyname().ok()? })
}

fn default_endpoint(en: &DeviceEnumerator, dir: &Direction, communications: bool) -> Option<Endpoint> {
    let role = if communications { WinRole::Communications } else { WinRole::Console };
    let d = en.get_default_device_for_role(dir, &role).ok()?;
    Some(Endpoint { id: d.get_id().ok()?, name: d.get_friendlyname().ok()? })
}

/// Replaces the "follow the Windows default" marker with the device that is the default right now.
/// If Windows has no default, the marker becomes an id that matches nothing (the source waits).
fn materialize(cfg: &SourceCfg, en: &DeviceEnumerator, sink: &Sink) -> SourceCfg {
    let dir = sink.direction();
    let fix = |r: &Option<DeviceRef>| {
        r.as_ref().map(|d| {
            if d.id == FOLLOW_DEFAULT {
                match default_endpoint(en, &dir, sink.name == "Chat") {
                    Some(e) => DeviceRef { id: e.id, name: e.name },
                    None => DeviceRef { id: "@none".into(), name: String::new() },
                }
            } else {
                d.clone()
            }
        })
    };
    SourceCfg { primary: fix(&cfg.primary), secondary: fix(&cfg.secondary) }
}

/// Peak level (0..1) of one endpoint over `ms` milliseconds, for the settings meters. A playback device is
/// measured by loopback. Accepts "@default:render" / "@default:comm" / "@default:capture".
pub fn measure_peak(token: &str, ms: u64) -> f32 {
    let _ = wasapi::initialize_mta();
    let Ok(en) = DeviceEnumerator::new() else { return 0.0 };
    let id = match token {
        "@default:render" => default_endpoint(&en, &Direction::Render, false).map(|e| e.id),
        "@default:comm" => default_endpoint(&en, &Direction::Render, true).map(|e| e.id),
        "@default:capture" => default_endpoint(&en, &Direction::Capture, false).map(|e| e.id),
        other => Some(other.to_string()),
    };
    let app_client = if let Some(stem) = token.strip_prefix("@app:") {
        crate::win::find_root_pid(stem).and_then(|p| wasapi::AudioClient::new_application_loopback_client(p, true).ok())
    } else if let Some(stem) = token.strip_prefix("@appx:") {
        crate::win::find_root_pid(stem).and_then(|p| wasapi::AudioClient::new_application_loopback_client(p, false).ok())
    } else {
        None
    };
    let mut client = match app_client {
        Some(c) => c,
        None => {
            let Some(id) = id else { return 0.0 };
            let Ok(device) = en.get_device(&id) else { return 0.0 };
            let Ok(c) = device.get_iaudioclient() else { return 0.0 };
            c
        }
    };
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    let period = client.get_device_period().map(|(_, p)| p).unwrap_or(0);
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: if period > 0 { period.max(200_000) } else { 0 } };
    if client.initialize_client(&fmt, &Direction::Capture, &mode).is_err() {
        return 0.0;
    }
    let (Ok(event), Ok(capture)) = (client.set_get_eventhandle(), client.get_audiocaptureclient()) else {
        return 0.0;
    };
    if client.start_stream().is_err() {
        return 0.0;
    }
    let mut scratch: VecDeque<u8> = VecDeque::new();
    let mut peak = 0f32;
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        let _ = event.wait_for_event(20);
        if capture.read_from_device_to_deque(&mut scratch).is_err() {
            break;
        }
        let bytes: Vec<u8> = scratch.drain(..).collect();
        for s in bytes.chunks_exact(4) {
            peak = peak.max(f32::from_le_bytes([s[0], s[1], s[2], s[3]]).abs());
        }
    }
    let _ = client.stop_stream();
    peak.min(1.0)
}

fn capture_once(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let generation = sink.generation.load(Ordering::Relaxed);
    let en = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let dir = sink.direction();
    let list = list_endpoints(&en, &dir);
    let raw_cfg = sink.cfg.lock().unwrap().clone();
    let cfg = materialize(&raw_cfg, &en, sink);
    let guess = auto_guess(&en, sink, &list);
    let (role, chosen) = pick::resolve(&cfg, &list, guess.as_ref());
    let follows_default =
        role == Role::Primary && raw_cfg.primary.as_ref().is_some_and(|d| d.id == FOLLOW_DEFAULT);
    let Some(chosen) = chosen else {
        *sink.role.lock().unwrap() = Role::Waiting.label();
        // nothing of this kind in Windows (a PC without a microphone) and no particular device asked for: not a fault
        let chosen_by_user = [&raw_cfg.primary, &raw_cfg.secondary]
            .into_iter()
            .flatten()
            .any(|d| !d.id.is_empty() && d.id != FOLLOW_DEFAULT);
        if list.is_empty() && !chosen_by_user {
            return Err("no_device".into());
        }
        let wanted = cfg.primary.as_ref().map(|d| d.name.trim().to_string()).unwrap_or_default();
        return Err(format!("wait_device:{wanted}"));
    };

    let device = en.get_device(&chosen.id).map_err(|e| e.to_string())?;
    *sink.device_name.lock().unwrap() = chosen.name.clone();
    *sink.role.lock().unwrap() = if follows_default { "default" } else { role.label() };

    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    let (_, min_period) = client.get_device_period().map_err(|e| e.to_string())?;
    // 200 ms of slack so the once-a-second device check cannot overrun the endpoint buffer.
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min_period.max(2_000_000) };
    // For a playback device, Direction::Capture selects loopback.
    client.initialize_client(&fmt, &Direction::Capture, &mode).map_err(|e| e.to_string())?;
    let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
    client.start_stream().map_err(|e| e.to_string())?;
    sink.set_live();

    let mut scratch: VecDeque<u8> = VecDeque::new();
    let mut last_check = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if sink.generation.load(Ordering::Relaxed) != generation {
            let _ = client.stop_stream();
            return Err("reconfigured".into());
        }
        // Once a second: would the rules pick a different device now (primary came back, or ours left)?
        if last_check.elapsed() >= Duration::from_secs(1) {
            last_check = Instant::now();
            let now = list_endpoints(&en, &dir);
            let guess = auto_guess(&en, sink, &now);
            // a program we are supposed to exclude has started: switch from the device to app capture
            if let AppSource::ExcludeStem(s) = sink.app.lock().unwrap().clone() {
                if crate::win::find_root_pid(&s).is_some() {
                    let _ = client.stop_stream();
                    return Err("reconfigured".into());
                }
            }
            let live_cfg = materialize(&sink.cfg.lock().unwrap().clone(), &en, sink);
            let (r, e) = pick::resolve(&live_cfg, &now, guess.as_ref());
            if r != role || e.map(|e| e.id) != Some(chosen.id.clone()) {
                let _ = client.stop_stream();
                return Err("reconfigured".into());
            }
        }
        // Loopback of a silent device delivers no packets, so poll on a short timeout.
        let _ = event.wait_for_event(20);
        // one read takes one packet: drain them all, or a backlog after any hiccup stays until the buffer overflows
        while capture.get_next_packet_size().map_err(|e| e.to_string())?.unwrap_or(0) > 0 {
            capture.read_from_device_to_deque(&mut scratch).map_err(|e| e.to_string())?;
        }
        if scratch.is_empty() {
            sink.level.store(0, Ordering::Relaxed);
            continue;
        }
        let bytes: Vec<u8> = scratch.drain(..).collect();
        let mut peak = 0f32;
        for s in bytes.chunks_exact(4) {
            let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]).abs();
            if v > peak {
                peak = v;
            }
        }
        sink.level.store((peak.min(1.0) * 1000.0) as u32, Ordering::Relaxed);
        let mut q = sink.buf.lock().unwrap();
        q.extend(bytes);
        // Never let the queue grow without bound if the feeder is not draining.
        let cap = RATE as usize * FRAME_BYTES * 2;
        if q.len() > cap {
            let drop = q.len() - cap;
            q.drain(..drop);
        }
    }
    let _ = client.stop_stream();
    Ok(())
}

/// Per-application capture (Windows 10 2004+): only one program's sound, or everything except one.
fn capture_app_once(sink: &Sink, stop: &AtomicBool, src: &AppSource) -> Result<(), String> {
    let generation = sink.generation.load(Ordering::Relaxed);
    // the label is the program's stem (the UI names it and words "everything except"); empty for the detected game
    let (pid, include, label) = match src {
        AppSource::IncludeStem(s) => (crate::win::find_root_pid(s), true, s.clone()),
        AppSource::ExcludeStem(s) => (crate::win::find_root_pid(s), false, s.clone()),
        AppSource::IncludePid(p) => (crate::win::pid_alive(*p).then_some(*p), true, String::new()),
        AppSource::None => return Err("no application source".into()),
    };
    let Some(pid) = pid else {
        *sink.role.lock().unwrap() = Role::Waiting.label();
        return Err(if label.is_empty() { "wait_game".into() } else { format!("wait_app:{label}") });
    };
    *sink.device_name.lock().unwrap() = label;
    *sink.role.lock().unwrap() = if include { "app" } else { "appx" };

    let mut client = wasapi::AudioClient::new_application_loopback_client(pid, include).map_err(|e| e.to_string())?;
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, RATE as usize, 2, None);
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 2_000_000 };
    client.initialize_client(&fmt, &Direction::Capture, &mode).map_err(|e| e.to_string())?;
    let event = client.set_get_eventhandle().map_err(|e| e.to_string())?;
    let capture = client.get_audiocaptureclient().map_err(|e| e.to_string())?;
    client.start_stream().map_err(|e| e.to_string())?;
    sink.set_live();

    // Whether the program restarted or exited is checked on its own thread: the process snapshot takes tens of
    // ms while a game runs, and the app-loopback client does not buffer that long (a 20 ms hole every second).
    let gone = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    {
        let (gone, done, src) = (gone.clone(), done.clone(), src.clone());
        thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(1));
                let now = match &src {
                    AppSource::IncludeStem(s) | AppSource::ExcludeStem(s) => crate::win::find_root_pid(s),
                    AppSource::IncludePid(p) => crate::win::pid_alive(*p).then_some(*p),
                    AppSource::None => None,
                };
                if now != Some(pid) {
                    gone.store(true, Ordering::Relaxed);
                    return;
                }
            }
        });
    }
    let result = app_loop(sink, stop, generation, &gone, &event, &capture);
    done.store(true, Ordering::Relaxed);
    let _ = client.stop_stream();
    result
}

fn app_loop(
    sink: &Sink,
    stop: &AtomicBool,
    generation: u32,
    gone: &AtomicBool,
    event: &wasapi::Handle,
    capture: &wasapi::AudioCaptureClient,
) -> Result<(), String> {
    let mut scratch: VecDeque<u8> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        if sink.generation.load(Ordering::Relaxed) != generation || gone.load(Ordering::Relaxed) {
            return Err("reconfigured".into()); // settings changed, or the program restarted or exited
        }
        let _ = event.wait_for_event(20);
        // one read takes one packet: drain them all, or a backlog after any hiccup stays until the buffer overflows
        while capture.get_next_packet_size().map_err(|e| e.to_string())?.unwrap_or(0) > 0 {
            capture.read_from_device_to_deque(&mut scratch).map_err(|e| e.to_string())?;
        }
        if scratch.is_empty() {
            sink.level.store(0, Ordering::Relaxed);
            continue;
        }
        let bytes: Vec<u8> = scratch.drain(..).collect();
        let mut peak = 0f32;
        for s in bytes.chunks_exact(4) {
            peak = peak.max(f32::from_le_bytes([s[0], s[1], s[2], s[3]]).abs());
        }
        sink.level.store((peak.min(1.0) * 1000.0) as u32, Ordering::Relaxed);
        let mut q = sink.buf.lock().unwrap();
        q.extend(bytes);
        let cap = RATE as usize * FRAME_BYTES * 2;
        if q.len() > cap {
            let drop = q.len() - cap;
            q.drain(..drop);
        }
    }
    Ok(())
}

/// One capture attempt using whatever the current configuration asks for.
fn run_source(sink: &Sink, stop: &AtomicBool) -> Result<(), String> {
    let app = sink.app.lock().unwrap().clone();
    match app {
        AppSource::None => capture_once(sink, stop),
        AppSource::ExcludeStem(ref s) if crate::win::find_root_pid(s).is_none() => capture_once(sink, stop),
        other => capture_app_once(sink, stop, &other),
    }
}

/// Runs forever: captures, and on any failure (device unplugged, driver reset) reports it and retries.
pub fn spawn_capture(sink: Arc<Sink>, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name(format!("capture-{}", sink.name))
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            while !stop.load(Ordering::Relaxed) {
                match run_source(&sink, &stop) {
                    Ok(()) => break,
                    Err(e) if e == "reconfigured" => {}
                    Err(e) => {
                        sink.level.store(0, Ordering::Relaxed);
                        sink.set_down(&e);
                        // Poll quickly while waiting so a reconnect is picked up within about a second.
                        let seen = sink.generation.load(Ordering::Relaxed);
                        for _ in 0..4 {
                            if stop.load(Ordering::Relaxed) || sink.generation.load(Ordering::Relaxed) != seen {
                                break;
                            }
                            thread::sleep(Duration::from_millis(250));
                        }
                    }
                }
            }
        })
        .expect("spawn capture thread");
}

/// The status code a down source reports, and whether it is a fault. Waiting for an app that is not running (the
/// chat app, the game) or for a kind of device the PC does not have is expected, not a fault; a missing chosen device
/// or a failed capture is. Anything that is not one of our codes is a raw error and becomes `error:<details>`.
fn down_status(err: &str) -> (String, bool) {
    if err.starts_with("wait_app:") || err == "wait_game" || err == "no_device" {
        (err.to_string(), false)
    } else if err.starts_with("wait_device:") {
        (err.to_string(), true)
    } else {
        (format!("error:{err}"), true)
    }
}

#[cfg(test)]
mod tests {
    use super::down_status;

    #[test]
    fn only_real_problems_count_as_faults() {
        assert_eq!(down_status("wait_app:discord"), ("wait_app:discord".to_string(), false), "chat app not running");
        assert_eq!(down_status("wait_game"), ("wait_game".to_string(), false), "the game just exited");
        assert_eq!(down_status("no_device"), ("no_device".to_string(), false), "a PC without a microphone");
        assert_eq!(down_status("wait_device:Headset"), ("wait_device:Headset".to_string(), true), "a chosen device is gone");
        assert_eq!(down_status("0x88890004"), ("error:0x88890004".to_string(), true), "the capture died");
    }
}
