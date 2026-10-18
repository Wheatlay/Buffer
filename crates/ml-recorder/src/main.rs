#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod away;
mod config;
mod endpoints;
mod eq;
mod icons;
mod identify;
mod lang;
mod mixer;
mod appdev;
mod flyout;
mod input;
mod longrec;
mod control;
mod overlay;
mod perf;
mod pick;
mod fans;
mod rgb;
mod session;
mod tray;
mod win;

use audio::{spawn_capture, Kind, Sink};
use config::Config;
use session::{log, supervise, Shared};
use std::{
    process::Command,
    sync::{atomic::AtomicBool, atomic::AtomicU32, atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};
use wasapi::{DeviceEnumerator, Direction};

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "BufferRecorder";

fn attach_console() {
    // The release build is a GUI-subsystem exe (no console window at login); still let CLI
    // subcommands print when started from a terminal.
    #[cfg(not(debug_assertions))]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

fn list_devices() {
    let _ = wasapi::initialize_mta();
    let Ok(en) = DeviceEnumerator::new() else {
        println!("cannot enumerate audio devices");
        return;
    };
    for (label, dir) in [("playback (loopback sources)", Direction::Render), ("microphones", Direction::Capture)] {
        println!("[{label}]");
        if let Ok(coll) = en.get_device_collection(&dir) {
            for d in &coll {
                if let Ok(d) = d {
                    println!("  {}\n    id: {}", d.get_friendlyname().unwrap_or_default(), d.get_id().unwrap_or_default());
                }
            }
        }
    }
}

/// Machine-readable device list for the UI's settings page.
fn devices_json() {
    let _ = wasapi::initialize_mta();
    let mut render = Vec::new();
    let mut capture = Vec::new();
    if let Ok(en) = DeviceEnumerator::new() {
        for (dir, out) in [(Direction::Render, &mut render), (Direction::Capture, &mut capture)] {
            if let Ok(coll) = en.get_device_collection(&dir) {
                for d in &coll {
                    if let Ok(d) = d {
                        out.push(serde_json::json!({
                            "id": d.get_id().unwrap_or_default(),
                            "name": d.get_friendlyname().unwrap_or_default(),
                        }));
                    }
                }
            }
        }
    }
    let id_of = |dir: Direction, role: wasapi::Role| {
        DeviceEnumerator::new()
            .ok()
            .and_then(|en| en.get_default_device_for_role(&dir, &role).ok())
            .and_then(|d| d.get_id().ok())
            .unwrap_or_default()
    };
    println!(
        "{}",
        serde_json::json!({
            "render": render,
            "capture": capture,
            "default": {
                "render": id_of(Direction::Render, wasapi::Role::Console),
                "communications": id_of(Direction::Render, wasapi::Role::Communications),
                "capture": id_of(Direction::Capture, wasapi::Role::Console),
            },
        })
    );
}

/// Peak level of the given endpoints (ids or "@default:*" tokens), measured in parallel for ~350 ms.
fn levels_json(args: Vec<String>) {
    // optional first argument "--ms=N" sets the measuring window (default 350 ms)
    let ms: u64 = args.iter().find_map(|a| a.strip_prefix("--ms=").and_then(|v| v.parse().ok())).unwrap_or(350).clamp(50, 2000);
    let tokens: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--ms=")).collect();
    let handles: Vec<_> = tokens
        .into_iter()
        .map(|t| std::thread::spawn(move || (t.clone(), audio::measure_peak(&t, ms))))
        .collect();
    let mut map = serde_json::Map::new();
    for h in handles {
        if let Ok((t, p)) = h.join() {
            map.insert(t, serde_json::json!(p));
        }
    }
    println!("{}", serde_json::Value::Object(map));
}

/// Monitors in enumeration order (index = the "monitor" setting), with their resolution.
fn monitors_json() {
    // physical pixels like the capture sees them, not the scaled size (a 1080p screen at 125% is not 1536x864)
    win::make_dpi_aware();
    println!(
        "{}",
        serde_json::json!(win::monitors()
            .into_iter()
            .enumerate()
            .map(|(i, m)| serde_json::json!({
                "index": i, "width": m.width, "height": m.height, "primary": m.primary,
                "left": m.left, "top": m.top, "name": m.name,
            }))
            .collect::<Vec<_>>())
    );
}

fn autostart_enabled() -> bool {
    let mut cmd = Command::new("reg");
    cmd.args(["query", RUN_KEY, "/v", RUN_VALUE]);
    ml_core::export::hide_window(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// A panic in any thread (the release build has no console) would otherwise vanish without a trace.
fn install_panic_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log(&format!("PANIC in thread '{}': {info}", thread.name().unwrap_or("?")));
        default(info);
    }));
}

