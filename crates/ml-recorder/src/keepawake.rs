//! Keeps HDMI / DisplayPort outputs awake. When nothing plays, the GPU's audio stream to the monitor goes idle and
//! the monitor needs 1-2 s to lock onto it again, so the start of every sound is lost. A shared-mode stream of
//! zeros on those outputs keeps the link up; it mixes with real sound and changes nothing audible.

use std::{collections::HashMap, sync::atomic::{AtomicBool, Ordering}, sync::Arc, thread, time::Duration};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

/// Interface names of GPU audio (the sound goes over the display cable).
const DISPLAY_AUDIO: [&str; 4] = ["nvidia high definition audio", "amd high definition audio", "intel(r) display audio", "displayport"];

fn is_display_output(iface: &str, name: &str) -> bool {
    let (i, n) = (iface.to_lowercase(), name.to_lowercase());
    DISPLAY_AUDIO.iter().any(|d| i.contains(d)) || n.contains("hdmi") || n.contains("displayport")
}

/// Whether this PC has an active HDMI / DisplayPort output at all (the Sound page shows the setting only then).
static PRESENT: AtomicBool = AtomicBool::new(false);

pub fn present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

/// Active display outputs (id, name), or none while the setting is off.
fn wanted(shared: &crate::session::Shared) -> Vec<(String, String)> {
    let Ok(coll) = DeviceEnumerator::new().and_then(|en| en.get_device_collection(&Direction::Render)) else {
        return Vec::new();
    };
    let found: Vec<_> = coll
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let iface = d.get_interface_friendlyname().unwrap_or_default();
            let name = d.get_friendlyname().unwrap_or_default();
            is_display_output(&iface, &name).then(|| Some((d.get_id().ok()?, name)))?
        })
        .collect();
    PRESENT.store(!found.is_empty(), Ordering::Relaxed);
    if shared.cfg.lock().unwrap().wake_display_audio { found } else { Vec::new() }
}

/// Runs for the life of the recorder; follows the `wake_display_audio` setting within a few seconds.
pub fn start(shared: Arc<crate::session::Shared>) {
    thread::Builder::new()
        .name("keepawake".into())
        .spawn(move || {
            let _ = wasapi::initialize_mta();
            let mut held: HashMap<String, Arc<AtomicBool>> = HashMap::new();
            loop {
                let now = wanted(&shared);
                // a stream whose thread ended (device gone, error) is dropped from `held` and retried next round
                held.retain(|id, stop| {
                    let keep = now.iter().any(|(i, _)| i == id) && !stop.load(Ordering::Relaxed);
                    if !keep {
                        stop.store(true, Ordering::Relaxed);
                    }
                    keep
                });
                for (id, name) in now {
                    if held.contains_key(&id) {
                        continue;
                    }
                    let stop = Arc::new(AtomicBool::new(false));
                    held.insert(id.clone(), stop.clone());
                    thread::spawn(move || {
                        if let Err(e) = play_zeros(&id, &name, &stop) {
                            crate::session::log(&format!("keep-awake on {name}: {e}"));
                            thread::sleep(Duration::from_secs(30)); // do not retry a failing device every few seconds
                        }
                        stop.store(true, Ordering::Relaxed);
                    });
                }
                thread::sleep(Duration::from_secs(5));
            }
        })
        .expect("spawn keepawake thread");
}

fn play_zeros(id: &str, name: &str, stop: &AtomicBool) -> Result<(), String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let device = en.get_device(id).map_err(|e| e.to_string())?;
    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let fmt = WaveFormat::new(32, 32, &SampleType::Float, crate::audio::RATE as usize, 2, None);
    let mode = StreamMode::PollingShared { autoconvert: true, buffer_duration_hns: 2_000_000 };
    client.initialize_client(&fmt, &Direction::Render, &mode).map_err(|e| e.to_string())?;
    let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
    let zeros = vec![0u8; client.get_buffer_size().map_err(|e| e.to_string())? as usize * crate::audio::FRAME_BYTES];
    client.start_stream().map_err(|e| e.to_string())?;
    crate::session::log(&format!("keep-awake: holding {name}"));
    while !stop.load(Ordering::Relaxed) {
        let frames = client.get_available_space_in_frames().map_err(|e| e.to_string())? as usize;
        // plain zeros, not the SILENT flag: some sinks treat flagged buffers as no stream
        render.write_to_device(frames, &zeros[..frames * crate::audio::FRAME_BYTES], None).map_err(|e| e.to_string())?;
        thread::sleep(Duration::from_millis(50));
    }
    let _ = client.stop_stream();
    crate::session::log(&format!("keep-awake: released {name}"));
    Ok(())
}
