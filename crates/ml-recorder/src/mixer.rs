//! The volume mixer behind the audio tray icon (EarTrumpet style): playback devices with their master volume and
//! the apps playing on each, read and changed through the documented Core Audio interfaces. Everything here is
//! used on the tray thread only (COM objects of an `IMMDevice` stay on the apartment that made them).

use windows::core::{Interface, BOOL, PWSTR};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};

/// One app on a device. Its processes' sessions (a browser has several) are moved together.
pub struct App {
    /// Stable across refreshes: "#system", the lower-case exe path, or "pid:N".
    pub key: String,
    pub name: String,
    /// Where its icon comes from: "path" or "path,index" (index < 0 = resource id), "" = none.
    pub icon: String,
    /// This process's own session (the recorder's cues and its loopback captures).
    own: bool,
    /// Its processes (a browser has several): per-app output pinning is set per process.
    pub pids: Vec<u32>,
    vols: Vec<ISimpleAudioVolume>,
    meters: Vec<IAudioMeterInformation>,
}

pub struct Dev {
    pub id: String,
    pub name: String,
    pub default: bool,
    /// The Windows communications device (a headset's chat half), when it is not also the default.
    pub comms: bool,
    vol: IAudioEndpointVolume,
    meter: Option<IAudioMeterInformation>,
    pub apps: Vec<App>,
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

impl Dev {
    pub fn volume(&self) -> f32 {
        unsafe { self.vol.GetMasterVolumeLevelScalar().unwrap_or(0.0) }
    }
    pub fn muted(&self) -> bool {
        unsafe { self.vol.GetMute().map(|b| b.as_bool()).unwrap_or(false) }
    }
    pub fn set_volume(&self, v: f32) {
        unsafe {
            let _ = self.vol.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
            // like the Windows flyout: moving the slider un-mutes
            if self.muted() {
                let _ = self.vol.SetMute(false, std::ptr::null());
            }
        }
    }
    pub fn set_muted(&self, m: bool) {
        unsafe {
            let _ = self.vol.SetMute(m, std::ptr::null());
        }
    }
    /// Sets the volume and leaves mute as it is.
    pub fn set_level(&self, v: f32) {
        unsafe {
            let _ = self.vol.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
        }
    }
    pub fn peak(&self) -> f32 {
        self.meter.as_ref().and_then(|m| unsafe { m.GetPeakValue().ok() }).unwrap_or(0.0)
    }

