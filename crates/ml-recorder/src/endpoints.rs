//! Output switching and "Listen to this device" through the undocumented IPolicyConfig COM interface — the one the
//! Sound control panel and EarTrumpet use. Vtable order (Windows 7 – 11): QueryInterface, AddRef, Release,
//! GetMixFormat, GetDeviceFormat, ResetDeviceFormat, SetDeviceFormat, GetProcessingPeriod, SetProcessingPeriod,
//! GetShareMode, SetShareMode, GetPropertyValue, SetPropertyValue (12), SetDefaultEndpoint (13), SetEndpointVisibility.

use crate::pick::Endpoint;
use wasapi::{DeviceEnumerator, Direction};
use windows_sys::core::GUID;
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

/// PKEY_MonitorEnabled / PKEY_MonitorOutput of a capture endpoint ("Listen to this device" and its target).
const LISTEN_FMTID: GUID = GUID::from_u128(0x24dbb0fc_9311_4b3d_9cf0_18ff155639d4);

#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

/// Just enough of PROPVARIANT for VT_BOOL / VT_LPWSTR.
#[repr(C)]
struct PropVariant {
    vt: u16,
    r: [u16; 3],
    val: [u64; 2],
}

type SetDefault = unsafe extern "system" fn(*mut core::ffi::c_void, *const u16, i32) -> i32;
type SetProperty = unsafe extern "system" fn(*mut core::ffi::c_void, *const u16, i32, *const PropertyKey, *const PropVariant) -> i32;
type Release = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Runs `f` with the IPolicyConfig vtable; returns its HRESULT (negative = failure).
fn with_policy(f: impl FnOnce(*mut core::ffi::c_void, *const usize) -> i32) -> Result<(), String> {
    unsafe {
        CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
        let mut obj: *mut core::ffi::c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(&CLSID_POLICY_CONFIG, std::ptr::null_mut(), CLSCTX_ALL, &IID_POLICY_CONFIG, &mut obj);
        if hr < 0 || obj.is_null() {
            return Err(format!("IPolicyConfig unavailable (0x{hr:08x})"));
        }
        let vtbl = *(obj as *const *const usize);
        let hr = f(obj, vtbl);
        let release: Release = std::mem::transmute(*vtbl.add(2));
        release(obj);
        if hr < 0 { Err(format!("IPolicyConfig call failed (0x{hr:08x})")) } else { Ok(()) }
    }
}

/// Makes `id` the default playback device (console + multimedia; communications stays where the user put it,
/// so a separate chat headset keeps working).
pub fn set_default_output(id: &str) -> Result<(), String> {
    let w = wide(id);
    with_policy(|obj, vtbl| unsafe {
        let set: SetDefault = std::mem::transmute(*vtbl.add(13));
        let a = set(obj, w.as_ptr(), 0); // eConsole
        let b = set(obj, w.as_ptr(), 1); // eMultimedia
        a.min(b)
    })
}

pub fn outputs() -> Vec<Endpoint> {
    let _ = wasapi::initialize_mta();
    let Ok(en) = DeviceEnumerator::new() else { return Vec::new() };
    let Ok(coll) = en.get_device_collection(&Direction::Render) else { return Vec::new() };
    coll.into_iter()
        .flatten()
        .filter_map(|d| Some(Endpoint { id: d.get_id().ok()?, name: d.get_friendlyname().ok()? }))
        .collect()
}

pub fn inputs() -> Vec<Endpoint> {
    let _ = wasapi::initialize_mta();
    let Ok(en) = DeviceEnumerator::new() else { return Vec::new() };
    let Ok(coll) = en.get_device_collection(&Direction::Capture) else { return Vec::new() };
    coll.into_iter()
        .flatten()
        .filter_map(|d| Some(Endpoint { id: d.get_id().ok()?, name: d.get_friendlyname().ok()? }))
        .collect()
}

/// Next microphone after the current default, wrapping around; it becomes the default for every role
/// (communications too: chat apps follow it). Returns its name.
pub fn cycle_input() -> Result<String, String> {
    let cur = default_capture().unwrap_or_default();
    let list = inputs();
    if list.len() < 2 {
        return Err("only one microphone".into());
    }
    let i = list.iter().position(|e| e.id == cur).map(|i| (i + 1) % list.len()).unwrap_or(0);
    let w = wide(&list[i].id);
    with_policy(|obj, vtbl| unsafe {
        let set: SetDefault = std::mem::transmute(*vtbl.add(13));
        set(obj, w.as_ptr(), 0).min(set(obj, w.as_ptr(), 1)).min(set(obj, w.as_ptr(), 2))
    })?;
    Ok(list[i].name.clone())
}

