//! Tray icon (always-visible recorder state), the global save hotkey, the "Buffer active" popup and
//! game detection. They share one thread because they all need that thread's Win32 message loop.

use crate::overlay::{self, Event, Overlay, Popup};
use crate::audio::AppSource;
use crate::session::ActiveGame;
use crate::{session, win};
use std::{
    collections::HashSet,
    process::Command,
    sync::{atomic::Ordering, mpsc::Receiver, Arc},
    thread,
};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

/// The audio part of the tray menu, EarTrumpet style: pick the output, EQ on/off, preset of the output in use,
/// hear my mic. Rebuilt only when something it shows has changed.
struct AudioMenu {
    output: Submenu,
    preset: Submenu,
    eq_on: CheckMenuItem,
    listen: CheckMenuItem,
    outputs: Vec<(CheckMenuItem, String)>,
    presets: Vec<(CheckMenuItem, String)>,
    shown: String,
}

/// The mic "Hear my mic" acts on: the EQ's mic, else the Windows default (it works without the EQ too).
fn mic_of(st: &ml_eq::EqState) -> String {
    if st.mic_device.is_empty() { crate::endpoints::default_capture().unwrap_or_default() } else { st.mic_device.clone() }
}

impl AudioMenu {
    fn new() -> Self {
        AudioMenu {
            output: Submenu::new(crate::lang::t("menu.output"), true),
            preset: Submenu::new(crate::lang::t("menu.eq_preset"), true),
            eq_on: CheckMenuItem::new(crate::lang::t("menu.eq_on"), true, false, None),
            listen: CheckMenuItem::new(crate::lang::t("menu.listen"), true, false, None),
            outputs: Vec::new(),
            presets: Vec::new(),
            shown: String::new(),
        }
    }

    fn refresh(&mut self, shared: &session::Shared) {
        let cur = crate::endpoints::default_output().unwrap_or_default();
        let hidden = shared.cfg.lock().unwrap().hidden_outputs.clone();
        let outs: Vec<_> = crate::endpoints::outputs().into_iter().filter(|e| e.id == cur || !hidden.contains(&e.id)).collect();
        let (enabled, preset, mut ids, mic) = {
            let st = shared.eq.lock().unwrap();
            let ids: Vec<(String, String)> = ml_eq::presets::IDS
                .iter()
                .map(|id| (id.to_string(), crate::lang::t(&format!("preset.{id}"))))
                // an edited built-in is an override under the same id: it must not appear twice
                .chain(st.user_presets.iter().filter(|p| !ml_eq::presets::IDS.contains(&p.name.as_str())).map(|p| (p.name.clone(), p.name.clone())))
                .collect();
            let preset = st.outputs.get(&cur).map(|c| c.preset.clone()).unwrap_or_else(|| "flat".into());
            (st.enabled, preset, ids, mic_of(&st))
        };
        let apo = ml_eq::apo::config_dir().is_some();
        let listening = !mic.is_empty() && crate::endpoints::listening(&mic);
        let sig = format!("{outs:?}|{cur}|{enabled}|{preset}|{ids:?}|{listening}|{apo}");
        if sig == self.shown {
            return;
        }
        self.shown = sig;
        for (item, _) in self.outputs.drain(..) {
            let _ = self.output.remove(&item);
        }
        for e in outs {
            let item = CheckMenuItem::new(&e.name, true, e.id == cur, None);
            let _ = self.output.append(&item);
            self.outputs.push((item, e.id));
        }
        for (item, _) in self.presets.drain(..) {
            let _ = self.preset.remove(&item);
        }
        for (id, label) in ids.drain(..) {
            let item = CheckMenuItem::new(label, apo && enabled, id == preset, None);
            let _ = self.preset.append(&item);
            self.presets.push((item, id));
        }
        self.eq_on.set_enabled(apo);
        self.eq_on.set_checked(enabled);
        self.listen.set_enabled(!mic.is_empty());
        self.listen.set_checked(listening);
    }

    /// True when the click was one of ours.
    fn handle(&mut self, shared: &session::Shared, id: &MenuId) -> bool {
        if let Some((_, dev)) = self.outputs.iter().find(|(i, _)| i.id() == id) {
            match crate::endpoints::set_default_output(dev) {
                Ok(()) => crate::eq::follow_output(shared),
                Err(e) => session::log(&format!("output switch: {e}")),
            }
        } else if let Some((_, p)) = self.presets.iter().find(|(i, _)| i.id() == id) {
            if let Some(dev) = crate::endpoints::default_output() {
                let p = p.clone();
                crate::eq::update(shared, |st| {
                    // picking a preset means "I want to hear it": switch the EQ on with it
                    if st.set_preset(&dev, &p) {
                        st.enabled = true;
                    }
                });
            }
        } else if id == self.eq_on.id() {
            crate::eq::update(shared, |st| st.enabled = !st.enabled);
        } else if id == self.listen.id() {
            let mic = mic_of(&shared.eq.lock().unwrap());
            if let Err(e) = crate::endpoints::set_listen(&mic, !crate::endpoints::listening(&mic), None) {
                session::log(&format!("mic listen: {e}"));
            }
        } else {
            return false;
        }
        self.shown.clear(); // check marks follow the new state on the next refresh
        self.refresh(shared);
        true
    }
}

/// What the icon says. The logo mark carries the colour; a corner badge appears only for stopped, paused and errors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Recording, everything works: the clean lime logo (default, no badge).
    Recording,
    /// Recording, but a problem was detected (a source is down): amber logo + "!".
    Problem,
    /// Not recording although it should (ffmpeg failed, ...): red logo + X.
    Stopped,
    /// Waiting for a game ("record only games"): grey logo + pause bars.
    Idle,
    /// Just started: grey logo until the first segment arrives.
    Starting,
    /// Post-roll running, the clip will be saved in a moment: lime logo (tooltip has the countdown).
    Saving,
    /// A clip was just saved: lime logo (tooltip says so).
    Saved,
    /// Saving failed: red logo + X.
    SaveFailed,
    /// A long recording is running: lime logo + red dot.
    LongRec,
}