    /// The meters of all its apps, except apps that only record this output (loopback: Buffer itself, SignalRGB,
    /// OBS): their session meters the device's whole mix, so every sound would seem to come from them too.
    /// Nothing in the session tells them apart (measured: same identifiers as a player), but their peak follows
    /// the device meter exactly (measured): equal to it where the volume is applied in hardware, the device peak
    /// times the endpoint's software attenuation where it is not (90 % = -1.6 dB = x0.83). A real player alone
    /// reads the device peak *divided* by that factor, so the match is checked on several reads in a row before an
    /// app is marked, and where the factor is 1 only while something else plays too. A clear miss unmarks it.
    /// Buffer's own session is always a mirror (it records every output it captures).
    pub fn app_peaks(&self) -> Vec<f32> {
        static SEEN: std::sync::Mutex<Vec<(String, i8)>> = std::sync::Mutex::new(Vec::new());
        let d = self.peak();
        let amp = unsafe { self.vol.GetMasterVolumeLevel() }.map(|db| 10f32.powf(db / 20.0)).unwrap_or(1.0);
        let raw: Vec<f32> = self.apps.iter().map(|a| a.peak()).collect();
        let near = |a: f32, b: f32| (a - b).abs() <= 0.003 + 0.02 * b;
        let mirrorish = |p: f32| near(p, d) || (amp < 0.97 && near(p, d * amp));
        let mut seen = SEEN.lock().unwrap();
        self.apps
            .iter()
            .zip(&raw)
            .enumerate()
            .map(|(i, (a, &p))| {
                if a.own {
                    return 0.0;
                }
                let key = format!("{}|{}", self.id, a.key);
                let at = match seen.iter().position(|(k, _)| *k == key) {
                    Some(at) => at,
                    None => {
                        if seen.len() > 200 {
                            seen.remove(0);
                        }
                        seen.push((key, 0));
                        seen.len() - 1
                    }
                };
                if d > 0.01 && p > 0.005 {
                    // with no software attenuation a lone player equals the device too: only judge while another
                    // source plays something that is not the device mix
                    let others = raw.iter().enumerate().any(|(j, &q)| j != i && q > 0.01 && !mirrorish(q));
                    let mirror = (amp < 0.97 && near(p, d * amp)) || (near(p, d) && others);
                    let n = &mut seen[at].1;
                    *n = if mirror { (*n).max(0) + 1 } else if near(p, d) || near(p, d * amp) { *n } else { -1 };
                    *n = (*n).min(10);
                }
                // marked after 4 matching reads in a row
                if seen[at].1 >= 4 { 0.0 } else { p }
            })
            .collect()
    }
}

impl App {
    pub fn volume(&self) -> f32 {
        self.vols.first().and_then(|v| unsafe { v.GetMasterVolume().ok() }).unwrap_or(0.0)
    }
    pub fn muted(&self) -> bool {
        self.vols.first().and_then(|v| unsafe { v.GetMute().ok() }).is_some_and(|b: BOOL| b.as_bool())
    }
    pub fn set_volume(&self, x: f32) {
        let un = self.muted();
        for v in &self.vols {
            unsafe {
                let _ = v.SetMasterVolume(x.clamp(0.0, 1.0), std::ptr::null());
                if un {
                    let _ = v.SetMute(false, std::ptr::null());
                }
            }
        }
    }
    pub fn set_muted(&self, m: bool) {
        for v in &self.vols {
            unsafe {
                let _ = v.SetMute(m, std::ptr::null());
            }
        }
    }
    pub fn peak(&self) -> f32 {
        self.meters.iter().filter_map(|m| unsafe { m.GetPeakValue().ok() }).fold(0.0, f32::max)
    }
}

fn title_case(stem: &str) -> String {
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn apps_of(dev: &wasapi::Device) -> Vec<App> {
    let mut apps: Vec<App> = Vec::new();
    let Ok(mgr) = (unsafe { dev.as_immdevice().Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) }) else { return apps };
    let Ok(list) = (unsafe { mgr.GetSessionEnumerator() }) else { return apps };
    let n = unsafe { list.GetCount() }.unwrap_or(0);
    for i in 0..n {
        let Ok(ctl) = (unsafe { list.GetSession(i) }) else { continue };
        if unsafe { ctl.GetState() }.map(|s| s == AudioSessionStateExpired).unwrap_or(true) {
            continue;
        }
        let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
        let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
        let meter = ctl.cast::<IAudioMeterInformation>().ok();
        // S_OK (0) = the system sounds session, S_FALSE (1) = an app
        let system = unsafe { ctl2.IsSystemSoundsSession() }.0 == 0;
        let pid = unsafe { ctl2.GetProcessId() }.unwrap_or(0);
        let exe = if system { None } else { crate::win::exe_path_of(pid) };
        let key = if system { "#system".to_string() } else { exe.as_ref().map(|e| e.to_lowercase()).unwrap_or(format!("pid:{pid}")) };
        if let Some(a) = apps.iter_mut().find(|a| a.key == key) {
            a.vols.push(vol);
            a.meters.extend(meter);
            if !a.pids.contains(&pid) {
                a.pids.push(pid);
            }
            continue;
        }
        let shown = take(unsafe { ctl.GetDisplayName() }.unwrap_or(PWSTR::null()));
        let icon_path = take(unsafe { ctl.GetIconPath() }.unwrap_or(PWSTR::null()));
        let stem = exe.as_deref().and_then(|e| std::path::Path::new(e).file_stem()).map(|s| s.to_string_lossy().into_owned());
        let name = if system {
            crate::lang::t("mix.system")
        } else if !shown.is_empty() && !shown.starts_with('@') {
            shown
        } else {
            stem.map(|s| title_case(&s)).unwrap_or_else(|| format!("PID {pid}"))
        };
        let icon = if !icon_path.is_empty() { icon_path.trim_start_matches('@').to_string() } else { exe.unwrap_or_default() };
        apps.push(App { key, name, icon, own: pid == std::process::id(), pids: vec![pid], vols: vec![vol], meters: meter.into_iter().collect() });
    }
    // system sounds first (as in the Windows mixer), then by name
    apps.sort_by(|a, b| (a.key != "#system", a.name.to_lowercase()).cmp(&(b.key != "#system", b.name.to_lowercase())));
    apps
}

/// Where a device goes in the list: the default output, then the chat device, then devices that have apps on
/// them, then the idle rest; by name within each group (Windows' own order shifts with every plug-in).
fn order(d: &Dev) -> (u8, String) {
    let rank = if d.default {
        0
    } else if d.comms {
        1
    } else if d.apps.iter().any(|a| a.key != "#system") {
        2
    } else {
        3
    };
    (rank, d.name.to_lowercase())
}

/// Active playback devices in display order (see `order`), each with the apps playing on it.
pub fn snapshot() -> Vec<Dev> {
    let _ = wasapi::initialize_mta();
    let cur = crate::endpoints::default_output().unwrap_or_default();
    let comms = crate::endpoints::default_comms_output().unwrap_or_default();
    let Ok(en) = wasapi::DeviceEnumerator::new() else { return Vec::new() };
    let Ok(coll) = en.get_device_collection(&wasapi::Direction::Render) else { return Vec::new() };
    let mut devs: Vec<Dev> = coll
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let id = d.get_id().ok()?;
            let name = d.get_friendlyname().ok()?;
            let vol = unsafe { d.as_immdevice().Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }.ok()?;
            let meter = unsafe { d.as_immdevice().Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) }.ok();
            let apps = apps_of(&d);
            Some(Dev { default: id == cur, comms: id == comms && id != cur, id, name, vol, meter, apps })
        })
        .collect();
    devs.sort_by_cached_key(order);
    devs
}