pub fn default_output() -> Option<String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().ok()?;
    en.get_default_device_for_role(&Direction::Render, &wasapi::Role::Console).ok()?.get_id().ok()
}

pub fn default_capture() -> Option<String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().ok()?;
    en.get_default_device_for_role(&Direction::Capture, &wasapi::Role::Console).ok()?.get_id().ok()
}

pub fn default_comms_output() -> Option<String> {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().ok()?;
    en.get_default_device_for_role(&Direction::Render, &wasapi::Role::Communications).ok()?.get_id().ok()
}

/// Outputs the switch shortcut goes through. With `chosen` set, exactly those that are plugged in; otherwise all
/// of them, except a separate communications device (a headset's "Chat" half): making that the main output would
/// send game sound into the chat channel.
pub fn cycle_list(all: Vec<Endpoint>, cur: &str, comms: Option<&str>, chosen: &[String], hidden: &[String]) -> Vec<Endpoint> {
    let all: Vec<Endpoint> = all.into_iter().filter(|e| !hidden.contains(&e.id)).collect();
    if !chosen.is_empty() {
        return all.into_iter().filter(|e| chosen.contains(&e.id)).collect();
    }
    let skip = comms.filter(|c| *c != cur);
    let kept: Vec<Endpoint> = all.iter().filter(|e| Some(e.id.as_str()) != skip).cloned().collect();
    if kept.len() >= 2 { kept } else { all }
}

/// Where the switch shortcut goes from `cur`: the next one in the list, or its first when `cur` is not in it (even
/// a list of one: playing on a device outside the cycle, the shortcut still brings sound back into it).
/// `None` = nowhere to go.
pub fn next_in_cycle(list: &[Endpoint], cur: &str) -> Option<usize> {
    match list.iter().position(|e| e.id == cur) {
        Some(_) if list.len() < 2 => None,
        Some(i) => Some((i + 1) % list.len()),
        None if list.is_empty() => None,
        None => Some(0),
    }
}

/// Next output after the current default, wrapping around; returns its name. `Err(None)`: nothing to switch to
/// (the caller tells the user), `Err(Some(e))`: Windows refused.
pub fn cycle_output(chosen: &[String], hidden: &[String]) -> Result<String, Option<String>> {
    let cur = default_output().unwrap_or_default();
    let list = cycle_list(outputs(), &cur, default_comms_output().as_deref(), chosen, hidden);
    let i = next_in_cycle(&list, &cur).ok_or(None)?;
    set_default_output(&list[i].id).map_err(Some)?;
    Ok(list[i].name.clone())
}

/// Where a mic should be heard: the Windows "communications" output (the headset's chat half, which is where a
/// headset's own mic belongs), else the default output. Windows' own "Default Playback Device" would pick the
/// default output, which for someone on a monitor plus headset is the monitor — silence in the headphones.
pub fn pick_listen_target() -> Option<String> {
    default_comms_output().or_else(default_output)
}

/// Turns "Listen to this device" on or off for a microphone. When turning it on, `target` (an output endpoint id)
/// or `pick_listen_target()` is where it plays.
pub fn set_listen(capture_id: &str, on: bool, target: Option<&str>) -> Result<(), String> {
    let w = wide(capture_id);
    if on {
        // PKEY_Listen_PlaybackDevice: a VT_LPWSTR endpoint id ("" = Windows' default playback device)
        let id = target.map(str::to_string).or_else(pick_listen_target).unwrap_or_default();
        let wid = wide(&id);
        let key = PropertyKey { fmtid: LISTEN_FMTID, pid: 0 };
        let val = PropVariant { vt: 31, r: [0; 3], val: [wid.as_ptr() as usize as u64, 0] };
        with_policy(|obj, vtbl| unsafe {
            let set: SetProperty = std::mem::transmute(*vtbl.add(12));
            set(obj, w.as_ptr(), 0, &key, &val)
        })?;
    }
    let key = PropertyKey { fmtid: LISTEN_FMTID, pid: 1 };
    // VT_BOOL: VARIANT_TRUE is -1
    let val = PropVariant { vt: 11, r: [0; 3], val: [if on { 0xffff } else { 0 }, 0] };
    with_policy(|obj, vtbl| unsafe {
        let set: SetProperty = std::mem::transmute(*vtbl.add(12));
        set(obj, w.as_ptr(), 0, &key, &val)
    })
}