/// Programs that are never treated as games even when they cover the whole screen.
pub const NEVER_GAMES: &[&str] = &[
    "explorer", "searchhost", "shellexperiencehost", "applicationframehost", "startmenuexperiencehost",
    "lockapp", "chrome", "msedge", "firefox", "opera", "brave", "vivaldi", "vlc", "mpv", "potplayer",
    "code", "devenv", "powerpnt", "winword", "excel", "buffer", "buffer-recorder", "obs64", "teams",
    "zoom", "discord", "messy", "messenger", "whatsapp", "telegram", "spotify", "slack", "steamwebhelper", "taskmgr", "mstsc", "vmware", "virtualbox",
    // screenshot tools cover the whole screen while you pick an area (Win+Shift+S)
    "snippingtool", "screenclippinghost", "screensketch", "textinputhost", "gamebar", "widgets", "searchapp",
];

fn colour(state: State) -> (u8, u8, u8) {
    match state {
        State::Recording | State::Saving | State::Saved | State::LongRec => (0xc6, 0xf4, 0x32),
        State::Problem => (0xff, 0xb4, 0x54),
        State::Stopped | State::SaveFailed => (0xff, 0x5d, 0x5d),
        State::Idle | State::Starting => (0x8b, 0x91, 0x9c),
    }
}

/// 32x32 icon: the logo mark (a play triangle of scan-lines with faded tails) in the state colour and a
/// 15 px badge in the bottom-right corner that says what is going on.
fn icon(state: State) -> Icon {
    Icon::from_rgba(icon_rgba(state), 32, 32).expect("valid icon")
}

fn icon_rgba(state: State) -> Vec<u8> {
    const N: usize = 32;
    let (r, g, b) = colour(state);
    let mut px = vec![0u8; N * N * 4];
    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) {
            let i = (y as usize * N + x as usize) * 4;
            px[i..i + 4].copy_from_slice(&c);
        }
    };
    // logo: 7 bars of 3px with 1px gaps, widest in the middle (thicker than the full logo, for 16-32px)
    let tails = [9i32, 3, 11, 5, 8, 2, 10];
    for (i, tail) in tails.iter().enumerate() {
        let y0 = 2 + i as i32 * 4;
        let w = 6 + (3 - (i as i32 - 3).abs()) * 5;
        for dy in 0..3 {
            for x in 0..*tail {
                put((10 - tail).max(0) + x, y0 + dy, [r, g, b, 80]);
            }
            for x in 0..w {
                put(10 + x, y0 + dy, [r, g, b, 255]);
            }
        }
    }

    // badge centre and helpers
    let (cx, cy) = (23i32, 23i32);
    let dark = [17u8, 17, 17, 255];
    let white = [255u8, 255, 255, 255];
    let disc = |put: &mut dyn FnMut(i32, i32, [u8; 4]), rad: f32, c: [u8; 4]| {
        for dy in -9..=9 {
            for dx in -9..=9 {
                if ((dx * dx + dy * dy) as f32) <= rad * rad {
                    put(cx + dx, cy + dy, c);
                }
            }
        }
    };
    // Outline so a badge reads on any taskbar colour. Healthy recording has no badge at all:
    // a red dot looked like an error, so "all good" is simply the clean lime logo.
    let badge = matches!(state, State::Problem | State::Stopped | State::SaveFailed | State::Idle | State::LongRec);
    if badge {
        disc(&mut put, 8.2, dark);
    }
    match state {
        State::Recording => {}
        State::Problem => {
            disc(&mut put, 6.2, [0xff, 0xb4, 0x54, 255]);
            for dy in -4..=1 {
                for dx in -1..=0 {
                    put(cx + dx, cy + dy, dark);
                }
            }
            for dy in 3..=4 {
                for dx in -1..=0 {
                    put(cx + dx, cy + dy, dark);
                }
            }
        }
        State::Stopped | State::SaveFailed => {
            disc(&mut put, 6.2, [0xff, 0x5d, 0x5d, 255]);
            for d in -3..=3i32 {
                // a 2px-thick X
                for off in [0i32, 1] {
                    put(cx + d + off, cy + d, white);
                    put(cx + d + off, cy - d, white);
                }
            }
        }
        State::LongRec => disc(&mut put, 6.2, [0xff, 0x3b, 0x3b, 255]),
        State::Idle => {
            disc(&mut put, 6.2, [0x8b, 0x91, 0x9c, 255]);
            for dy in -3..=3 {
                for dx in [-3, -2, 1, 2] {
                    put(cx + dx, cy + dy, dark);
                }
            }
        }
        // Healthy or transient states carry no badge: only stopped, paused and errors are called out.
        State::Starting | State::Saving | State::Saved => {}
    }
    px
}

fn source_label(name: &str) -> String {
    match name {
        "Game" | "Chat" | "Mic" => crate::lang::t(&format!("src.{name}")),
        other => other.to_string(),
    }
}

fn state_of(shared: &session::Shared) -> (State, String) {
    let now = std::time::Instant::now();
    if let Some((at, ok)) = *shared.flash.lock().unwrap() {
        if now.duration_since(at) < std::time::Duration::from_secs(4) {
            return if ok {
                (State::Saved, crate::lang::t("tip.saved"))
            } else {
                (State::SaveFailed, crate::lang::t("tip.save_failed"))
            };
        }
    }
    if let Some(until) = *shared.post_roll_until.lock().unwrap() {
        if until > now {
            let left = until.duration_since(now).as_secs() + 1;
            return (State::Saving, crate::lang::f("tip.postroll", &[("n", &left)]));
        }
    }
    if shared.finalizing.load(Ordering::Relaxed) {
        return (State::Saving, crate::lang::t("tip.finalizing"));
    }
    if let Some(secs) = crate::longrec::elapsed_secs(shared) {
        return (State::LongRec, crate::lang::f("tip.longrec", &[("t", &clock(secs))]));
    }
    if shared.idle.load(Ordering::Relaxed) {
        return (State::Idle, crate::lang::t("tip.idle"));
    }
    if !shared.recording.load(Ordering::Relaxed) {
        let err = shared.last_error.lock().unwrap().clone();
        if err.is_empty() && shared.restarts.load(Ordering::Relaxed) == 0 && shared.started.elapsed().as_secs() < 20 {
            return (State::Starting, crate::lang::t("tip.starting"));
        }
        let msg = if err.is_empty() { crate::lang::t("word.stopped") } else { err.chars().take(90).collect() };
        return (State::Stopped, crate::lang::f("tip.stopped", &[("msg", &msg)]));
    }
    let down: Vec<String> = shared
        .sinks
        .iter()
        .filter(|s| s.faulty())
        .map(|s| source_label(s.name))
        .collect();
    if !down.is_empty() {
        return (State::Problem, crate::lang::f("tip.problem", &[("list", &down.join(", "))]));
    }
    let cfg = shared.cfg.lock().unwrap();
    let game = shared.active_game.lock().unwrap().as_ref().map(|g| format!(" · {}", g.name)).unwrap_or_default();
    (
        State::Recording,
        crate::lang::f("tip.recording", &[("cur", &session::buffered_seconds().min(cfg.buffer_seconds)), ("max", &cfg.buffer_seconds), ("game", &game)]),
    )
}