/// Small enough not to need a crate: the app shows app icons as data URLs.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// The app's mixer page: `mix-get` (devices, apps, volumes, peaks), `mix-set {"dev","app"?,"volume"?,"muted"?}`,
/// `mix-default <id>`, `mix-icon <icon source>` (PNG as base64).
pub fn command(shared: &crate::session::Shared, cmd: &str, arg: &str) -> serde_json::Value {
    use serde_json::json;
    match cmd {
        "mix-get" => {
            let snap = snapshot();
            crate::appdev::settle(&snap);
            // apps pinned to an output with Windows' per-app setting, and the ones Buffer pinned that aren't playing
            let running: Vec<&str> = snap.iter().flat_map(|d| d.apps.iter().map(|a| a.key.as_str())).collect();
            let idle: Vec<crate::appdev::Pin> = crate::appdev::pins().into_iter().filter(|p| !p.unpin && !running.contains(&p.key.as_str())).collect();
            let pinned = crate::appdev::app_outputs(&snap.iter().flat_map(|d| d.apps.iter().map(|a| a.pids.first().copied().unwrap_or(0))).collect::<Vec<_>>());
            let mut pinned = pinned.into_iter();
            let devs: Vec<_> = snap
                .iter()
                .map(|d| {
                    let peaks = d.app_peaks();
                    let apps: Vec<_> = d
                        .apps
                        .iter()
                        .zip(peaks)
                        .map(|(a, pk)| {
                            let pin = pinned.next().flatten();
                            json!({ "key": a.key, "name": a.name, "icon": a.icon, "volume": a.volume(), "muted": a.muted(), "peak": pk,
                                    "pinned": pin.is_some(), "pinned_to": pin, "system": a.key == "#system" })
                        })
                        .collect();
                    let idle: Vec<_> = idle.iter().filter(|p| p.dev == d.id).map(|p| json!({ "key": p.key, "name": p.name, "icon": p.icon })).collect();
                    json!({ "id": d.id, "name": d.name, "default": d.default, "comms": d.comms, "volume": d.volume(), "muted": d.muted(), "peak": d.peak(), "apps": apps, "idle": idle })
                })
                .collect();
            json!({ "ok": true, "devices": devs, "display_audio": crate::keepawake::present() })
        }
        // `mix-move {"app": key, "to": endpoint id}`: the app plays on that output from now on (the default one = unpin)
        "mix-move" => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(arg) else { return json!({ "ok": false, "error": "bad json" }) };
            let (key, to) = (v["app"].as_str().unwrap_or(""), v["to"].as_str().unwrap_or(""));
            let snap = snapshot();
            let Some(app) = snap.iter().flat_map(|d| d.apps.iter()).find(|a| a.key == key && !a.pids.is_empty() && a.key != "#system") else {
                return json!({ "ok": false, "error": "the app is not playing" });
            };
            match crate::appdev::move_app(app, to) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        // `mix-unpin <app key>`: back to following the default output (a closed app: the next time it runs)
        "mix-unpin" => {
            let snap = snapshot();
            let app = snap.iter().flat_map(|d| d.apps.iter()).find(|a| a.key == arg);
            match crate::appdev::unpin(arg, app) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        "mix-set" => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(arg) else { return json!({ "ok": false, "error": "bad json" }) };
            let (dev, app) = (v["dev"].as_str().unwrap_or(""), v["app"].as_str());
            let (vol, muted) = (v["volume"].as_f64().map(|x| x as f32), v["muted"].as_bool());
            let devs = snapshot();
            let Some(d) = devs.iter().find(|d| d.id == dev) else { return json!({ "ok": false, "error": "no such device" }) };
            match app {
                Some(key) => {
                    let Some(a) = d.apps.iter().find(|a| a.key == key) else { return json!({ "ok": false, "error": "no such app" }) };
                    if let Some(x) = vol {
                        a.set_volume(x);
                    }
                    if let Some(m) = muted {
                        a.set_muted(m);
                    }
                }
                None => {
                    if let Some(x) = vol {
                        d.set_volume(x);
                    }
                    if let Some(m) = muted {
                        d.set_muted(m);
                    }
                }
            }
            json!({ "ok": true })
        }
        "mix-default" => match crate::endpoints::set_default_output(arg) {
            Ok(()) => {
                crate::eq::follow_output(shared);
                json!({ "ok": true })
            }
            Err(e) => json!({ "ok": false, "error": e }),
        },
        "mix-icon" => match crate::win::icon_png(arg) {
            Some(png) => json!({ "ok": true, "png": base64(&png) }),
            None => json!({ "ok": false }),
        },
        "mix-flyout" if cfg!(debug_assertions) => {
            crate::flyout::OPEN_REQUEST.store(true, std::sync::atomic::Ordering::Relaxed);
            crate::overlay::wake();
            json!({ "ok": true })
        }
        _ => json!({ "ok": false, "error": format!("unknown command '{cmd}'") }),
    }
}

