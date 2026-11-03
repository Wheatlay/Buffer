//! Live EQ: the recorder is the only process that writes `eq.json` and APO's `buffer.txt`, so the tray and
//! the app never race. The app goes through the control commands below.

use crate::session::Shared;
use ml_eq::{EqState, Solo};
use serde_json::{json, Value};

/// Rewrites buffer.txt from the state at start, so an edit made while the recorder was down still applies.
pub fn init(shared: &Shared) {
    let st = shared.eq.lock().unwrap();
    shared.eq_on.store(st.enabled, std::sync::atomic::Ordering::Relaxed);
    match ml_eq::apply(&st, &shared.eq_live.lock().unwrap()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => crate::session::log(&format!("eq: cannot write buffer.txt: {e}")),
        _ => {}
    }
}

fn reply(r: std::io::Result<()>) -> Value {
    match r {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Pushes state + live overlays to APO without saving (solo, game presets).
fn reapply(shared: &Shared) -> Value {
    let st = shared.eq.lock().unwrap();
    reply(ml_eq::apply(&st, &shared.eq_live.lock().unwrap()))
}

/// Saves the state and pushes it to APO.
pub fn commit(shared: &Shared, st: &EqState) -> Value {
    shared.eq_on.store(st.enabled, std::sync::atomic::Ordering::Relaxed);
    reply(st.save().and_then(|_| ml_eq::apply(st, &shared.eq_live.lock().unwrap())))
}

/// Changes the state in place (tray, app) and commits it.
pub fn update(shared: &Shared, f: impl FnOnce(&mut EqState)) -> Value {
    let mut st = shared.eq.lock().unwrap();
    f(&mut st);
    commit(shared, &st)
}

/// A game came to the front (`Some(stem)`) or went away (`None`). Its preset is an overlay on the output in use:
/// never saved, so the output's own EQ is what comes back afterwards (even after a restart mid-game).
pub fn on_game(shared: &Shared, stem: Option<&str>, name: Option<&str>) {
    *shared.eq_game_stem.lock().unwrap() = stem.map(|s| s.to_ascii_lowercase());
    // remember the game, so the EQ page can list it with a preset choice and nobody has to type an exe
    if let (Some(stem), Some(name)) = (stem, name) {
        let stem = stem.to_ascii_lowercase();
        let news = {
            let st = shared.eq.lock().unwrap();
            st.recent_games.first() != Some(&stem) || st.seen_games.get(&stem).map(String::as_str) != Some(name)
        };
        if news {
            update(shared, |st| {
                st.note_game(&stem, name);
            });
        }
    }
    refresh_game(shared);
}

/// Works out the game overlay from the game in front and the saved game presets. Called when a game starts or
/// ends and whenever the state is saved, so choosing a preset for the running game takes effect at once.
pub fn refresh_game(shared: &Shared) {
    let stem = shared.eq_game_stem.lock().unwrap().clone();
    let overlay = stem.and_then(|stem| {
        let st = shared.eq.lock().unwrap();
        let preset = st.game_presets.get(&stem)?;
        let bands = st.preset_bands(preset)?;
        Some((crate::endpoints::default_output()?, bands))
    });
    let changed = {
        let mut live = shared.eq_live.lock().unwrap();
        let changed = live.game != overlay;
        live.game = overlay;
        changed
    };
    if changed {
        reapply(shared);
    }
}

/// Equalizer APO is installed. The registry and the disk are asked at most every five seconds: the status poll runs
/// about once a second and must not touch either each time.
pub fn apo_installed() -> bool {
    use std::{
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };
    static CACHE: OnceLock<Mutex<(Instant, bool)>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new((Instant::now() - Duration::from_secs(60), false)));
    let mut c = cache.lock().unwrap();
    if c.0.elapsed() > Duration::from_secs(5) {
        *c = (Instant::now(), ml_eq::apo::config_dir().is_some());
    }
    c.1
}

/// Drops a solo the page stopped refreshing (it re-sends it while a band is held), called from the tray tick.
pub fn expire_solo(shared: &Shared) {
    let stale = shared.eq_solo_at.lock().unwrap().is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(2500));
    if stale {
        *shared.eq_solo_at.lock().unwrap() = None;
        shared.eq_live.lock().unwrap().solo = None;
        reapply(shared);
    }
}

/// The default output changed while a game preset is on: the preset moves with it. Costs nothing without one.
pub fn follow_output(shared: &Shared) {
    let Some((dev, bands)) = shared.eq_live.lock().unwrap().game.clone() else { return };
    let Some(cur) = crate::endpoints::default_output() else { return };
    if cur != dev {
        shared.eq_live.lock().unwrap().game = Some((cur, bands));
        reapply(shared);
    }
}