/// What the health watcher sees each tick. `Neutral` (starting, waiting for a game) neither alarms nor re-arms.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Health {
    Good,
    Neutral,
    /// Recording, but these sources are down.
    Warn(String),
    /// Not recording although it should; the text is the last error (may be empty).
    Down(String),
}

/// Raises each problem once, and only after it has lasted `GRACE` (a restart or a device switch heals in seconds).
/// Recovery re-arms it, so the next outage is reported again.
struct HealthWatch {
    since: Option<(Health, std::time::Instant)>,
    alerted: Option<Health>,
}

impl HealthWatch {
    const GRACE: std::time::Duration = std::time::Duration::from_secs(15);

    fn new() -> Self {
        Self { since: None, alerted: None }
    }

    /// Returns the problem to announce, if this tick is the one that crosses the grace period.
    fn feed(&mut self, h: Health, now: std::time::Instant) -> Option<Health> {
        match h {
            Health::Good => {
                self.since = None;
                self.alerted = None;
                None
            }
            Health::Neutral => {
                self.since = None;
                None
            }
            bad => {
                let started = match &self.since {
                    Some((prev, t)) if *prev == bad => *t,
                    _ => {
                        self.since = Some((bad.clone(), now));
                        now
                    }
                };
                if now.duration_since(started) >= Self::GRACE && self.alerted.as_ref() != Some(&bad) {
                    self.alerted = Some(bad.clone());
                    Some(bad)
                } else {
                    None
                }
            }
        }
    }
}

fn health_of(shared: &session::Shared, state: State) -> Health {
    match state {
        State::Recording | State::Saving | State::Saved | State::LongRec => Health::Good,
        State::Idle | State::Starting | State::SaveFailed => Health::Neutral,
        State::Problem => {
            let down: Vec<String> =
                shared.sinks.iter().filter(|s| s.faulty()).map(|s| source_label(s.name)).collect();
            Health::Warn(down.join(", "))
        }
        State::Stopped => Health::Down(shared.last_error.lock().unwrap().chars().take(90).collect()),
    }
}

fn raise_health(shared: &session::Shared, h: &Health) {
    match h {
        Health::Warn(list) => {
            session::log(&format!("health: sources down: {list}"));
            shared.popup(Event::GameWarn, crate::lang::t("pop.health_warn"), crate::lang::f("pop.warn_nokey", &[("list", list)]));
        }
        Health::Down(err) => {
            session::log(&format!("health: recording stopped: {err}"));
            let sub = if err.is_empty() { crate::lang::t("pop.down_sub") } else { err.clone() };
            shared.popup(Event::RecordingDown, crate::lang::t("pop.health_down"), sub);
        }
        _ => {}
    }
}

/// A saved clip that cannot be read, has no picture or is only a blink is a failure the user should hear about now,
/// not when they open it a week later.
fn clip_problem(path: &std::path::Path) -> Option<String> {
    match ml_core::probe::probe(path) {
        Err(e) => Some(format!("cannot be read: {e}")),
        Ok(i) if i.width == 0 || i.height == 0 => Some("has no video".into()),
        Ok(i) if i.duration < 2.0 => Some(format!("is only {:.1} s long", i.duration)),
        Ok(_) => None,
    }
}

fn clock(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// Asks a running editor window to close (WM_CLOSE through taskkill, not a kill: it saves its state).
fn close_editor() {
    let mut c = Command::new("taskkill");
    c.args(["/IM", "buffer.exe"]);
    ml_core::export::hide_window(&mut c);
    let _ = c.spawn();
}

/// Opens the app on one of its pages (`--view=<name>`; a running window switches to it).
pub(crate) fn open_app_view(view: &str) {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let _ = Command::new(dir.join("buffer.exe")).arg(format!("--view={view}")).spawn();
    }
}

pub(crate) fn open_editor() {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let _ = Command::new(dir.join("buffer.exe")).spawn();
    }
}

/// Which sources use per-application capture, given the current settings and the detected game:
/// the chat app is split from the game, and "game only" records just the game's own sound.
pub fn apply_app_sources(shared: &session::Shared) {
    let cfg = shared.cfg.lock().unwrap().clone();
    let chat_app = cfg.chat_app.trim().to_lowercase();
    let chat_app = chat_app.trim_end_matches(".exe").to_string();
    shared.sinks[1].set_app(if chat_app.is_empty() { AppSource::None } else { AppSource::IncludeStem(chat_app.clone()) });
    let game = shared.active_game.lock().unwrap().clone();
    shared.sinks[0].set_app(match game {
        Some(g) if cfg.game_only => AppSource::IncludePid(g.pid),
        _ if !chat_app.is_empty() => AppSource::ExcludeStem(chat_app),
        _ => AppSource::None,
    });
}

