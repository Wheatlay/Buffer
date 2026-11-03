//! Tiny line-based control protocol on 127.0.0.1 so the UI (and `buffer-recorder save`) can talk to the
//! running recorder. Commands: status | save | record | record-start | record-stop | reload | quit | eq-* (eq.rs) | rgb-* (rgb.rs).
//! One JSON line is returned per command.

use crate::config::{Config, CONTROL_PORT};
use crate::session::{buffered_seconds, save, Shared};
use serde_json::json;
use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{atomic::Ordering, Arc},
    thread,
};

/// What the app's icon rail marks: `warn` = a dot (the page has something to fix), `off` = greyed (the module's
/// own part is not installed). Re-read at most every 5 s: the UI polls status often and these touch the registry,
/// the device list and schtasks.
fn nav_status(shared: &Shared) -> serde_json::Value {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<Option<(Instant, serde_json::Value)>>> = OnceLock::new();
    let mut c = CACHE.get_or_init(|| Mutex::new(None)).lock().unwrap();
    if let Some((_, v)) = c.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(5)) {
        return v.clone();
    }
    let (hotkey, chosen, hidden) = {
        let c = shared.cfg.lock().unwrap();
        (c.output_hotkey.clone(), c.output_cycle.clone(), c.hidden_outputs.clone())
    };
    let cur = crate::endpoints::default_output().unwrap_or_default();
    let apo = ml_eq::apo::status();
    // EQ: Equalizer APO is there but not on the output that plays now (a replaced endpoint id, a new device)
    let eq_warn = apo.installed && !cur.is_empty() && !ml_eq::apo::attached(&apo, &cur);
    // SoundSwitcher: a shortcut is set but has nowhere to go
    let snd_warn = !hotkey.is_empty() && {
        let list = crate::endpoints::cycle_list(crate::endpoints::outputs(), &cur, crate::endpoints::default_comms_output().as_deref(), &chosen, &hidden);
        crate::endpoints::next_in_cycle(&list, &cur).is_none()
    };
    let v = json!({
        "warn": { "eq": eq_warn, "snd": snd_warn },
        "off": { "eq": !apo.installed, "rgb": !ml_rgb::install::installed(), "fans": !ml_fans::install::installed() },
    });
    *c = Some((Instant::now(), v.clone()));
    v
}

pub fn status_json(shared: &Shared) -> serde_json::Value {
    let cfg = shared.cfg.lock().unwrap().clone();
    let sources: Vec<_> = shared
        .sinks
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "device": *s.device_name.lock().unwrap(),
                "role": *s.role.lock().unwrap(),
                "live": s.live.load(Ordering::Relaxed),
                "fault": s.faulty(),
                "level": s.level.load(Ordering::Relaxed) as f64 / 1000.0,
                "status": *s.status.lock().unwrap(),
            })
        })
        .collect();
    let gpu = shared.gpu_x10.load(Ordering::Relaxed);
    let gpu_pct = if gpu < 0 { serde_json::Value::Null } else { json!(gpu as f64 / 10.0) };
    // false while Windows refuses the save shortcut (another program holds it). Compared with the shortcut in the
    // settings, so a new one is not reported as taken in the half second before the tray thread registers it.
    let hotkey_ok = {
        let t = shared.hotkey_taken.lock().unwrap();
        t.is_empty() || *t != cfg.hotkey.trim()
    };
    // a pressed save waits out its post-roll: how long until it saves (a second press saves at once)
    let post_roll_left_ms = shared
        .post_roll_until
        .lock()
        .unwrap()
        .map(|u| u.saturating_duration_since(std::time::Instant::now()).as_millis() as u64)
        .unwrap_or(0);
    json!({
        "recording": shared.recording.load(Ordering::Relaxed),
        "idle": shared.idle.load(Ordering::Relaxed),
        "hotkey_hits": shared.hotkey_hits.load(Ordering::Relaxed),
        "kbps": shared.kbps_now.load(Ordering::Relaxed),
        "cpu_pct": shared.cpu_x10.load(Ordering::Relaxed) as f64 / 10.0,
        "gpu_pct": gpu_pct,
        "game": shared.active_game.lock().unwrap().as_ref().map(|g| g.name.clone()),
        "post_roll_s": cfg.post_roll_seconds,
        "monitor": shared.monitor_now.load(Ordering::Relaxed),
        "uptime_s": shared.started.elapsed().as_secs(),
        "restarts": shared.restarts.load(Ordering::Relaxed),
        "last_error": *shared.last_error.lock().unwrap(),
        "last_saved": *shared.last_saved.lock().unwrap(),
        "saving": shared.saving.load(Ordering::Relaxed) || shared.post_roll_active.load(Ordering::Relaxed),
        "buffer_s": buffered_seconds().min(cfg.buffer_seconds),
        "buffer_target_s": cfg.buffer_seconds,
        "hotkey_ok": hotkey_ok,
        "post_roll_left_ms": post_roll_left_ms,
        "hotkey": cfg.hotkey,
        "output_dir": cfg.output_dir,
        "ffmpeg": ml_core::tools::ffmpeg().to_string_lossy(),
        "encoder": *shared.encoder.lock().unwrap(),
        "capture": *shared.capture.lock().unwrap(),
        "sources": sources,
        "record_hotkey": cfg.record_hotkey,
        // what the top bar's EQ pill shows: the EQ is on, and Equalizer APO is there to do the work
        "eq_on": shared.eq_on.load(Ordering::Relaxed),
        "eq_apo": crate::eq::apo_installed(),
        "nav": nav_status(shared),
        "long": {
            "active": crate::longrec::active(shared),
            "elapsed_s": crate::longrec::elapsed_secs(shared),
            "mb": crate::longrec::bytes(shared) / 1_000_000,
            "finalizing": shared.finalizing.load(Ordering::Relaxed),
        },
    })
}

