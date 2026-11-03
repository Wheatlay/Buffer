use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Segment length of the rolling buffer, in seconds.
pub const SEGMENT_SECS: u32 = 2;
/// Loopback control port (localhost only). The single-instance mutex guards against two recorders.
pub const CONTROL_PORT: u16 = 47821;

/// A remembered audio endpoint. The ID is authoritative; the name lets us find the same physical
/// device again if Windows hands it a new ID after it was plugged into another port.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceRef {
    pub id: String,
    pub name: String,
}

/// One audio source (Game / Chat / Mic): a preferred device and an optional fallback.
/// Both empty means "choose automatically" (only meant for a brand-new, not yet configured install).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceCfg {
    pub primary: Option<DeviceRef>,
    pub secondary: Option<DeviceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where finished clips are written (same folder the editor scans).
    pub output_dir: String,
    /// Put every clip into a sub-folder named after the game.
    pub per_game_folders: bool,
    /// After the save hotkey, keep recording this many seconds and save then (0 = save immediately).
    pub post_roll_seconds: u32,
    /// Buffer only while a game is running (and record only that game's sound), instead of the whole screen.
    pub game_only: bool,
    /// Capture the monitor the game is on instead of always the configured one.
    pub follow_game_monitor: bool,
    /// Executable stem of a chat app (e.g. "discord") to split from the game when both share one output.
    pub chat_app: String,
    /// How many seconds a saved clip contains.
    pub buffer_seconds: u32,
    pub fps: u32,
    /// Output width in pixels (height keeps the aspect ratio). 0 = native resolution.
    pub width: u32,
    /// Legacy absolute bitrate; only used when `quality` is empty (a custom value set by hand).
    pub video_kbps: u32,
    /// "low" | "std" | "high": the bitrate is derived from resolution, fps and this level (ml-core::bitrate).
    pub quality: String,
    /// Which monitor to capture (0 = primary).
    pub monitor: u32,
    /// "auto" picks NVENC, then AMF, then QuickSync, then CPU. Or force one of h264_nvenc / h264_amf / h264_qsv / libx264.
    pub encoder: String,
    /// Shifts audio against video, in milliseconds (positive = audio later). Tune by ear if lips and
    /// voice drift apart on this machine; saved clips are not touched, only new recordings.
    pub audio_offset_ms: i32,
    /// Bindings are comma-separated: a keyboard combo ("Alt+Q"), a mouse button ("Mouse4", "Ctrl+Mouse5")
    /// and/or a gamepad chord ("Pad:View+RB"). Keyboard combos need a recorder restart; mouse and pad apply at once.
    pub hotkey: String,
    /// UI language code written by the app (en, pl, de, es, fr, uk, tr, ja, zh); used for tray and popup text.
    pub lang: String,
    /// App theme ("ink" | "oled"), written by the app; the popup card follows it.
    pub theme: String,
    /// Starts / stops a long recording (the whole session, not just the last minutes).
    pub record_hotkey: String,
    /// Makes the next playback device the Windows default (like EarTrumpet); empty = none.
    pub output_hotkey: String,
    /// Endpoint ids the output shortcut cycles through; empty = all of them (minus a separate chat device).
    pub output_cycle: Vec<String>,
    /// A second tray icon for sound: volume flyout on click, volume on the mouse wheel, the audio menu.
    pub audio_tray: bool,
    /// When the default output changes (from anywhere), the new one gets the volume the old one had.
    pub keep_volume: bool,
    /// Plays inaudible zeros on HDMI / DisplayPort outputs so the monitor never drops the audio link (keepawake.rs).
    pub wake_display_audio: bool,
    /// A popup naming the new default output whenever it changes (shortcut, flyout, Windows, plugging in).
    pub output_popup: bool,
    /// Makes the next microphone the Windows default (all roles); empty = none.
    pub mic_hotkey: String,
    /// Output endpoint ids left out of the flyout, the tray menu and the switch cycle (the SoundSwitcher page
    /// lists them folded away).
    pub hidden_outputs: Vec<String>,
    pub game: SourceCfg,
    pub chat: SourceCfg,
    pub mic: SourceCfg,
    /// Show the "Buffer active" popup when a game starts and when a clip is saved.
    pub popup: bool,
    /// Short sound cues: clip saved, save failed, long recording start / stop.
    pub sounds: bool,
    /// The extra, quiet cue when a game is detected (only used while `sounds` is on).
    pub game_sound: bool,
    /// Loudness of the cues: "low" (-6 dB), "normal" (the files as they are) or "high" (+8 dB).
    pub sound_level: String,
    /// Executable stems (lowercase) that are never treated as a game.
    pub ignore_apps: Vec<String>,
    /// Executable stems (lowercase) that are always treated as a game, even in a window.
    pub extra_games: Vec<String>,
    /// Executable stem (lowercase) -> display name used in file names, e.g. "cs2" -> "Counter-Strike-2".
    pub game_names: std::collections::BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            output_dir: default_output_dir(),
            per_game_folders: true,
            post_roll_seconds: 15,
            game_only: false,
            follow_game_monitor: true,
            chat_app: String::new(),
            buffer_seconds: 120,
            fps: 30,
            width: 1920,
            video_kbps: 14_000,
            quality: "std".into(),
            monitor: 0,
            encoder: "auto".into(),
            audio_offset_ms: 0,
            hotkey: "Alt+Q".into(),
            lang: "en".into(),
            theme: "ink".into(),
            record_hotkey: "Alt+R".into(),
            output_hotkey: String::new(),
            output_cycle: Vec::new(),
            audio_tray: true,
            keep_volume: false,
            wake_display_audio: true,
            output_popup: true,
            mic_hotkey: String::new(),
            hidden_outputs: Vec::new(),
            game: SourceCfg::default(),
            chat: SourceCfg::default(),
            mic: SourceCfg::default(),
            popup: true,
            sounds: true,
            game_sound: true,
            sound_level: "normal".into(),
            ignore_apps: Vec::new(),
            extra_games: Vec::new(),
            game_names: Default::default(),
        }
    }
}