/// Hotkey: wait the configured post-roll (so the moments right after the press are in the clip), then save.
/// Pressing again while waiting saves immediately.
fn hotkey_save(shared: &Arc<session::Shared>) {
    // nothing is being buffered (waiting for a game, or stopped): say so now instead of counting down to a failure
    let recording = shared.recording.load(Ordering::Relaxed);
    if !recording && crate::longrec::elapsed_secs(shared).is_none() && !session::has_buffer() {
        let why = if shared.idle.load(Ordering::Relaxed) { "pop.nothing_idle" } else { "pop.nothing_stopped" };
        shared.popup(Event::SaveFailed, crate::lang::t("pop.nothing"), crate::lang::t(why));
        return;
    }
    let roll = shared.cfg.lock().unwrap().post_roll_seconds;
    // no new footage is coming when recording has already stopped (the game closed), so save what is there now
    if roll == 0 || !recording {
        save_async(shared, false);
        return;
    }
    if shared.post_roll_active.swap(true, Ordering::Relaxed) {
        shared.post_roll_skip.store(true, Ordering::Relaxed);
        return;
    }
    shared.popup(Event::PostRoll { roll_ms: roll as u64 * 1000 }, crate::lang::f("pop.postroll", &[("n", &roll)]), crate::lang::t("pop.postroll_sub"));
    *shared.post_roll_until.lock().unwrap() = Some(std::time::Instant::now() + std::time::Duration::from_secs(roll as u64));
    let s = shared.clone();
    thread::spawn(move || {
        for _ in 0..roll * 4 {
            if s.post_roll_skip.load(Ordering::Relaxed) || s.quit.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(250));
        }
        // a second press ends the wait early; its notice may be long gone, so that save confirms itself
        let skipped = s.post_roll_skip.swap(false, Ordering::Relaxed);
        s.post_roll_active.store(false, Ordering::Relaxed);
        *s.post_roll_until.lock().unwrap() = None;
        save_async(&s, !skipped);
    });
}

/// `announced`: the key press already showed the "will be saved" notice, so success stays quiet (one notice per save).
fn save_async(shared: &Arc<session::Shared>, announced: bool) {
    let s = shared.clone();
    thread::spawn(move || match session::save(&s) {
        Ok(path) => {
            // the cue comes with the popup below: "saved" for a good clip, "failed" for a broken one
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(why) = clip_problem(&path) {
                session::log(&format!("saved clip {name} {why}"));
                s.popup(Event::SaveFailed, crate::lang::t("pop.bad_clip"), name);
            } else {
                // a countdown card still on screen turns into this one; otherwise it appears fresh
                let _ = announced;
                s.popup(Event::Saved, crate::lang::t("pop.saved"), name);
            }
        }
        Err(e) => {
            session::log(&format!("save failed: {e}"));
            *s.last_error.lock().unwrap() = format!("save failed: {e}");
            s.popup(Event::SaveFailed, crate::lang::t("pop.save_failed"), e);
        }
    });
}

/// "Buffer active" message shown when a game comes to the foreground.
/// Is it time to tell the user about a game that just came to the front? Yes once recording is up (normal popup), as
/// soon as there is a real error, or after `GRACE` without recording; not in the first moments, when a session that is
/// still starting would otherwise raise a false "recording isn't working".
fn announce_due(recording: bool, seen_for: std::time::Duration, has_error: bool) -> bool {
    const GRACE: std::time::Duration = std::time::Duration::from_secs(12);
    recording || has_error || seen_for >= GRACE
}

fn announce_game(shared: &session::Shared, game: &str) {
    let cfg = shared.cfg.lock().unwrap().clone();
    let down: Vec<&str> = shared.sinks.iter().filter(|s| s.faulty()).map(|s| s.name).collect();
    let len = if cfg.buffer_seconds >= 60 && cfg.buffer_seconds % 60 == 0 {
        format!("{} min", cfg.buffer_seconds / 60)
    } else {
        format!("{} s", cfg.buffer_seconds)
    };
    if !shared.recording.load(Ordering::Relaxed) {
        shared.popup(Event::RecordingDown, crate::lang::f("pop.down", &[("game", &game)]), crate::lang::t("pop.down_sub"));
    } else if down.is_empty() {
        shared.popup(Event::Game, crate::lang::t("pop.active"), if cfg.hotkey.trim().is_empty() { game.to_string() } else { crate::lang::f("pop.active_sub", &[("hk", &cfg.hotkey), ("len", &len), ("game", &game)]) });
    } else {
        shared.popup(Event::GameWarn, crate::lang::t("pop.warn"), if cfg.hotkey.trim().is_empty() { crate::lang::f("pop.warn_nokey", &[("list", &down.join(", "))]) } else { crate::lang::f("pop.warn_sub", &[("list", &down.join(", ")), ("hk", &cfg.hotkey)]) });
    }
}

/// Never returns while the recorder runs.
/// Registers the save, record-everything and output-switch shortcuts, replacing the previous ones when the
/// settings changed.
fn apply_hotkeys(shared: &session::Shared, current: &mut (String, String, String, String)) {
    let (save, rec, out, mic) = {
        let c = shared.cfg.lock().unwrap();
        (c.hotkey.trim().to_string(), c.record_hotkey.trim().to_string(), c.output_hotkey.trim().to_string(), c.mic_hotkey.trim().to_string())
    };
    if *current == (save.clone(), rec.clone(), out.clone(), mic.clone()) {
        return;
    }
    win::unregister_hotkey_id(1);
    win::unregister_hotkey_id(crate::input::RECORD as i32);
    win::unregister_hotkey_id(crate::input::OUTPUT as i32);
    win::unregister_hotkey_id(crate::input::MIC as i32);
    for (spec, id) in [(&out, crate::input::OUTPUT), (&mic, crate::input::MIC)] {
        if !spec.is_empty() {
            if let Err(e) = win::register_hotkey_id(spec, id as i32) {
                session::log(&e);
            }
        }
    }
    // a refused save shortcut is reported as `hotkey_ok: false` (the app asks for another one), not as `last_error`:
    // recording itself is fine, and the error would outlive a later shortcut that works
    let taken = match if save.is_empty() { Ok(()) } else { win::register_hotkey(&save) } {
        Err(e) => {
            session::log(&e);
            save.clone()
        }
        Ok(()) => String::new(),
    };
    *shared.hotkey_taken.lock().unwrap() = taken;
    if !rec.is_empty() {
        if let Err(e) = win::register_hotkey_id(&rec, crate::input::RECORD as i32) {
            session::log(&e);
        }
    }
    *current = (save, rec, out, mic);
}

/// The output-switch shortcut: next playback device (the popup, the EQ and a kept volume follow through
/// `check_output`, as for a switch made anywhere else).
fn switch_output(shared: &session::Shared) {
    let (chosen, hidden) = {
        let c = shared.cfg.lock().unwrap();
        (c.output_cycle.clone(), c.hidden_outputs.clone())
    };
    if let Err(e) = crate::endpoints::cycle_output(&chosen, &hidden) {
        session::log(&format!("output switch: {e}"));
    }
    check_output(shared);
}