fn run() {
    install_panic_log();
    win::make_dpi_aware();
    if !win::claim_single_instance("Local\\BufferRecorder") {
        println!("recorder is already running");
        return;
    }
    win::raise_priority();
    let cfg = Config::load();
    lang::set(&cfg.lang);
    overlay::set_theme(&cfg.theme);
    let sinks = [
        Sink::new("Game", Kind::Loopback, "Game"),
        Sink::new("Chat", Kind::Loopback, "Chat"),
        Sink::new("Mic", Kind::Microphone, ""),
    ];
    sinks[0].set_cfg(cfg.game.clone());
    sinks[1].set_cfg(cfg.chat.clone());
    sinks[2].set_cfg(cfg.mic.clone());
    let (popup_tx, popup_rx) = std::sync::mpsc::channel();

    let shared = Arc::new(Shared {
        cfg: Mutex::new(cfg.clone()),
        sinks,
        quit: AtomicBool::new(false),
        restart: AtomicBool::new(false),
        recording: AtomicBool::new(false),
        restarts: AtomicU32::new(0),
        last_error: Mutex::new(String::new()),
        last_saved: Mutex::new(String::new()),
        saving: AtomicBool::new(false),
        started: Instant::now(),
        save_lock: Mutex::new(()),
        job: win::Job::new(),
        popups: Mutex::new(popup_tx),
        encoder: Mutex::new(String::new()),
        capture: Mutex::new("gfxcapture"),
        fast_failures: AtomicU32::new(0),
        active_game: Mutex::new(None),
        idle: AtomicBool::new(false),
        monitor_now: AtomicU32::new(0),
        post_roll_active: AtomicBool::new(false),
        post_roll_skip: AtomicBool::new(false),
        post_roll_until: Mutex::new(None),
        flash: Mutex::new(None),
        hotkey_hits: AtomicU32::new(0),
        hotkey_taken: Mutex::new(String::new()),
        kbps_now: AtomicU32::new(0),
        cpu_x10: AtomicU32::new(0),
        gpu_x10: std::sync::atomic::AtomicI32::new(-1),
        encoder_override: Mutex::new(None),
        long: Mutex::new(None),
        long_stopping: AtomicBool::new(false),
        finalizing: AtomicBool::new(false),
        restart_pending: AtomicBool::new(false),
        eq: Mutex::new(ml_eq::EqState::load()),
        eq_live: Mutex::new(ml_eq::Live::default()),
        eq_game_stem: Mutex::new(None),
        eq_on: AtomicBool::new(false),
        eq_solo_at: Mutex::new(None),
    });
    eq::init(&shared);
    rgb::init(&shared);
    fans::init();
    {
        let cfg = cfg.clone();
        std::thread::spawn(move || longrec::recover_orphans(&cfg));
    }
    tray::apply_app_sources(&shared);
    let _ = std::fs::remove_file(config::quit_marker());
    log(&format!("recorder starting (ffmpeg: {})", ml_core::tools::ffmpeg().display()));

    let stop_capture = Arc::new(AtomicBool::new(false));
    for s in &shared.sinks {
        spawn_capture(s.clone(), stop_capture.clone());
    }
    if let Err(e) = control::serve(shared.clone()) {
        log(&format!("control port unavailable: {e}"));
    }

    perf::start(shared.clone());

    // Tray icon + global hotkey share one message-loop thread.
    let ui_shared = shared.clone();
    let hotkey = cfg.hotkey.clone();
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || tray::run(ui_shared, hotkey, popup_rx))
        .expect("spawn tray thread");

    let sup = shared.clone();
    let handle = std::thread::spawn(move || supervise(sup));
    while !shared.quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(200));
    }
    // closing the recorder must not lose a long recording in progress
    let _ = longrec::stop(&shared);
    rgb::shutdown();
    fans::shutdown();
    stop_capture.store(true, Ordering::Relaxed);
    let _ = handle.join();
    // a closing on purpose: the app must not bring the recorder back (see config::quit_marker)
    let _ = std::fs::write(config::quit_marker(), "quit");
    log("recorder stopped");
}