/// The output the mic is currently set to play on (`None` = Windows' default playback device).
pub fn listen_target(capture_id: &str) -> Option<String> {
    let guid = ml_eq::device_guid(capture_id);
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\Properties");
    // Windows keeps this one as plain REG_SZ (measured), unlike the binary "enabled" flag
    ml_eq::apo::hklm_string(&key, "{24dbb0fc-9311-4b3d-9cf0-18ff155639d4},0").filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use crate::pick::Endpoint;

    fn ep(id: &str) -> Endpoint {
        Endpoint { id: id.into(), name: id.into() }
    }

    #[test]
    fn switch_skips_a_separate_chat_device() {
        let all = vec![ep("monitor"), ep("chat"), ep("game")];
        let ids = |v: Vec<Endpoint>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(super::cycle_list(all.clone(), "monitor", Some("chat"), &[], &[])), ["monitor", "game"]);
        // chat is also the main output: keep it, the user chose that
        assert_eq!(ids(super::cycle_list(all.clone(), "chat", Some("chat"), &[], &[])), ["monitor", "chat", "game"]);
        // never shrink to a single device
        assert_eq!(ids(super::cycle_list(vec![ep("a"), ep("chat")], "a", Some("chat"), &[], &[])), ["a", "chat"]);
        // a hidden device is never switched to
        assert_eq!(ids(super::cycle_list(all.clone(), "monitor", None, &[], &["game".to_string()])), ["monitor", "chat"]);
    }

    #[test]
    fn a_chosen_cycle_keeps_only_the_plugged_in_choices() {
        let all = vec![ep("monitor"), ep("chat"), ep("game"), ep("hdmi")];
        let ids = |v: Vec<Endpoint>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
        let chosen = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // the user's choice wins, even the chat device, in the Windows order
        assert_eq!(ids(super::cycle_list(all.clone(), "game", Some("chat"), &chosen(&["game", "chat"]), &[])), ["chat", "game"]);
        // an unplugged choice is skipped, never replaced by another device
        assert_eq!(ids(super::cycle_list(all, "game", None, &chosen(&["game", "usb"]), &[])), ["game"]);
    }

    #[test]
    fn the_shortcut_leaves_a_device_outside_the_cycle() {
        let one = vec![ep("monitor")];
        // playing on the headset, only the monitor in the cycle (03.11.2026): go to the monitor
        assert_eq!(super::next_in_cycle(&one, "headset"), Some(0));
        assert_eq!(super::next_in_cycle(&one, "monitor"), None);
        assert_eq!(super::next_in_cycle(&[], "monitor"), None);
        assert_eq!(super::next_in_cycle(&[ep("a"), ep("b")], "b"), Some(0));
    }

    /// On a real machine: `cargo test -p ml-recorder endpoints -- --ignored --nocapture`. Re-applies the current
    /// default output (no audible change) and reads the mic listen state.
    #[test]
    #[ignore]
    fn policy_config_round_trip() {
        let outs = super::outputs();
        println!("{outs:?}");
        let cur = super::default_output().expect("a default output");
        super::set_default_output(&cur).expect("SetDefaultEndpoint");
        assert_eq!(super::default_output().as_deref(), Some(cur.as_str()));
        let mic = std::env::var("MIC_ID").unwrap_or_default();
        if !mic.is_empty() {
            let was = super::listening(&mic);
            super::set_listen(&mic, !was, None).expect("SetPropertyValue");
            let flipped = super::listening(&mic);
            super::set_listen(&mic, was, None).expect("SetPropertyValue");
            println!("listening: {was} -> {flipped} -> {}", super::listening(&mic));
            assert_eq!(flipped, !was);
        }
    }
}

/// Current "Listen to this device" state, read from the endpoint's stored properties.
pub fn listening(capture_id: &str) -> bool {
    let guid = ml_eq::device_guid(capture_id);
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\Properties");
    // REG_BINARY PROPVARIANT: vt = VT_BOOL (11) in bytes 0..2, boolVal at bytes 8..10 (0xFFFF = true)
    ml_eq::apo::hklm_binary(&key, "{24dbb0fc-9311-4b3d-9cf0-18ff155639d4},1")
        .is_some_and(|b| b.len() >= 10 && b[0] == 11 && b[1] == 0 && b[8] == 0xff && b[9] == 0xff)
}
