#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ml_core::{
    export::{self, hide_window, ExportOptions, ExportResult},
    probe::{self, MediaInfo},
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Condvar, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};

mod eq;
mod fans;
mod rgb;

const VIDEO_EXT: [&str; 3] = ["mp4", "mkv", "mov"];
/// Folders the app writes to or that only hold derived files.
const SKIP_DIRS: [&str; 2] = ["Exported Clips", "Shared Clips"];
const RECORDER_PORT: u16 = 47821;

// ───────────────────────── library ─────────────────────────

#[derive(Serialize)]
struct ClipEntry {
    path: String,
    name: String,
    group: String,
    size: u64,
    modified: u64,
}

fn collect(root: &Path, dir: &Path, depth: u32, recurse: bool, out: &mut Vec<ClipEntry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            let skip = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(n)));
            if recurse && depth < 3 && !skip {
                collect(root, &p, depth + 1, recurse, out);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| VIDEO_EXT.iter().any(|v| v.eq_ignore_ascii_case(x)))
        {
            let group = p
                .parent()
                .and_then(|d| d.strip_prefix(root).ok())
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(ClipEntry {
                path: p.to_string_lossy().into_owned(),
                name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                group,
                size: meta.len(),
                modified: meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs()),
            });
        }
    }
}

#[tauri::command]
async fn list_clips(dirs: Vec<String>, recurse: bool) -> Result<Vec<ClipEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut v = Vec::new();
        let mut missing = Vec::new();
        for d in &dirs {
            let root = PathBuf::from(d);
            if root.is_dir() {
                collect(&root, &root, 0, recurse, &mut v);
            } else {
                missing.push(d.clone());
            }
        }
        if v.is_empty() && !missing.is_empty() {
            return Err(format!("Folder not found: {}", missing.join(", ")));
        }
        v.sort_by(|a, b| b.modified.cmp(&a.modified));
        Ok(v)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn probe_clip(path: String) -> Result<MediaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || probe::probe(Path::new(&path)).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

// ───────────────────────── cache (audio tracks, thumbnails, waveforms) ─────────────────────────

fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(ml_core::paths::LOCAL_DIR_NAME)
}

fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Buffer")
}

fn cache_dir(input: &Path) -> Result<PathBuf, String> {
    let meta = fs::metadata(input).map_err(|e| e.to_string())?;
    let mut h = DefaultHasher::new();
    input.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    let dir = local_dir().join("cache").join(format!("{:016x}", h.finish()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Counting semaphore so a scrolling library never starts dozens of ffmpeg processes at once.
struct Permits {
    free: Mutex<usize>,
    cv: Condvar,
}

impl Permits {
    const fn new(n: usize) -> Self {
        Permits { free: Mutex::new(n), cv: Condvar::new() }
    }
    fn acquire(&self) -> PermitGuard<'_> {
        let mut n = self.free.lock().unwrap();
        while *n == 0 {
            n = self.cv.wait(n).unwrap();
        }
        *n -= 1;
        PermitGuard(self)
    }
}

struct PermitGuard<'a>(&'a Permits);
impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        *self.0.free.lock().unwrap() += 1;
        self.0.cv.notify_one();
    }
}

static FFMPEG_JOBS: Permits = Permits::new(3);

fn ffmpeg_cmd() -> Command {
    let mut c = Command::new(ml_core::tools::ffmpeg());
    hide_window(&mut c);
    c
}

/// The extracted audio tracks are only a stream copy of the clip (a second or two to remake), so they are kept under a
/// cap instead of piling up: over `cap`, the least recently used go first, down to three quarters of it. Thumbnails and
/// waveforms are tiny and stay. A file touched in the last ten minutes (the clip being edited) is never removed.
fn prune_audio(root: &Path, cap: u64) -> u64 {
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for d in rd.flatten() {
            let Ok(inner) = fs::read_dir(d.path()) else { continue };
            for e in inner.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "m4a") {
                    if let Ok(m) = e.metadata() {
                        files.push((m.modified().unwrap_or(UNIX_EPOCH), m.len(), p));
                    }
                }
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return 0;
    }
    files.sort_by_key(|f| f.0);
    let fresh = SystemTime::now().checked_sub(Duration::from_secs(600)).unwrap_or(UNIX_EPOCH);
    let target = cap / 4 * 3;
    let mut freed = 0;
    for (t, len, p) in files {
        if total <= target {
            break;
        }
        if t > fresh {
            continue;
        }
        if fs::remove_file(&p).is_ok() {
            total -= len;
            freed += len;
        }
    }
    freed
}