/// Whether a settings change touches the capture itself. Fields the recorder reads live on every use are left out
/// (`apply_hotkeys` and input.rs re-read the shortcuts, the tray reads `audio_tray`, popups read their options).
fn needs_restart(old: &Config, new: &Config) -> bool {
    let live = |c: &Config| {
        let mut c = c.clone();
        c.hotkey.clear();
        c.record_hotkey.clear();
        c.output_hotkey.clear();
        c.output_cycle.clear();
        c.audio_tray = true;
        c.keep_volume = false;
        c.output_popup = true;
        c.mic_hotkey.clear();
        c.hidden_outputs.clear();
        c.lang.clear();
        c.theme.clear();
        c.popup = true;
        c.sounds = true;
        c.game_sound = true;
        c.sound_level.clear();
        serde_json::to_value(c).unwrap_or_default()
    };
    live(old) != live(new)
}

fn handle(shared: &Arc<Shared>, stream: TcpStream) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut out = stream;
    for line in BufReader::new(read_half).lines().map_while(Result::ok) {
        let (cmd, arg) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let reply = match line.trim() {
            _ if cmd.starts_with("eq-") => crate::eq::command(shared, cmd, arg),
            _ if cmd.starts_with("rgb-") => crate::rgb::command(cmd, arg),
            _ if cmd.starts_with("fans-") => crate::fans::command(cmd, arg),
            _ if cmd.starts_with("mix-") => crate::mixer::command(shared, cmd, arg),
            "status" => status_json(shared),
            "save" => match save(shared) {
                Ok(p) => {
                    shared.sound(crate::win::Sound::Saved);
                    json!({ "ok": true, "path": p.to_string_lossy() })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "record" | "record-start" | "record-stop" => {
                let on = crate::longrec::active(shared);
                match line.trim() {
                    "record-start" if on => {}
                    "record-stop" if !on => {}
                    _ => crate::longrec::toggle(shared),
                }
                json!({ "ok": true })
            }
            "reload" => {
                let cfg = Config::load();
                crate::lang::set(&cfg.lang);
                crate::overlay::set_theme(&cfg.theme);
                // choosing an encoder by hand overrides an earlier automatic fallback
                if cfg.encoder != shared.cfg.lock().unwrap().encoder {
                    *shared.encoder_override.lock().unwrap() = None;
                }
                shared.sinks[0].set_cfg(cfg.game.clone());
                shared.sinks[1].set_cfg(cfg.chat.clone());
                shared.sinks[2].set_cfg(cfg.mic.clone());
                let restart = needs_restart(&shared.cfg.lock().unwrap(), &cfg);
                *shared.cfg.lock().unwrap() = cfg;
                crate::tray::apply_app_sources(shared);
                // shortcuts, sound/tray options, language and theme are read live: no reason to cut the buffer
                if restart {
                    shared.restart.store(true, Ordering::Relaxed);
                }
                json!({ "ok": true })
            }
            // Shows every popup the recorder can raise, one every 7 s, for a visual check of wording and colours.
            "demo" => {
                let s = shared.clone();
                thread::spawn(move || {
                    use crate::lang::{f, t};
                    use crate::overlay::Event as E;
                    let cards = [
                        (E::Game, t("pop.active"), f("pop.active_sub", &[("hk", &"Ctrl+F9"), ("len", &"2 min"), ("game", &"Demo Game")])),
                        (E::GameWarn, t("pop.warn"), f("pop.warn_sub", &[("list", &"Mic"), ("hk", &"Ctrl+F9")])),
                        (E::PostRoll { roll_ms: 3000 }, f("pop.postroll", &[("n", &3)]), t("pop.postroll_sub")),
                        (E::Saved, t("pop.saved"), "Demo Game__2026-10-15_21-00-00.mp4".into()),
                        (E::SaveFailed, t("pop.save_failed"), t("err.read")),
                        (E::SaveFailed, t("pop.bad_clip"), "Demo Game__2026-10-15_21-00-00.mp4".into()),
                        (E::SaveFailed, t("pop.nothing"), t("pop.nothing_idle")),
                        (E::RecordingDown, f("pop.down", &[("game", &"Demo Game")]), t("pop.down_sub")),
                        (E::GameWarn, t("pop.health_warn"), f("pop.warn_nokey", &[("list", &"Mic, Chat")])),
                        (E::RecordingDown, t("pop.health_down"), "ffmpeg exited (exit code: 1)".into()),
                        (E::Game, t("lr.started"), f("lr.started_sub", &[("hk", &"Ctrl+F10")])),
                        (E::Saved, t("lr.saved"), "Demo Game__long.mp4".into()),
                        (E::SaveFailed, t("lr.start_failed"), t("err.space")),
                    ];
                    // a long recording has its own start / stop cues, the rest follows the popup's event
                    let (long_start, long_stop) = (t("lr.started"), t("lr.saved"));
                    for (event, title, sub) in cards {
                        if title == long_start {
                            s.popup_with_sound(event, crate::win::Sound::RecStart, title, sub);
                        } else if title == long_stop {
                            s.popup_with_sound(event, crate::win::Sound::RecStop, title, sub);
                        } else {
                            s.popup(event, title, sub);
                        }
                        thread::sleep(std::time::Duration::from_secs(7));
                    }
                });
                json!({ "ok": true })
            }
            // the settings play a sample after the volume was changed
            "test-sound" => {
                shared.sound(crate::win::Sound::Saved);
                json!({ "ok": true })
            }
            "quit" => {
                shared.quit.store(true, Ordering::Relaxed);
                json!({ "ok": true })
            }
            other => json!({ "ok": false, "error": format!("unknown command '{other}'") }),
        };
        if writeln!(out, "{reply}").is_err() {
            return;
        }
    }
}

pub fn serve(shared: Arc<Shared>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", CONTROL_PORT))?;
    thread::Builder::new().name("control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let shared = shared.clone();
            thread::spawn(move || handle(&shared, stream));
        }
    })?;
    Ok(())
}

/// Client side: send one command to a running recorder and return its JSON reply.
pub fn send(cmd: &str) -> Result<String, String> {
    let mut s = TcpStream::connect(("127.0.0.1", CONTROL_PORT)).map_err(|_| "recorder is not running".to_string())?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(60))).ok();
    writeln!(s, "{cmd}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn only_capture_settings_restart_the_session() {
        let old = Config::default();
        let mut c = old.clone();
        c.output_cycle = vec!["{0.0.0.00000000}.{abc}".into()];
        c.audio_tray = false;
        c.output_hotkey = "Ctrl+PgDn".into();
        assert!(!super::needs_restart(&old, &c), "mixer and shortcut options are read live");
        c.fps = 60;
        assert!(super::needs_restart(&old, &c));
    }
}