pub fn default_output_dir() -> String {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Videos").join("Buffer").to_string_lossy().into_owned()
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Buffer")
}

pub fn local_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(ml_core::paths::LOCAL_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    data_dir().join("recorder.json")
}

pub fn buffer_dir() -> PathBuf {
    local_dir().join("buffer")
}

/// Written when the recorder is closed on purpose (tray, `quit` command), removed when it starts. The app's watchdog
/// brings back a recorder that vanished (crash, killed) but must leave one alone that was quit by the user.
pub fn quit_marker() -> PathBuf {
    local_dir().join("recorder.quit")
}

pub fn log_path() -> PathBuf {
    local_dir().join("recorder.log")
}

impl Config {
    /// Reads the config. A missing file yields defaults (and is written so it can be edited);
    /// an unreadable one keeps the previous behaviour of falling back to defaults too.
    pub fn load() -> Config {
        let path = config_path();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                migrate_quality(&mut v);
                serde_json::from_value(v).unwrap_or_default()
            }
            Err(_) => {
                let cfg = Config::default();
                let _ = std::fs::create_dir_all(data_dir());
                let _ = std::fs::write(&path, serde_json::to_vec_pretty(&cfg).unwrap_or_default());
                cfg
            }
        }
    }
}

/// Configs written before the quality levels existed stored an absolute bitrate. Keep their meaning:
/// 8 / 14 / 30 Mb/s become low / std / high, anything else stays a custom absolute value.
fn migrate_quality(v: &mut serde_json::Value) {
    let Some(obj) = v.as_object_mut() else { return };
    if obj.contains_key("quality") {
        return;
    }
    if let Some(kbps) = obj.get("video_kbps").and_then(|k| k.as_u64()) {
        let q = match kbps {
            8_000 => "low",
            14_000 => "std",
            30_000 => "high",
            _ => "",
        };
        obj.insert("quality".into(), serde_json::Value::String(q.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bitrates_map_to_levels() {
        for (kbps, want) in [(8000, "low"), (14000, "std"), (30000, "high"), (21000, "")] {
            let mut v = serde_json::json!({ "video_kbps": kbps });
            migrate_quality(&mut v);
            assert_eq!(v["quality"], want);
        }
        let mut v = serde_json::json!({ "video_kbps": 30000, "quality": "low" });
        migrate_quality(&mut v);
        assert_eq!(v["quality"], "low", "an explicit level is never overwritten");
    }
}