const AUDIO_CACHE_CAP: u64 = 200 * 1024 * 1024;

fn prune_cache() {
    prune_audio(&local_dir().join("cache"), AUDIO_CACHE_CAP);
}

/// Splits every audio track into its own file (stream copy) so the preview can mix them independently.
fn prepare_audio_sync(input: &Path) -> Result<Vec<PathBuf>, String> {
    let info = probe::probe(input).map_err(|e| e.to_string())?;
    let dir = cache_dir(input)?;
    let files: Vec<PathBuf> = (0..info.audio.len()).map(|i| dir.join(format!("a{i}.m4a"))).collect();
    if files.iter().any(|f| !f.exists()) {
        let _p = FFMPEG_JOBS.acquire();
        let mut cmd = ffmpeg_cmd();
        cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(input);
        for (i, f) in files.iter().enumerate() {
            cmd.args(["-map", &format!("0:a:{i}"), "-vn", "-c", "copy"]).arg(f);
        }
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            for f in &files {
                let _ = fs::remove_file(f);
            }
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        prune_cache();
    } else {
        // used again: the oldest-used files are the ones that go first
        for f in &files {
            if let Ok(h) = fs::File::options().write(true).open(f) {
                let _ = h.set_modified(SystemTime::now());
            }
        }
    }
    Ok(files)
}