/// The microphone shortcut: next capture device, named in a popup.
fn switch_mic(shared: &session::Shared) {
    match crate::endpoints::cycle_input() {
        Ok(name) => {
            if shared.cfg.lock().unwrap().output_popup {
                shared.popup_with_sound(Event::Game, crate::win::Sound::Game, crate::lang::t("pop.mic"), name);
            }
        }
        Err(e) => session::log(&format!("mic switch: {e}")),
    }
}

thread_local! {
    /// The default output last seen and its volume, to notice a switch made anywhere.
    static OUT_WATCH: std::cell::RefCell<Option<(String, f32)>> = const { std::cell::RefCell::new(None) };
}

/// Notices a change of the default output, whoever made it (our shortcut, the flyout, the app, Windows, a headset
/// plugged in): the EQ follows, "keep volume" carries the old device's volume over, the popup names the new one.
pub fn check_output(shared: &session::Shared) {
    let Some(d) = crate::mixer::default_dev() else { return };
    let (keep, popup) = {
        let c = shared.cfg.lock().unwrap();
        (c.keep_volume, c.output_popup)
    };
    let mut vol = d.volume();
    let prev = OUT_WATCH.with(|w| w.borrow().clone());
    if let Some((id, old)) = prev {
        if id != d.id {
            if keep {
                d.set_level(old);
                vol = old;
            }
            crate::eq::follow_output(shared);
            if popup {
                shared.popup_with_sound(Event::Game, crate::win::Sound::Game, crate::lang::t("pop.output"), d.name.clone());
            }
            audio_changed();
        }
    }
    OUT_WATCH.with(|w| *w.borrow_mut() = Some((d.id.clone(), vol)));
}

/// The audio icon's picture, in the logo's style: the speaker as seven lime scan-lines, the waves above the level
/// faded like the logo's tails. Muted: a grey speaker with the same red badge the recorder icon uses for errors.
fn sound_icon_rgba(waves: u8, muted: bool) -> Vec<u8> {
    const N: usize = 32;
    let mut px = vec![0u8; N * N * 4];
    let mut put = |x: i32, y: i32, c: [u8; 3], a: f32| {
        if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) && a > 0.0 {
            let i = (y as usize * N + x as usize) * 4;
            // over what is there (straight alpha)
            let (da, na) = (px[i + 3] as f32 / 255.0, a.min(1.0));
            let oa = na + da * (1.0 - na);
            for k in 0..3 {
                px[i + k] = ((c[k] as f32 * na + px[i + k] as f32 * da * (1.0 - na)) / oa.max(1e-6)).round() as u8;
            }
            px[i + 3] = (oa * 255.0).round() as u8;
        }
    };
    let lime = [0xc6, 0xf4, 0x32];
    let grey = [0x8b, 0x91, 0x9c];
    for (x0, y0, x1, y1, part) in crate::flyout::speaker_bars() {
        let (c, a) = match part {
            0 => (if muted { grey } else { lime }, 1.0),
            _ if muted => continue,
            n if n <= waves => (lime, 1.0),
            _ => (lime, 0.3),
        };
        // area coverage, for the half-unit wave edges
        for y in y0.floor() as i32..y1.ceil() as i32 {
            for x in x0.floor() as i32..x1.ceil() as i32 {
                let ox = ((x + 1) as f64).min(x1) - (x as f64).max(x0);
                let oy = ((y + 1) as f64).min(y1) - (y as f64).max(y0);
                put(x, y, c, (ox.clamp(0.0, 1.0) * oy.clamp(0.0, 1.0)) as f32 * a);
            }
        }
    }
    if muted {
        let (cx, cy) = (23.0f32, 23.0f32);
        for y in 0..N as i32 {
            for x in 0..N as i32 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                put(x, y, [17, 17, 17], (8.7 - d).clamp(0.0, 1.0));
                put(x, y, [0xff, 0x5d, 0x5d], (6.7 - d).clamp(0.0, 1.0));
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let on = (dx - dy).abs().min((dx + dy).abs()) / std::f32::consts::SQRT_2;
                if dx.abs() <= 3.6 && dy.abs() <= 3.6 {
                    put(x, y, [255, 255, 255], (1.6 - on).clamp(0.0, 1.0));
                }
            }
        }
    }
    px
}