fn autostart(install: bool) {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut cmd = Command::new("reg");
    if install {
        // drop the pre-release value name if it is still there
        let mut old = Command::new("reg");
        old.args(["delete", RUN_KEY, "/v", "MomentsLiteRecorder", "/f"]);
        ml_core::export::hide_window(&mut old);
        let _ = old.output();
        // a debug build is a console program: started at login it would open a terminal window, so it starts through a
        // headless console host (the release build has no console and starts directly)
        let line = if cfg!(debug_assertions) { format!("conhost.exe --headless \"{exe}\" run") } else { format!("\"{exe}\" run") };
        cmd.args(["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &line, "/f"]);
    } else {
        cmd.args(["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]);
    }
    ml_core::export::hide_window(&mut cmd);
    match cmd.output() {
        Ok(o) if o.status.success() => println!("autostart {}", if install { "enabled" } else { "disabled" }),
        Ok(o) => println!("autostart change failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => println!("autostart change failed: {e}"),
    }
}

fn main() {
    ml_core::paths::migrate_legacy_dirs();
    let arg = std::env::args().nth(1).unwrap_or_else(|| "run".into());
    match arg.as_str() {
        "run" => run(),
        other => {
            attach_console();
            match other {
                "devices" => list_devices(),
                "devices-json" => devices_json(),
                "monitors-json" => monitors_json(),
                "identify" => identify::run(),
                "import-icons" => icons::run(std::env::args().skip(2).collect()),
                "levels" => levels_json(std::env::args().skip(2).collect()),
                // RGB: one elevated, all-or-nothing step (one Windows permission prompt), see ml-rgb install.rs
                "rgb-install" => println!("{}", ml_rgb::install::install(&config::local_dir().join("rgb")).map_or_else(|e| format!("failed: {e}"), |_| "ok".into())),
                "rgb-uninstall" => println!("{}", ml_rgb::install::uninstall().map_or_else(|e| format!("failed: {e}"), |_| "ok".into())),
                "autostart-status" => println!("{}", if autostart_enabled() { "enabled" } else { "disabled" }),
                "status" | "save" | "reload" | "quit" | "demo" | "record" | "record-start" | "record-stop" => match control::send(other) {
                    Ok(r) => println!("{r}"),
                    Err(e) => println!("{e}"),
                },
                // uninstall: takes Buffer's line out of Equalizer APO's config (APO itself stays)
                "eq-unhook" => {
                    if let Err(e) = ml_eq::apo::unhook() {
                        println!("{e}");
                    }
                }
                "install-autostart" => autostart(true),
                "uninstall-autostart" => autostart(false),
                _ => println!("usage: buffer-recorder [run|devices|status|save|record|reload|quit|rgb-install|rgb-uninstall|install-autostart|uninstall-autostart]"),
            }
        }
    }
}