#[tauri::command]
async fn prepare_audio(path: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_audio_sync(Path::new(&path))
            .map(|v| v.iter().map(|f| f.to_string_lossy().into_owned()).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn thumbnail(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let input = PathBuf::from(&path);
        let out = cache_dir(&input)?.join("thumb.jpg");
        if !out.exists() {
            let _p = FFMPEG_JOBS.acquire();
            for seek in ["2", "0"] {
                let mut cmd = ffmpeg_cmd();
                cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-ss", seek, "-i"])
                    .arg(&input)
                    .args(["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "5"])
                    .arg(&out);
                let _ = cmd.output();
                if out.exists() {
                    break;
                }
            }
        }
        if out.exists() {
            Ok(out.to_string_lossy().into_owned())
        } else {
            Err("no frame".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Waveform resolution: 50 bars per second, so zooming the timeline stays sharp.
const WAVE_HZ: usize = 4000;
const SAMPLES_PER_BAR: usize = WAVE_HZ / 50;

#[tauri::command]
async fn waveform(path: String, index: usize) -> Result<Vec<u8>, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let input = PathBuf::from(&path);
        let cache = cache_dir(&input)?.join(format!("wave{index}.v2.bin"));
        if let Ok(b) = fs::read(&cache) {
            if !b.is_empty() {
                return Ok(b);
            }
        }
        let files = prepare_audio_sync(&input)?;
        let file = files.get(index).ok_or("no such track")?;
        let _p = FFMPEG_JOBS.acquire();
        let mut cmd = ffmpeg_cmd();
        cmd.args(["-v", "error", "-i"])
            .arg(file)
            .args(["-ac", "1", "-ar", "4000", "-f", "s16le", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let _ = child.wait();
        let samples: Vec<i16> = raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let bars: Vec<u8> = samples
            .chunks(SAMPLES_PER_BAR)
            .map(|c| {
                let peak = c.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0;
                (peak.sqrt() * 100.0).round().clamp(0.0, 100.0) as u8
            })
            .collect();
        let _ = fs::write(&cache, &bars);
        Ok(bars)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn clear_cache() -> Result<u64, String> {
    let dir = local_dir().join("cache");
    let mut bytes = 0u64;
    fn walk(p: &Path, bytes: &mut u64) {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let q = e.path();
                if q.is_dir() {
                    walk(&q, bytes);
                } else if let Ok(m) = e.metadata() {
                    *bytes += m.len();
                }
            }
        }
    }
    walk(&dir, &mut bytes);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

#[tauri::command]
fn cache_size() -> u64 {
    fn walk(p: &Path) -> u64 {
        fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| {
                        let q = e.path();
                        if q.is_dir() { walk(&q) } else { e.metadata().map_or(0, |m| m.len()) }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    walk(&local_dir().join("cache"))
}

// ───────────────────────── export / files ─────────────────────────

static CANCEL_EXPORT: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn cancel_export() {
    CANCEL_EXPORT.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn export_clip(app: AppHandle, opts: ExportOptions) -> Result<ExportResult, String> {
    if let Some(parent) = opts.output.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    CANCEL_EXPORT.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        export::export(
            &opts,
            |p| {
                let _ = app.emit("export-progress", p);
            },
            |n| {
                let _ = app.emit("export-retry", n);
            },
            || CANCEL_EXPORT.load(Ordering::Relaxed),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Game icons the recorder saved: (game name, png path).
#[tauri::command]
fn game_icons() -> Vec<(String, String)> {
    let Ok(rd) = fs::read_dir(data_dir().join("icons")) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "png" {
                return None;
            }
            Some((p.file_stem()?.to_string_lossy().into_owned(), p.to_string_lossy().into_owned()))
        })
        .collect()
}

/// Optional scan: looks for installed games (Steam, Epic, usual folders) for these names and keeps their icons.
/// Resolves with `{ found: [...], missing: [...] }`.
#[tauri::command]
async fn import_game_icons(names: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut args: Vec<&str> = vec!["import-icons"];
        args.extend(names.iter().map(|s| s.as_str()));
        let raw = recorder_cli(&args)?;
        // an older recorder next to the app answers an unknown command with its usage text
        if raw.starts_with("usage:") || raw.is_empty() {
            return Err("The recorder is older than the app: restart Buffer (or update) and try again".to_string());
        }
        serde_json::from_str(&raw).map_err(|e| format!("{e}: {raw}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Renames a clip on disk. A recorder name `Game__2026-10-14__22-38-28` keeps its game and stamp and gets
/// `__<title>` appended (an empty title removes it again); any other file simply takes the title as its name.
/// Returns the new path.
#[tauri::command]
fn rename_clip(path: String, title: String) -> Result<String, String> {
    fn is_digits(s: &str, n: usize) -> bool {
        s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
    }
    let src = PathBuf::from(&path);
    let dir = src.parent().ok_or("no folder")?.to_path_buf();
    let ext = src.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = src.file_stem().ok_or("no name")?.to_string_lossy().into_owned();

    let mut t: String = title
        .chars()
        .map(|c| if c.is_control() || ['<', '>', ':', '"', '/', '\\', '|', '?', '*'].contains(&c) { '-' } else { c })
        .collect();
    while t.contains("__") {
        t = t.replace("__", "_");
    }
    let t: String = t.trim().trim_matches('.').trim().chars().take(80).collect();

    let parts: Vec<&str> = stem.split("__").collect();
    let stamped = parts.len() >= 3 && {
        let d: Vec<&str> = parts[1].split('-').collect();
        let m: Vec<&str> = parts[2].split('-').collect();
        d.len() == 3 && is_digits(d[0], 4) && is_digits(d[1], 2) && is_digits(d[2], 2)
            && m.len() == 3 && m.iter().all(|x| is_digits(x, 2))
    };
    let keep = if stamped { parts[..3].join("__") } else { String::new() };
    if t.is_empty() && !stamped {
        return Err("empty name".into());
    }
    let build = |suffix: &str| {
        let name = if stamped {
            if t.is_empty() { keep.clone() } else { format!("{keep}__{t}{suffix}") }
        } else {
            format!("{t}{suffix}")
        };
        let file = if ext.is_empty() { name } else { format!("{name}.{ext}") };
        dir.join(file)
    };
    let mut dst = build("");
    let mut n = 2;
    while dst != src && dst.exists() {
        dst = build(&format!(" ({n})"));
        n += 1;
    }
    if dst != src {
        fs::rename(&src, &dst).map_err(|e| e.to_string())?;
    }
    Ok(dst.to_string_lossy().into_owned())
}

/// Opens one of a few known pages in the browser: Buffer's own GitHub pages (new issue, releases, license, About tab)
/// and the web configurators of keyboard makers (a link on the keyboard's card). Nothing else.
#[tauri::command]
fn open_issue_page(url: String) -> Result<(), String> {
    const ALLOWED: [&str; 2] = ["https://github.com/Wheatlay/Buffer/", "https://software.darkproject.eu/"];
    if url != "https://github.com/Wheatlay/Buffer" && !ALLOWED.iter().any(|p| url.starts_with(p)) {
        return Err("this page cannot be opened from Buffer".into());
    }
    // rundll32 hands the address straight to the default browser (explorer.exe would open a folder window)
    Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg(format!("/select,{}", path.replace('/', "\\")))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens a file with its default app (the user's own video player or editor).
#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &path.replace('/', "\\")])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Moves files to the Windows Recycle Bin (never a permanent delete).
#[tauri::command]
fn delete_clips(paths: Vec<String>) -> Result<usize, String> {
    let mut n = 0;
    for p in &paths {
        trash::delete(p).map_err(|e| format!("{p}: {e}"))?;
        n += 1;
    }
    Ok(n)
}

// ───────────────────────── persisted UI state ─────────────────────────

#[tauri::command]
fn load_state() -> serde_json::Value {
    fs::read(data_dir().join("app-state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

#[tauri::command]
fn save_state(state: serde_json::Value) -> Result<(), String> {
    let dir = data_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("app-state.json.tmp");
    fs::write(&tmp, serde_json::to_vec(&state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, dir.join("app-state.json")).map_err(|e| e.to_string())
}

// ───────────────────────── tools & recorder ─────────────────────────

#[derive(Serialize, Clone)]
struct ToolsStatus {
    ffmpeg_path: String,
    ffmpeg_found: bool,
    ffmpeg_bundled: bool,
    ffmpeg_version: String,
    /// Encoder the recorder and exporter will use on this machine ("h264_nvenc", "h264_amf", "h264_qsv", "libx264").
    encoder: String,
    nvenc: bool,
    gpu: String,
}

/// Two ffmpeg runs and nvidia-smi cost ~110 ms and the answer cannot change while Buffer runs, so a found ffmpeg is
/// remembered (a missing one is looked for again: it can be installed meanwhile). Settings > Advanced waits for this.
static TOOLS_CACHE: std::sync::OnceLock<ToolsStatus> = std::sync::OnceLock::new();

#[tauri::command]
async fn tools_status() -> ToolsStatus {
    if let Some(t) = TOOLS_CACHE.get() {
        return t.clone();
    }
    let t = tools_status_probe().await;
    if t.ffmpeg_found {
        let _ = TOOLS_CACHE.set(t.clone());
    }
    t
}

async fn tools_status_probe() -> ToolsStatus {
    tauri::async_runtime::spawn_blocking(|| {
        let path = ml_core::tools::ffmpeg();
        let found = ml_core::tools::available(path);
        let mut version = String::new();
        let mut nvenc = false;
        if found {
            let mut c = ffmpeg_cmd();
            if let Ok(o) = c.arg("-version").output() {
                version = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(2)
                    .unwrap_or("")
                    .to_string();
            }
            let mut c = ffmpeg_cmd();
            if let Ok(o) = c.args(["-hide_banner", "-encoders"]).output() {
                nvenc = String::from_utf8_lossy(&o.stdout).contains("h264_nvenc");
            }
        }
        let mut gpu = String::new();
        let mut c = Command::new("nvidia-smi");
        hide_window(&mut c);
        if let Ok(o) = c.args(["--query-gpu=name", "--format=csv,noheader"]).output() {
            gpu = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string();
        }
        let encoder = if found { ml_core::tools::best_h264_encoder().to_string() } else { String::new() };
        ToolsStatus {
            ffmpeg_path: path.to_string_lossy().into_owned(),
            ffmpeg_found: found,
            ffmpeg_bundled: ml_core::tools::is_bundled(),
            ffmpeg_version: version,
            encoder,
            nvenc,
            gpu,
        }
    })
    .await
    .unwrap_or(ToolsStatus {
        ffmpeg_path: String::new(),
        ffmpeg_found: false,
        ffmpeg_bundled: false,
        ffmpeg_version: String::new(),
        encoder: String::new(),
        nvenc: false,
        gpu: String::new(),
    })
}

fn recorder_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let mut candidates = vec![dir.join("buffer-recorder.exe")];
    // development layout: target/<profile>/buffer.exe next to target/<profile>/buffer-recorder.exe
    candidates.push(dir.join("..").join("release").join("buffer-recorder.exe"));
    candidates.push(dir.join("..").join("debug").join("buffer-recorder.exe"));
    candidates.into_iter().find(|p| p.is_file())
}

fn recorder_cli(args: &[&str]) -> Result<String, String> {
    let exe = recorder_exe().ok_or("buffer-recorder.exe not found next to the app")?;
    let mut c = Command::new(exe);
    c.args(args);
    hide_window(&mut c);
    let o = c.output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn recorder_send(cmd: &str) -> Result<String, String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], RECORDER_PORT).into(), Duration::from_millis(600))
        .map_err(|_| "recorder is not running".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(90))).ok();
    writeln!(s, "{cmd}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

/// status | save | reload | quit
#[tauri::command]
async fn recorder_cmd(cmd: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let raw = recorder_send(&cmd)?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The recorder was closed on purpose (tray, Settings) and has not been started since: the watchdog leaves it down.
/// A crash or a kill leaves no marker, and the recorder is brought back.
#[tauri::command]
fn recorder_quit_deliberate() -> bool {
    local_dir().join("recorder.quit").is_file()
}

#[tauri::command]
fn recorder_launch() -> Result<(), String> {
    let exe = recorder_exe().ok_or("buffer-recorder.exe not found next to the app")?;
    let mut c = Command::new(exe);
    c.arg("run").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    hide_window(&mut c);
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn recorder_config() -> serde_json::Value {
    fs::read(data_dir().join("recorder.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

#[tauri::command]
fn set_recorder_config(config: serde_json::Value) -> Result<(), String> {
    let dir = data_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("recorder.json"), serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn recorder_devices() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let raw = recorder_cli(&["devices-json"])?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Peak level (0..1) per endpoint id / "@default:*" token, for the live meters in the audio settings.
#[tauri::command]
async fn recorder_levels(ids: Vec<String>, ms: Option<u64>) -> Result<serde_json::Value, String> {
    if ids.is_empty() {
        return Ok(serde_json::json!({}));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let window = ms.map(|m| format!("--ms={m}"));
        let mut args: Vec<&str> = vec!["levels"];
        if let Some(w) = &window {
            args.push(w);
        }
        args.extend(ids.iter().map(|s| s.as_str()));
        let raw = recorder_cli(&args)?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shows each monitor's number on that monitor for a moment (does not wait for it to disappear).
#[tauri::command]
fn identify_monitors() -> Result<(), String> {
    let exe = recorder_exe().ok_or("buffer-recorder.exe not found next to the app")?;
    let mut c = Command::new(exe);
    c.arg("identify").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    hide_window(&mut c);
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn recorder_monitors() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let raw = recorder_cli(&["monitors-json"])?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// True in a debug build (what `tauri dev` runs); the UI shows a DEV badge so it is never mistaken for the installed app.
#[tauri::command]
fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// The window starts hidden (no blank frame while the page loads); the UI calls this once it has drawn.
/// The page asked for on the command line (`--view=snd`), if any.
fn view_arg(args: &[String]) -> Option<String> {
    args.iter().find_map(|a| a.strip_prefix("--view=")).map(str::to_string)
}

/// The page this launch was asked to open (the first launch; later ones arrive as an `open-view` event).
#[tauri::command]
fn startup_view() -> Option<String> {
    view_arg(&std::env::args().collect::<Vec<_>>())
}

#[tauri::command]
fn show_main_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
    }
}

/// Where clips go on a fresh install: the user's Videos\Buffer folder.
#[tauri::command]
fn default_clips_dir() -> String {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Videos").join("Buffer").to_string_lossy().into_owned()
}

/// Discord is installed (its installer always puts it in `%LOCALAPPDATA%\Discord`, so a running Discord has it too).
/// The setup wizard splits Discord out of the game track only when this is true, otherwise the Chat track would wait
/// for a program that never starts.
#[tauri::command]
fn discord_installed() -> bool {
    std::env::var_os("LOCALAPPDATA").is_some_and(|d| PathBuf::from(d).join("Discord").is_dir())
}

#[derive(Serialize)]
struct FoundFolder {
    path: String,
    source: String,
    clips: usize,
}

fn count_videos(dir: &Path, depth: u32) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 2 {
                n += count_videos(&p, depth + 1);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| VIDEO_EXT.iter().any(|v| v.eq_ignore_ascii_case(x)))
        {
            n += 1;
        }
    }
    n
}

/// Folders where other recorders keep clips, so the library can be set up without browsing.
#[tauri::command]
async fn detect_clip_folders() -> Vec<FoundFolder> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(videos) = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Videos")) else {
            return Vec::new();
        };
        let known = [
            ("SteelSeries Moments", "SteelSeries Moments"),
            ("Captures", "Xbox Game Bar / Windows"),
            ("Medal", "Medal"),
            ("Radeon ReLive", "AMD ReLive"),
            ("Overwolf", "Overwolf"),
            ("Buffer", "Buffer"),
        ];
        let mut out = Vec::new();
        for (dir, source) in known {
            let p = videos.join(dir);
            if p.is_dir() {
                let clips = count_videos(&p, 0);
                if clips > 0 {
                    out.push(FoundFolder { path: p.to_string_lossy().into_owned(), source: source.into(), clips });
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// `None` reports the current state; `Some(on)` changes it. Returns whether autostart is enabled.
#[tauri::command]
fn recorder_autostart(enable: Option<bool>) -> Result<bool, String> {
    match enable {
        Some(true) => {
            recorder_cli(&["install-autostart"])?;
        }
        Some(false) => {
            recorder_cli(&["uninstall-autostart"])?;
        }
        None => {}
    }
    Ok(recorder_cli(&["autostart-status"])? == "enabled")
}

/// The single-instance plugin only claims its mutex once Tauri is set up (hundreds of ms on a cold start), so quick
/// repeated tray clicks used to slip past it and open several windows. This claims a mutex at the very first line; a
/// later launch waits until the first one's plugin is ready, then the plugin hands it over to that window and exits.
fn wait_for_first_instance() {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, wide("Local\\BufferAppStart").as_ptr());
        if h.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            return; // first launch: the handle stays open for the life of the process
        }
        CloseHandle(h);
        // the plugin's mutex name (tauri-plugin-single-instance: "<identifier>-sim")
        let plugin = wide("dev.wheatlay.buffer-sim");
        for _ in 0..150 {
            let p = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, plugin.as_ptr());
            if !p.is_null() {
                CloseHandle(p);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn main() {
    wait_for_first_instance();
    ml_core::paths::migrate_legacy_dirs();
    // the release build has no console: keep panics from any thread in a file next to recorder.log
    if let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join(ml_core::paths::LOCAL_DIR_NAME)) {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = fs::create_dir_all(&dir);
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("app.log")) {
                let _ = writeln!(f, "PANIC in thread '{}': {info}", std::thread::current().name().unwrap_or("?"));
            }
            default(info);
        }));
    }
    tauri::Builder::default()
        // Only one window ever: a second launch (tray click, shortcut) just brings the first one forward.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            // `--view=snd` (the tray flyout's "open" button): switch the running window to that page
            if let Some(v) = view_arg(&args) {
                let _ = app.emit("open-view", v);
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            is_dev_build,
            show_main_window,
            startup_view,
            list_clips,
            probe_clip,
            prepare_audio,
            thumbnail,
            waveform,
            clear_cache,
            cache_size,
            export_clip,
            reveal,
            open_file,
            game_icons,
            import_game_icons,
            rename_clip,
            delete_clips,
            load_state,
            save_state,
            tools_status,
            recorder_cmd,
            recorder_launch,
            recorder_quit_deliberate,
            recorder_config,
            set_recorder_config,
            recorder_devices,
            recorder_monitors,
            identify_monitors,
            recorder_levels,
            cancel_export,
            detect_clip_folders,
            discord_installed,
            default_clips_dir,
            recorder_autostart,
            eq::eq_spectrum_start,
            eq::eq_spectrum_stop,
            eq::eq_probe_level,
            eq::eq_autoeq_search,
            eq::eq_autoeq_fetch,
            eq::eq_apo_status,
            eq::eq_install_apo,
            eq::eq_attach_apo,
            fans::fans_status,
            fans::fans_install,
            fans::fans_uninstall,
            fans::fans_enable,
            fans::fans_disable,
            fans::fans_setup_set,
            fans::fans_detect,
            fans::fans_profile,
            fans::fans_probe,
            fans::fans_boost,
            fans::fans_close_conflict,
            fans::fans_import_find,
            fans::fans_import,
            rgb::rgb_status,
            rgb::rgb_install,
            rgb::rgb_start,
            rgb::rgb_rescan,
            rgb::rgb_close_conflicts,
            rgb::rgb_disable_autostart,
            rgb::rgb_set,
            rgb::rgb_stop,
            rgb::rgb_uninstall,
            rgb::rgb_devices,
            rgb::rgb_scene_get,
            rgb::rgb_scene_set,
            rgb::rgb_look,
            rgb::rgb_unlook,
            rgb::rgb_blink,
            rgb::rgb_follow,
            rgb::rgb_calib,
            rgb::rgb_test,
            rgb::rgb_zone,
            rgb::rgb_hide,
            rgb::rgb_rename,
            rgb::rgb_power,
            rgb::rgb_profile,
            rgb::rgb_game_profile,
            open_issue_page,
        ])
        .setup(|app| {
            std::thread::spawn(prune_cache);
            // safety net: if the UI never reports in (a script error), the window still appears
            if let Some(w) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(4));
                    let _ = w.show();
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Buffer")
        .run(|_, _| {}); // RGB belongs to the recorder: closing the window leaves the lights as they are
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn audio_cache_drops_the_least_recently_used_and_keeps_thumbnails() {
        let root = std::env::temp_dir().join(format!("buffer-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let hour = Duration::from_secs(3600);
        let now = SystemTime::now();
        // four tracks of 100 bytes, oldest first, plus a thumbnail beside the oldest
        for (i, age) in [4u64, 3, 2, 1].iter().enumerate() {
            let dir = root.join(format!("d{i}"));
            fs::create_dir_all(&dir).unwrap();
            let f = dir.join("a0.m4a");
            fs::write(&f, [0u8; 100]).unwrap();
            fs::File::options().write(true).open(&f).unwrap().set_modified(now - hour * (*age as u32)).unwrap();
        }
        fs::write(root.join("d0").join("thumb.jpg"), [0u8; 10]).unwrap();
        assert_eq!(prune_audio(&root, 1000), 0, "under the cap nothing goes");
        // cap 250 -> target 187: the three oldest go, the newest stays
        assert_eq!(prune_audio(&root, 250), 300);
        assert!(root.join("d3").join("a0.m4a").exists());
        assert!(!root.join("d0").join("a0.m4a").exists());
        assert!(root.join("d0").join("thumb.jpg").exists(), "thumbnails stay");
        // a track used a moment ago is never removed, even over the cap
        let f = root.join("d3").join("a0.m4a");
        fs::File::options().write(true).open(&f).unwrap().set_modified(SystemTime::now()).unwrap();
        assert_eq!(prune_audio(&root, 10), 0);
        assert!(f.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