thread_local! {
    /// The audio tray icon, and what its picture and tooltip show now (set again only when that changes).
    static SOUND: std::cell::RefCell<Option<(TrayIcon, (u8, bool), String)>> = const { std::cell::RefCell::new(None) };
    static WHEEL_REST: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Reads the default output's volume into the audio icon (picture and tooltip).
pub fn audio_changed() {
    let Some(d) = crate::mixer::default_dev() else { return };
    let (v, m) = (d.volume(), d.muted());
    let pic = (crate::flyout::waves_for(v), m);
    let n = (v * 100.0).round() as i32;
    let tip = if m { crate::lang::f("tip.sound_muted", &[("dev", &d.name)]) } else { crate::lang::f("tip.sound", &[("dev", &d.name), ("n", &n)]) };
    SOUND.with(|s| {
        if let Ok(mut g) = s.try_borrow_mut() {
            if let Some((icon, shown, shown_tip)) = g.as_mut() {
                if *shown != pic {
                    if let Ok(i) = Icon::from_rgba(sound_icon_rgba(pic.0, pic.1), 32, 32) {
                        let _ = icon.set_icon(Some(i));
                    }
                    *shown = pic;
                }
                if *shown_tip != tip {
                    let _ = icon.set_tooltip(Some(&tip));
                    *shown_tip = tip;
                }
            }
        }
    });
}

/// The mouse wheel over the audio icon: 5 % per notch on the default output.
pub fn on_wheel(delta: i32) {
    let rest = WHEEL_REST.with(|w| w.get()) + delta;
    WHEEL_REST.with(|w| w.set(rest % 120));
    let notches = rest / 120;
    if notches == 0 {
        return;
    }
    if let Some(d) = crate::mixer::default_dev() {
        d.set_volume(crate::mixer::step(d.volume(), notches as f32));
    }
    audio_changed();
}

fn rect_of(r: &tray_icon::Rect) -> windows_sys::Win32::Foundation::RECT {
    let (x, y) = (r.position.x.round() as i32, r.position.y.round() as i32);
    windows_sys::Win32::Foundation::RECT { left: x, top: y, right: x + r.size.width as i32, bottom: y + r.size.height as i32 }
}

thread_local! {
    /// The audio part of the menu lives on the tray thread, where tray-icon also calls our event handler: a right
    /// click refreshes it there, just before the menu opens (no polling of devices and registry in the background).
    static AUDIO: std::cell::RefCell<Option<(AudioMenu, Arc<session::Shared>)>> = const { std::cell::RefCell::new(None) };
}

pub fn run(shared: Arc<session::Shared>, _hotkey: String, popups: Receiver<Popup>) {
    let menu = Menu::new();
    let save_i = MenuItem::new(crate::lang::t("menu.save"), true, None);
    let rec_i = MenuItem::new(crate::lang::t("menu.record"), true, None);
    let open_i = MenuItem::new(crate::lang::t("menu.open"), true, None);
    let quit_i = MenuItem::new(crate::lang::t("menu.quit"), true, None);
    let mut audio = AudioMenu::new();
    audio.refresh(&shared);
    // without Equalizer APO the EQ items would only sit there greyed out: leave them out (installing APO means a
    // restart anyway, which brings them in)
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    let mut audio_items: Vec<&dyn tray_icon::menu::IsMenuItem> = vec![&audio.output];
    if ml_eq::apo::config_dir().is_some() {
        audio_items.push(&audio.eq_on);
        audio_items.push(&audio.preset);
    }
    audio_items.push(&audio.listen);
    let rec_items: [&dyn tray_icon::menu::IsMenuItem; 5] = [&save_i, &rec_i, &open_i, &sep2, &quit_i];
    // The audio items live in the audio icon's menu; with that icon switched off they join the recorder's menu
    // (a menu item may sit in several menus at once).
    let sound_menu = Menu::new();
    let _ = sound_menu.append_items(&audio_items);
    let _ = menu.append_items(&audio_items);
    let _ = menu.append(&sep1);
    let _ = menu.append_items(&rec_items);
    let menu_plain = Menu::new();
    let _ = menu_plain.append_items(&rec_items);
    let mut audio_tray = shared.cfg.lock().unwrap().audio_tray;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(if audio_tray { menu_plain.clone() } else { menu.clone() }))
        .with_menu_on_left_click(false)
        .with_tooltip("Buffer")
        .with_icon(icon(State::Starting))
        .build()
        .map_err(|e| session::log(&format!("tray unavailable: {e}")))
        .ok();
    // Created second, always (hidden when switched off): Windows remembers a pinned icon by exe and creation order.
    crate::flyout::init(shared.clone());
    let sound = TrayIconBuilder::new()
        .with_menu(Box::new(sound_menu.clone()))
        .with_menu_on_left_click(false)
        .with_tooltip("Buffer")
        .with_icon(Icon::from_rgba(sound_icon_rgba(3, false), 32, 32).expect("valid icon"))
        .build()
        .map_err(|e| session::log(&format!("audio tray icon unavailable: {e}")))
        .ok();
    let sound_id = sound.as_ref().map(|t| t.id().clone());
    if let Some(t) = &sound {
        let _ = t.set_visible(audio_tray);
    }
    SOUND.with(|s| *s.borrow_mut() = sound.map(|t| (t, (3, false), String::new())));
    audio_changed();
    AUDIO.with(|a| *a.borrow_mut() = Some((audio, shared.clone())));
    // tray-icon sends the click, then shows the menu, both inside its window procedure on this thread
    let (tray_tx, tray_rx) = std::sync::mpsc::channel::<TrayIconEvent>();
    TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
        let mine = sound_id.as_ref() == Some(ev.id());
        match &ev {
            TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, .. } => {
                AUDIO.with(|a| {
                    if let Ok(mut g) = a.try_borrow_mut() {
                        if let Some((m, s)) = g.as_mut() {
                            m.refresh(s);
                        }
                    }
                });
            }
            // the audio icon is handled right here: the flyout must open at once, not on the next tick
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } if mine => {
                crate::flyout::toggle(rect_of(rect));
            }
            TrayIconEvent::Enter { rect, .. } if mine => {
                crate::flyout::hover_icon(Some(rect_of(rect)));
                audio_changed();
            }
            TrayIconEvent::Move { rect, .. } if mine => crate::flyout::hover_icon(Some(rect_of(rect))),
            TrayIconEvent::Leave { .. } if mine => crate::flyout::hover_icon(None),
            _ => {}
        }
        if !mine {
            let _ = tray_tx.send(ev);
        }
    }));

    // keyboard shortcuts are (re)registered whenever the settings change; an empty one means "none"
    let mut registered = (String::new(), String::new(), String::new(), String::new());
    apply_hotkeys(&shared, &mut registered);
    crate::input::spawn(shared.clone(), unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() });

    overlay::bind_thread();
    let mut overlay = Overlay::new();
    let mut shown: Option<State> = None;
    let mut notified: HashSet<u32> = HashSet::new();
    let mut first_seen: std::collections::HashMap<u32, std::time::Instant> = std::collections::HashMap::new();
    // since when the active game's own window has been in front but no longer covers its monitor
    let mut windowed_since: Option<std::time::Instant> = None;
    let mut tick = 0u32;
    let mut health = HealthWatch::new();
    let my_pid = std::process::id();

    let hk_shared = shared.clone();
    let tick_shared = shared.clone();
    win::pump_messages(|ev| match ev {
        win::Pump::Hotkey(id) => {
            if id == crate::input::RECORD {
                crate::longrec::toggle(&hk_shared);
            } else if id == crate::input::OUTPUT {
                switch_output(&hk_shared);
            } else if id == crate::input::MIC {
                switch_mic(&hk_shared);
            } else {
                hk_shared.hotkey_hits.fetch_add(1, Ordering::Relaxed);
                hotkey_save(&hk_shared);
            }
        }
        win::Pump::Wake => {
            while let Ok(p) = popups.try_recv() {
                overlay.push(p);
            }
            // debug builds: `mix-flyout` opens the flyout from outside, for screenshots
            if crate::flyout::OPEN_REQUEST.swap(false, Ordering::Relaxed) {
                let rect = SOUND.with(|s| s.borrow().as_ref().and_then(|(t, ..)| t.rect()));
                if let Some(r) = rect {
                    crate::flyout::toggle(rect_of(&r));
                }
            }
        }
        win::Pump::Timer(id) if overlay.is_anim_timer(id) => overlay.frame(),
        win::Pump::Timer(_) => {
            tick += 1;
            apply_hotkeys(&tick_shared, &mut registered);
            while let Ok(p) = popups.try_recv() {
                overlay.push(p);
            }
            // Left click opens the app; the context menu belongs to the right button.
            while let Ok(ev) = tray_rx.try_recv() {
                // the icon can outlive the quit by a few seconds (the recording is finalised first): a click then must not reopen the app
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                    if !tick_shared.quit.load(Ordering::Relaxed) {
                        open_editor();
                    }
                }
            }
            // a game preset moves with the output when Windows (or another app) switches it
            crate::eq::expire_solo(&tick_shared);
            // a switch made anywhere: popup, kept volume, EQ (cheap: the default endpoint and its volume)
            check_output(&tick_shared);
            if tick % 4 == 0 {
                crate::eq::follow_output(&tick_shared);
                // the audio icon's picture follows a mute or volume change made elsewhere
                if audio_tray {
                    audio_changed();
                }
            }
            let want = tick_shared.cfg.lock().unwrap().audio_tray;
            if want != audio_tray {
                audio_tray = want;
                SOUND.with(|s| {
                    if let Some((t, ..)) = s.borrow().as_ref() {
                        let _ = t.set_visible(want);
                    }
                });
                if let Some(tray) = &tray {
                    tray.set_menu(Some(Box::new(if want { menu_plain.clone() } else { menu.clone() })));
                }
                if !want {
                    crate::flyout::hover_icon(None);
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                let ours = AUDIO.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(|(m, s)| m.handle(s, &ev.id))));
                if ours == Some(true) {
                } else if ev.id == *save_i.id() {
                    save_async(&tick_shared, false);
                } else if ev.id == *rec_i.id() {
                    crate::longrec::toggle(&tick_shared);
                } else if ev.id == *open_i.id() {
                    open_editor();
                } else if ev.id == *quit_i.id() {
                    tick_shared.quit.store(true, Ordering::Relaxed);
                    // "Quit Buffer" quits it all: the icon goes at once and an open editor window is asked to close
                    if let Some(tray) = &tray {
                        let _ = tray.set_visible(false);
                    }
                    SOUND.with(|s| {
                        if let Some((t, ..)) = s.borrow().as_ref() {
                            let _ = t.set_visible(false);
                        }
                    });
                    close_editor();
                }
            }
            if let Some(tray) = &tray {
                let (state, tip) = state_of(&tick_shared);
                let _ = rec_i.set_text(match crate::longrec::elapsed_secs(&tick_shared) {
                    Some(s) => crate::lang::f("menu.record_stop", &[("t", &clock(s))]),
                    None => crate::lang::t("menu.record"),
                });
                let _ = tray.set_tooltip(Some(tip));
                if shown != Some(state) {
                    let _ = tray.set_icon(Some(icon(state)));
                    shown = Some(state);
                }
                if !tick_shared.quit.load(Ordering::Relaxed) {
                    if let Some(h) = health.feed(health_of(&tick_shared, state), std::time::Instant::now()) {
                        raise_health(&tick_shared, &h);
                    }
                }
            }
            // Game detection: twice a second is plenty and costs next to nothing.
            let cfg = tick_shared.cfg.lock().unwrap().clone();
            // 1) the game we know about has exited
            let gone = {
                let g = tick_shared.active_game.lock().unwrap();
                g.as_ref().is_some_and(|g| !win::pid_alive(g.pid))
            };
            if gone {
                if let Some(g) = tick_shared.active_game.lock().unwrap().as_ref() {
                    session::log(&format!("game ended: {} (pid {}) exited", g.name, g.pid));
                }
                windowed_since = None;
                *tick_shared.active_game.lock().unwrap() = None;
                apply_app_sources(&tick_shared);
                crate::eq::on_game(&tick_shared, None, None);
            }
            // 2) a new game came to the foreground / the known one moved to another monitor
            if let Some(fg) = win::foreground() {
                if fg.pid != my_pid {
                    let ignored = NEVER_GAMES.contains(&fg.stem.as_str()) || cfg.ignore_apps.iter().any(|a| a == &fg.stem);
                    let forced = cfg.extra_games.iter().any(|a| a == &fg.stem);
                    // A detected game whose own window is in front but no longer fullscreen (a chat app that showed a
                    // photo fullscreen once) stops being a game after a while. Alt-tab away does not count: then the
                    // game is not in front at all.
                    let is_known = tick_shared.active_game.lock().unwrap().as_ref().is_some_and(|g| g.pid == fg.pid);
                    if is_known && !forced && !fg.fullscreen {
                        let since = *windowed_since.get_or_insert_with(std::time::Instant::now);
                        if since.elapsed() >= std::time::Duration::from_secs(10) {
                            windowed_since = None;
                            let name = tick_shared.active_game.lock().unwrap().take().map(|g| g.name).unwrap_or_default();
                            session::log(&format!(
                                "game ended: {name} ({}) in front but not fullscreen for 10 s ({})",
                                fg.stem, fg.rect
                            ));
                            notified.remove(&fg.pid);
                            apply_app_sources(&tick_shared);
                            crate::eq::on_game(&tick_shared, None, None);
                        }
                    } else if is_known {
                        windowed_since = None;
                    }
                    if !ignored && (fg.fullscreen || forced) {
                        let known = tick_shared.active_game.lock().unwrap().clone();
                        let name = session::current_game_name(&cfg);
                        match known {
                            Some(g) if g.pid == fg.pid => {
                                if g.monitor != fg.monitor {
                                    tick_shared.active_game.lock().unwrap().as_mut().unwrap().monitor = fg.monitor;
                                    if cfg.follow_game_monitor
                                        && session::desired_monitor(&tick_shared, &cfg)
                                            != tick_shared.monitor_now.load(Ordering::Relaxed)
                                    {
                                        tick_shared.restart.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                            _ => {
                                session::log(&format!(
                                    "game detected: {name} ({}, pid {}) {} on monitor {} ({})",
                                    fg.stem,
                                    fg.pid,
                                    if forced { "added by hand" } else { "fullscreen" },
                                    fg.monitor,
                                    fg.rect
                                ));
                                windowed_since = None;
                                *tick_shared.active_game.lock().unwrap() =
                                    Some(ActiveGame { pid: fg.pid, name: name.clone(), monitor: fg.monitor });
                                apply_app_sources(&tick_shared);
                                crate::eq::on_game(&tick_shared, Some(&fg.stem), Some(&name));
                                if cfg.follow_game_monitor
                                    && session::desired_monitor(&tick_shared, &cfg)
                                        != tick_shared.monitor_now.load(Ordering::Relaxed)
                                    && tick_shared.recording.load(Ordering::Relaxed)
                                {
                                    tick_shared.restart.store(true, Ordering::Relaxed);
                                }
                                if !notified.contains(&fg.pid) {
                                    // the session needs a few seconds to come up after a game launch, a restart or an
                                    // update: wait for it (the normal "Buffer active" popup) before raising an alarm
                                    let seen = *first_seen.entry(fg.pid).or_insert_with(std::time::Instant::now);
                                    let recording = tick_shared.recording.load(Ordering::Relaxed);
                                    let has_error = !tick_shared.last_error.lock().unwrap().is_empty();
                                    if announce_due(recording, seen.elapsed(), has_error) {
                                        first_seen.remove(&fg.pid);
                                        notified.insert(fg.pid);
                                        if notified.len() > 256 {
                                            notified.clear();
                                        }
                                        announce_game(&tick_shared, &name);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = tick;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_false_alarm_while_the_session_is_still_starting() {
        use std::time::Duration;
        let s = Duration::from_secs;
        assert!(!announce_due(false, s(0), false), "just seen, not recording yet: wait");
        assert!(!announce_due(false, s(5), false), "still inside the grace period");
        assert!(announce_due(true, s(1), false), "recording is up: announce normally");
        assert!(announce_due(false, s(1), true), "a real error is announced at once");
        assert!(announce_due(false, s(12), false), "never came up: tell the user");
    }

    #[test]
    fn health_alerts_once_after_the_grace_period_and_rearms_on_recovery() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = HealthWatch::new();
        let down = || Health::Down("ffmpeg exited".into());
        assert_eq!(w.feed(down(), at(0)), None, "a fresh outage is not announced yet");
        assert_eq!(w.feed(down(), at(14)), None, "still inside the grace period");
        assert_eq!(w.feed(down(), at(15)), Some(down()), "lasted long enough: announce");
        assert_eq!(w.feed(down(), at(40)), None, "never twice for one outage");
        assert_eq!(w.feed(Health::Neutral, at(41)), None);
        assert_eq!(w.feed(down(), at(42)), None, "a restart attempt does not make it a new outage");
        assert_eq!(w.feed(down(), at(70)), None);
        assert_eq!(w.feed(Health::Good, at(71)), None);
        assert_eq!(w.feed(down(), at(80)), None);
        assert_eq!(w.feed(down(), at(95)), Some(down()), "a new outage after recovery is announced again");
    }

    #[test]
    fn a_short_blip_never_alerts() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut w = HealthWatch::new();
        let warn = || Health::Warn("Mic".into());
        assert_eq!(w.feed(warn(), t0), None);
        assert_eq!(w.feed(Health::Good, t0 + Duration::from_secs(5)), None);
        assert_eq!(w.feed(warn(), t0 + Duration::from_secs(20)), None, "the timer restarted after the recovery");
    }

    #[test]
    fn every_state_has_a_valid_icon() {
        for s in [State::Recording, State::Problem, State::Stopped, State::Idle, State::Starting, State::Saving, State::Saved, State::SaveFailed, State::LongRec] {
            assert_eq!(icon_rgba(s).len(), 32 * 32 * 4);
            let _ = icon(s);
        }
    }

    /// Dev helper: `SOUND_ICON_DUMP=<file.png> cargo test dump_sound_icons` writes the audio icon's pictures
    /// (0-3 waves, muted) side by side, 4x enlarged, on a dark and a light taskbar strip.
    #[test]
    fn dump_sound_icons() {
        let Ok(path) = std::env::var("SOUND_ICON_DUMP") else { return };
        let pics = [(0, false), (1, false), (2, false), (3, false), (3, true)];
        let (z, n) = (4usize, 32usize);
        let (w, h) = (pics.len() * n * z, 2 * n * z);
        let mut out = vec![0u8; w * h * 4];
        for (i, (wv, m)) in pics.iter().enumerate() {
            let px = sound_icon_rgba(*wv, *m);
            for strip in 0..2 {
                let bg: [f32; 3] = if strip == 0 { [32.0, 32.0, 32.0] } else { [238.0, 238.0, 238.0] };
                for y in 0..n * z {
                    for x in 0..n * z {
                        let s = &px[((y / z) * n + x / z) * 4..][..4];
                        let a = s[3] as f32 / 255.0;
                        let o = (((strip * n * z + y) * w) + i * n * z + x) * 4;
                        for k in 0..3 {
                            out[o + k] = (s[k] as f32 * a + bg[k] * (1.0 - a)).round() as u8;
                        }
                        out[o + 3] = 255;
                    }
                }
            }
        }
        std::fs::write(path, crate::win::png_rgba(w as u32, h as u32, &out)).unwrap();
    }

    /// Dev helper: `ICON_DUMP=<file> cargo test dump_icons` writes every state side by side as raw RGBA.
    #[test]
    fn dump_icons() {
        let Ok(path) = std::env::var("ICON_DUMP") else { return };
        let states = [State::Recording, State::Problem, State::Stopped, State::Idle, State::Starting, State::Saving, State::Saved, State::SaveFailed, State::LongRec];
        let mut out = Vec::new();
        for y in 0..32 {
            for s in states {
                let px = icon_rgba(s);
                out.extend_from_slice(&px[y * 32 * 4..(y + 1) * 32 * 4]);
            }
        }
        std::fs::write(path, out).unwrap();
    }
}