/// `eq-get` | `eq-status` | `eq-set <state json>` | `eq-bypass on|off` | `eq-solo off|<endpoint id> <f> <q>` |
/// `eq-preset <endpoint id> <preset id>` | `eq-unhook` | `eq-outputs` | `eq-output <id>|next` |
/// `eq-listen <capture id> [on|off]`
pub fn command(shared: &Shared, cmd: &str, arg: &str) -> Value {
    match cmd {
        "eq-get" => {
            let live = shared.eq_live.lock().unwrap().game.clone();
            let game = live.as_ref().map(|(dev, _)| dev.clone());
            // the bands that are actually playing while a game preset is on, so the page can draw what is heard
            let game_bands = live.map(|(_, b)| b);
            // the game in front (exe stem), so the page can offer "add a preset for this game" without typing
            let stem = shared.eq_game_stem.lock().unwrap().clone();
            let names = shared.cfg.lock().unwrap().game_names.clone();
            json!({ "ok": true, "state": *shared.eq.lock().unwrap(), "game_on": game, "game_bands": game_bands, "game_stem": stem, "game_names": names })
        }
        "eq-status" => json!({ "ok": true, "apo": ml_eq::apo::status() }),
        "eq-set" => match serde_json::from_str::<EqState>(arg) {
            Ok(new) => {
                let r = update(shared, |st| {
                    // the list of seen games belongs to the recorder (a page with an older copy must neither
                    // drop a game seen meanwhile nor bring one back that was forgotten): see eq-game-add / -forget
                    let (seen, recent) = (std::mem::take(&mut st.seen_games), std::mem::take(&mut st.recent_games));
                    *st = new;
                    st.seen_games = seen;
                    st.recent_games = recent;
                });
                refresh_game(shared);
                r
            }
            Err(e) => json!({ "ok": false, "error": format!("bad eq state: {e}") }),
        },
        "eq-bypass" => update(shared, |st| st.enabled = arg != "on"),
        "eq-solo" => {
            let mut w = arg.split_whitespace();
            let solo = match (w.next(), w.next().and_then(|v| v.parse().ok()), w.next().and_then(|v| v.parse().ok())) {
                (Some(dev), Some(f), Some(q)) if dev != "off" => Some(Solo { device: dev.to_string(), f, q }),
                _ => None,
            };
            *shared.eq_solo_at.lock().unwrap() = solo.as_ref().map(|_| std::time::Instant::now());
            shared.eq_live.lock().unwrap().solo = solo;
            reapply(shared)
        }
        "eq-preset" => {
            let (dev, id) = arg.split_once(' ').unwrap_or((arg, ""));
            let mut ok = false;
            let r = update(shared, |st| ok = st.set_preset(dev, id.trim()));
            if ok { r } else { json!({ "ok": false, "error": "unknown preset" }) }
        }
        // `eq-game-add <exe stem>`: list a game by hand; `eq-game-forget <exe stem>`: drop it and its preset
        "eq-game-add" if !arg.trim().is_empty() => {
            let stem = arg.trim().trim_end_matches(".exe").to_ascii_lowercase();
            update(shared, |st| {
                let name = st.seen_games.get(&stem).cloned().unwrap_or_else(|| stem.clone());
                st.note_game(&stem, &name);
            })
        }
        "eq-game-forget" => {
            let stem = arg.trim().to_ascii_lowercase();
            let r = update(shared, |st| {
                st.seen_games.remove(&stem);
                st.recent_games.retain(|g| *g != stem);
                st.game_presets.remove(&stem);
            });
            refresh_game(shared);
            r
        }
        // debug builds only: pretend a game came to the front (`eq-testgame <stem>`) or went away (`eq-testgame off`)
        "eq-testgame" if cfg!(debug_assertions) => {
            if arg == "off" || arg.is_empty() {
                on_game(shared, None, None);
            } else {
                on_game(shared, Some(arg), Some(&arg.replace('-', " ")));
            }
            json!({ "ok": true })
        }
        // `eq-preset-reset <built-in id>`: back to the shipped bands (drops what the user edited in that preset)
        "eq-preset-reset" => {
            let mut ok = false;
            let r = update(shared, |st| ok = st.reset_preset(arg.trim()));
            refresh_game(shared);
            if ok { r } else { json!({ "ok": false, "error": "not a built-in preset" }) }
        }
        "eq-unhook" => reply(ml_eq::apo::unhook()),
        "eq-outputs" => json!({
            "ok": true,
            "outputs": crate::endpoints::outputs().into_iter().map(|e| json!({ "id": e.id, "name": e.name })).collect::<Vec<_>>(),
            "default": crate::endpoints::default_output(),
        }),
        "eq-output" => match if arg == "next" {
            let (chosen, hidden) = {
                let c = shared.cfg.lock().unwrap();
                (c.output_cycle.clone(), c.hidden_outputs.clone())
            };
            crate::endpoints::cycle_output(&chosen, &hidden).map(|_| ()).map_err(|e| e.unwrap_or_else(|| "nothing to switch to".into()))
        } else {
            crate::endpoints::set_default_output(arg)
        } {
            Ok(()) => {
                follow_output(shared);
                json!({ "ok": true, "default": crate::endpoints::default_output() })
            }
            Err(e) => json!({ "ok": false, "error": e }),
        },
        // `eq-listen <capture id> [on|off [output id]]`; the reply says where the mic plays ("target")
        "eq-listen" => {
            let mut w = arg.splitn(3, ' ');
            let (dev, on, target) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().filter(|t| !t.is_empty()));
            let reply_for = |dev: &str| {
                let target = crate::endpoints::listen_target(dev);
                json!({ "ok": true, "on": crate::endpoints::listening(dev), "target": target })
            };
            if on.is_empty() {
                reply_for(dev)
            } else {
                match crate::endpoints::set_listen(dev, on == "on", target) {
                    Ok(()) => reply_for(dev),
                    Err(e) => json!({ "ok": false, "error": e }),
                }
            }
        }
        _ => json!({ "ok": false, "error": "unknown eq command" }),
    }
}