/// The default output alone (the tray icon's wheel and tooltip): no session enumeration.
pub fn default_dev() -> Option<Dev> {
    let _ = wasapi::initialize_mta();
    let en = wasapi::DeviceEnumerator::new().ok()?;
    let d = en.get_default_device_for_role(&wasapi::Direction::Render, &wasapi::Role::Console).ok()?;
    let vol = unsafe { d.as_immdevice().Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }.ok()?;
    Some(Dev { id: d.get_id().ok()?, name: d.get_friendlyname().unwrap_or_default(), default: true, comms: false, vol, meter: None, apps: Vec::new() })
}

/// One wheel notch = 5 % (the user's choice; EarTrumpet uses 2), snapped to the grid so steps land on round numbers.
pub fn step(v: f32, notches: f32) -> f32 {
    (((v * 100.0 / 5.0).round() + notches) * 5.0 / 100.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn wheel_steps_snap_to_five_percent() {
        assert!((super::step(0.74, 1.0) - 0.80).abs() < 1e-6, "74 snaps to 75, then one step");
        assert!((super::step(0.731, -1.0) - 0.70).abs() < 1e-6);
        assert_eq!(super::step(0.99, 3.0), 1.0);
        assert_eq!(super::step(0.04, -2.0), 0.0);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }

    /// On a real machine: `cargo test -p ml-recorder mixer -- --ignored --nocapture`. Lists what the flyout shows.
    #[test]
    #[ignore]
    fn lists_devices_and_apps() {
        for d in super::snapshot() {
            println!("{} {} {:.0}% muted={} peak={:.2}", if d.default { "*" } else { " " }, d.name, d.volume() * 100.0, d.muted(), d.peak());
            for (a, pk) in d.apps.iter().zip(d.app_peaks()) {
                println!("    {} [{}] {:.0}% peak={:.3} icon={}", a.name, a.key, a.volume() * 100.0, pk, a.icon);
            }
        }
    }
}
