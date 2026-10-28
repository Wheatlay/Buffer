//! Update notice: the recorder runs all day in the tray while the app window may never be opened, so it looks at the
//! release feed itself and shows a popup card when a newer Buffer exists. Installing stays in the app (its top-bar
//! chip); this only tells the user to open it. At most once a day per version, never while a game is in front.

use std::{
    os::windows::process::CommandExt,
    process::Command,
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::overlay::{Event, Popup};
use crate::session::Shared;

/// The same feed the app's updater reads.
const FEED: &str = "https://github.com/Wheatlay/Buffer/releases/latest/download/latest.json";
const FIRST_CHECK: Duration = Duration::from_secs(90);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
const NO_WINDOW: u32 = 0x0800_0000;

pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || run(&shared))
        .expect("spawn update thread");
}

fn run(shared: &Shared) {
    let mut wait = FIRST_CHECK;
    loop {
        if !sleep(shared, wait) {
            return;
        }
        wait = CHECK_EVERY;
        let Some(v) = latest() else { continue };
        if !newer(&v, env!("CARGO_PKG_VERSION")) {
            continue;
        }
        crate::session::log(&format!("update available: {v}"));
        // a game in front: try again every minute until it is gone (the card would cover the game)
        while shared.active_game.lock().unwrap().is_some() {
            if !sleep(shared, Duration::from_secs(60)) {
                return;
            }
        }
        if notify_due(&v) {
            let title = crate::lang::f("pop.update", &[("v", &v)]);
            let _ = shared.popups.lock().unwrap().send(Popup { event: Event::Update, title, sub: crate::lang::t("pop.update_sub") });
            crate::overlay::wake();
        }
    }
}

/// Sleeps in short steps so quitting is not held up; false once the recorder is quitting.
fn sleep(shared: &Shared, d: Duration) -> bool {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        if shared.quit.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    !shared.quit.load(Ordering::Relaxed)
}

/// The version in the release feed, or None (offline, GitHub down, a feed we cannot read).
fn latest() -> Option<String> {
    let out = Command::new("curl.exe")
        .args(["-sL", "--fail", "--max-time", "20", FEED])
        .creation_flags(NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(v.get("version")?.as_str()?.trim_start_matches('v').to_string())
}

fn parts(v: &str) -> Vec<u32> {
    v.split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

/// `a` is a higher version than `b` (the updater never goes down, so neither does the notice).
fn newer(a: &str, b: &str) -> bool {
    parts(a) > parts(b)
}

/// True when this version was not announced today; remembers the announcement in `update-notice.txt`.
fn notify_due(v: &str) -> bool {
    let day = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0);
    let file = crate::config::local_dir().join("update-notice.txt");
    let stamp = format!("{v} {day}");
    if std::fs::read_to_string(&file).is_ok_and(|s| s.trim() == stamp) {
        return false;
    }
    let _ = std::fs::write(&file, stamp);
    true
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_versions_by_number() {
        assert!(newer("0.4.13", "0.4.12"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.4.12", "0.4.12"));
        assert!(!newer("0.4.11", "0.4.12"));
    }
}
