# Moments Lite â€” project state

A small, reliable Windows replacement for the SteelSeries GG "Moments" clip recorder and editor:
no bloat, no launcher/update breakage, a richer editor, and a size-targeted exporter (e.g. for Discord).

## Layout
- `crates/ml-core` â€” probe + export (ffmpeg wrapper: target size, trim, 16:9 crop, speed, loudness, NVENC).
- `crates/ml-cli` â€” `ml` command line over ml-core.
- `crates/ml-recorder` â€” background recorder (separate process, tray icon, global hotkey).
- `src-tauri` + `ui/` â€” Tauri v2 app; plain HTML/CSS/JS, no bundler. Fonts are bundled in `ui/fonts`.
- `build-installer.ps1` â€” builds recorder + app + NSIS installer (`target/release/bundle/nsis`).

## Recorder (always-on)
- ffmpeg `ddagrab` + a GPU encoder (NVENC, else AMF, else QuickSync, else x264) â†’ rolling 2-second MPEG-TS segments in `%LOCALAPPDATA%\moments-lite\buffer`.
- Audio is captured by the recorder itself via WASAPI (Game and Chat as loopback of playback endpoints, Mic as
  capture), fed to ffmpeg at real-time rate (zeros while silent/unplugged) so streams never desync or stall.
- Save (hotkey / tray / UI) joins the last N seconds byte-wise (`concat:` protocol) and remuxes to MP4 with three
  audio tracks tagged `Game`/`Chat`/`Mic` â€” the same layout as SteelSeries clips. File name `Game__YYYY-MM-DD__HH-MM-SS.mp4`
  (game = executable name of the foreground window, or `Desktop`; map names in `recorder.json: game_names`).
- Reliability: supervisor restarts ffmpeg with back-off; audio sources reconnect every 2 s; single-instance mutex;
  ffmpeg lives in a kill-on-close job object; tray icon is green (all live) / amber (a source is down) / red (stopped).
  A source that only waits for its chat app (not running) or for a kind of device the PC does not have (no mic at all)
  is down but not a fault (`Sink::fault`, status `fault: false`): no amber pill or tray, no health popup. A chosen device
  that is gone or a failed capture is a fault. Source statuses are codes (`wait_device:<name>`, `wait_app:<stem>`,
  `wait_game`, `no_device`, `error:<details>`) that the UI translates. The wizard splits Discord out of the game track
  only when `%LOCALAPPDATA%\Discord` exists (`discord_installed`).
- Config: `%APPDATA%\moments-lite\recorder.json` (UI: Ustawienia â†’ Nagrywanie). fps, width, bitrate, buffer length,
  monitor, hotkey, devices, audio offset (ms). Control: `ml-recorder status|save|reload|quit` on 127.0.0.1:47821.
- Autostart: HKCU Run key (`ml-recorder install-autostart`), done by the installer and by Settings.

## UI (design handoff "Moments Lite")
Library (virtualised grid, lazy thumbnails, categories/games/favourites, bulk bar, details panel),
Editor (preview with per-track gain, waveforms, trim, 16:9 crop, speed, loudness, export panel, drag-out),
Exports (history, drag-out), Settings (Nagrywanie, WyglÄ…d, OgĂłlne, Eksport, SkrĂłty, Zaawansowane).
Favourites/exports/settings persist in `%APPDATA%\moments-lite\app-state.json`.
- First-run wizard (`wizard()` in `ui/js/recording.js`, also Settings → "Kreator konfiguracji"): 4 steps, no UAC.
  Start shows what was detected as values (encoder, the 3 sources, main monitor, other programs' clip folders from
  `detect_clip_folders`); Sound = the 3 tracks with live meters; Picture = 3 profiles (recommended marked), "keep last",
  clip size, the rest under "Customize", its button starts recording; Shortcut = press the save shortcut, the clip the
  recorder really saved (`status.last_saved`) appears with "Open". "Skip, use detected" jumps to Shortcut. Every step
  change saves the recorder config and, while unfinished, `settings.wizStep`: a closed window resumes there.
- Sidebar: a right click hides EQ / Sound Switcher / RGB / Fans (`settings.navHidden`; Library and Settings stay) and
  shows hidden ones again, as does Settings → General → Sidebar. A module never opened has a lime dot
  (`settings.navSeen`; installs from before it start with all seen).
- Fans / RGB gates propose taking over: FanControl config found -> "Import from FanControl" (enable + import dialog,
  which offers to close FanControl); a lighting app running -> "Close {name} and enable".

## Distribution
- `build-installer.ps1` fetches a checksum-verified static ffmpeg (vendor/, not committed), builds the recorder and app,
  and produces a ~9 MB NSIS installer: per-user, no questions asked, ffmpeg + recorder inside, app opens its first-run
  wizard when done. Custom template: `src-tauri/windows/installer.nsi` (forced passive mode + auto-launch).
- ffmpeg is our own trimmed build of FFmpeg 9.0.2 (~18 MB; `scripts/build-ffmpeg-min.sh`, MSYS2), published as the
  `ffmpeg-9.0.2-min-1` release and pinned by SHA-256 in `scripts/fetch-ffmpeg.ps1`. `fetch-ffmpeg.ps1 -Essentials` falls
  back to the full gyan.dev build (100 MB). A needed component missing from the trimmed build shows up as a runtime
  "decoder/encoder/filter not found": add it to the script, rebuild, bump the release tag and the pinned hash.
- The installer writes `ffmpeg.exe` itself (`src-tauri/windows/hooks.nsh`, hash marker `ffmpeg.id`): an update skips the
  write when the installed copy is the same build. `build-installer.ps1` generates `ffmpeg.nsh` (hash + path).
- Publish: `scripts/release.ps1 -Local` (build here, ~3 min, thin LTO, tests first) or `scripts/release.ps1` (tag; GitHub
  Actions builds it, ~10 min, full LTO). Only the CI path can be Authenticode-signed (SignPath).
- ffmpeg is GPL; `FFMPEG-LICENSE.txt` is shipped next to it.

## Encoders, bindings, long recording
- Encoder "auto": each candidate is probed with the recorder's real arguments (`encoder_usable`); a failing GPU encoder
  falls back to libx264 for the rest of the run (`Shared.encoder_override`, survives settings reloads). x264 is capped
  at 1080p30. Every encoder gets `-force_key_frames` so segments stay 2.0 s. AMF/QSV were NOT run on real hardware
  (only `ffmpeg -h encoder=` option names were checked); NVENC and x264 were.
- Bindings (`hotkey`, `marker_hotkey`, `record_hotkey`) are comma-separated: keyboard combo (RegisterHotKey, re-registered
  live on `reload`; a combo another program holds gives `hotkey_ok: false` in `status`, shown as "Shortcut taken" in the
  wizard and Settings), `Mouse3/4/5` with optional modifiers, `Pad:View+RB` chords (>= 2 buttons, XInput only). `input.rs` polls
  mouse/pad on its own thread (no hooks) and posts WM_HOTKEY to the tray thread; applied live. Pad path untested (no pad).
- Long recording (`longrec.rs`, Alt+R / tray / Settings button / `record-start|record-stop`): finished segments are
  appended to `<game>__<stamp>.rec.ts` next to the clips, remuxed to MP4 on stop (3 named tracks, no faststart).
  Restarts/reloads are held back until it ends; `game_only` idle is overridden; stops itself below 2 GB free;
  orphaned `.rec.ts` files are recovered at recorder start.

## Audio sources
Each of Game/Chat/Mic has a primary and an optional secondary device (id + name). Rules live in `pick.rs` (unit-tested):
primary, else secondary, else wait. A configured-but-missing device never silently becomes the Windows default and
settings are never rewritten by plug/unplug; the primary wins again as soon as it returns.

## Live EQ (Sonar-like, on top of Equalizer APO)
- Equalizer APO (free, GPL, not bundled) does all DSP inside the Windows audio engine: no added latency, works with
  Buffer closed. Optional (not in the first-run wizard): the EQ page installs the pinned
  `EqualizerAPO-x64-1.4.2.exe` (SHA-256 checked, UAC) â€” `src-tauri/src/eq.rs`.
- `crates/ml-eq`: model (7 bands Ă— 7 shapes, per-output System EQ + one Mic EQ, AutoEQ headphone stage, user and
  per-game presets), `buffer.txt` writer, RBJ curve maths (mirrored in `ui/js/eq.js`), registry detection.
- The **recorder is the only writer** of `%APPDATA%\moments-lite\eq.json` and `<APO>\config\buffer.txt`
  (control commands `eq-get|eq-set|eq-bypass|eq-solo|eq-preset|eq-status|eq-outputs|eq-output|eq-listen`).
  The app goes through `recorder_cmd`; uninstall runs `buffer-recorder eq-unhook`.
- Measured on a real install (2026-10-16): APO's config folder is writable by Users (no elevation after install);
  the audio engine can NOT read an include under the user profile (so buffer.txt lives in APO's folder); APO
  hot-reloads an edited include in < 1 s; `Device: {endpoint guid}` scopes lines; all 7 generated filter lines work.
  Hooking in replaces APO's stock config (it only adds -6 dB) with `Include: buffer.txt`, otherwise appends the line.
- One `Device:` section per output, so the EQ follows the Windows default output by itself. Output switch
  binding `output_hotkey` + tray "Output" submenu use the undocumented IPolicyConfig (`endpoints.rs`, like
  EarTrumpet); same interface turns "Listen to this device" on for the mic (verified: on/off round-trip).
- Game presets: while a game in `game_presets` is in front its preset is an in-memory overlay (`ml_eq::Live`,
  like solo) on the output in use, never saved; it moves with the output when the default changes
  (`eq::follow_output`, every 2 s and right after a switch from Buffer).
- The EQ page (`ui/js/eq.js`) always says what is heard: a "now" bar (device Â· preset or `game â†’ preset` Â· headphone
  correction). While a game preset plays, `eq-get` returns its bands (`game_bands`); the graph draws those, locks the
  dots, faders and preset chips, and shows my own EQ as a faint dashed line; "Edit my EQ" unlocks it (`viewOwn`, UI only).
- The games card lists games that have a preset, the one in front and the last five from `eq.json: recent_games`
  (recorder-owned, most recent first, capped at 30). Library clips can't seed it: their names (e.g. `Counter-Strike-2`)
  don't map back to the exe stem the recorder matches on, so a game never seen in front is added by picking its .exe
  (`App.api.pickFile` -> `eq-game-add <stem>`). The card for headphone profiles is called "AutoEQ" (headphones and
  earphones only: the AutoEq database has no speakers).
- Games are listed automatically: the recorder stores every game it sees in front in `eq.json: seen_games` (stem ->
  name); each row has one choice (my EQ / a preset). The recorder owns that list (`eq-set` ignores the page's copy;
  `eq-game-add|-forget <stem>` change it). Choosing a preset for the running game applies at once (`refresh_game`).
  The built-in preset id `gaming` is shown as "Footsteps" ("Kroki") so it isn't mistaken for the per-game presets.
  Debug builds have `eq-testgame <stem>|off` to fake a game in front.
- The tray's audio items are refreshed in tray-icon's event handler on the right click, before the menu opens
  (thread-local `AUDIO` in tray.rs); nothing polls devices in the background.
- **Seamless APO setup (2026-10-16):** one click, one UAC prompt, no wizard pages, no device picker, no reboot.
  APO's NSIS installer has no switch to skip `DeviceSelector /i` (and that program only knows `/u`), so
  `eq_install_apo` / `eq_attach_apo` (`src-tauri/src/eq.rs`) run ONE elevated PowerShell passed inline via
  `-EncodedCommand` (`crates/ml-eq/src/apo_setup.ps1`, filled in by `ml_eq::apo::setup_script`; no file written):
  re-hash the pinned installer, run it with `/S`, kill the picker the moment it starts, `regsvr32` if needed, apply
  the registry ops, restart `audiosrv` (the engine reloads FxProperties; no reboot). Any failure undoes every write
  of that run (undo log) and, if the run installed APO, runs its `Uninstall.exe` (the picker is killed after 12 s: it
  lingers) and deletes `HKLMSOFTWAREEqualizerAPO`. The attach step is a port of `DeviceAPOInfo::install()`
  (GPL source, mirror/equalizerapo) in `crates/ml-eq/src/apo/setup.rs`: pure `plan_device` -> ops, same `Child APOs`
  records, so APO's own uninstaller restores each device. Targets: all active playback devices + the Mic EQ device (else the
  default mic). The "Not active on this device" chip calls `eq_attach_apo` (replaces `eq_open_device_selector`).
  Command line limit: ops use `%M`/`%C` prefix aliases, error if > ~30k chars. `restart_pending` now compares with
  the start of audiodg.exe (not Windows boot). Tests: `cargo test -p ml-eq`; real machine (UAC, restarts audio, PC
  without APO): `cargo test -p buffer apo_ -- --ignored --nocapture --test-threads=1`; `... -p ml-eq -- --ignored attached_here`
  checks the plan against what the real picker wrote on this PC (identical for 3 outputs + 1 mic).
- `apo::status().restart_pending`: APO's install folder or an attached device's FxProperties key is newer than
  the last boot â€” shown as "Restart may be needed" (a hint; never seen true here, the user had rebooted).
- **Clips and the EQ:** endpoint loopback and mic capture are post-APO, so Game/Chat (device sources) and Mic are
  recorded WITH the EQ; per-app sources (`@app:`) are pre-APO and stay clean (measured).
- **Attached is not heard (2026-10-17):** some drivers never run APO (measured: Focusrite USB Audio 4.124 on the dev PC; the
  Realtek output next to it obeys). The EQ page's "Test EQ" chip (`probeEq` in `ui/js/eq.js`, `eq_probe_level` in
  `src-tauri/src/eq.rs`) plays a -40 dBFS 1 kHz tone on the output, reads it back from the loopback (post-APO) with and
  without a band-pass solo at 8 kHz, and calls the EQ heard when the tone drops by 10 dB or more. Manual, because it makes
  a sound and mutes the output for ~4 s. Real machine: `cargo test -p buffer probe_default -- --ignored --nocapture`.
- Spectrum: the app opens its own WASAPI loopback/capture only while the EQ page is open (own radix-2 FFT, 160
  log bins as raw bytes over a `tauri::ipc::Channel`, ~30 fps).

## RGB page (OpenRGB, opt-in)
- Dark Project Midnight 87 (`342D:E40F`, `crates/ml-rgb/src/darkproject.rs`, protocol in `docs-rgb-keyboard.md`): a "Keyboard"
  chip in the RGB header (shown only when the keyboard is found; off by default; `rgb-keyboard on|off`, `rgb.json: keyboard`).
  The page shows it as a device (index 1000, `stored: true`) and the recorder writes its Simple look as a solid colour,
  at most once per 2 s (every write rewrites the keyboard's stored profile). Before the first write the profile is saved in
  `rgb.json: kb_backup`; Give back (`rgb-unlook`), RGB off, leaving Simple mode and clean exit write it back (a crash is healed at the next
  tick). Advanced scenes do not drive it. Compiled and unit-tested only; the recorder-side sync has not run against the keyboard.
- The always-on recorder hosts RGB (`crates/ml-recorder/src/rgb.rs`): when `%APPDATA%\Buffer\rgb.json` says `enabled` it starts
  OpenRGB, connects with `crates/ml-rgb` (SDK protocol 5, never linked: GPL) and shows the saved scene, so lights persist after the
  window closes and come up with the recorder's autostart. Simple mode / paused = one frame, playing Advanced = a stream at the
  scene's fps (default 30, cap 60) with no per-frame allocation; a dropped socket is retried with back-off. Devices it took over go
  back to their earlier mode on `rgb-disable` and on clean recorder exit (mode names are kept in rgb.json, so a crash is covered too).
  A server that already answers on :6742 is reused and never stopped.
- Scene = `RGB-SCENE.md` (`ml-rgb/src/scene.rs`; the effect maths is `ml-rgb/src/engine.rs`, a port of the page's JS engine with golden
  samples from node). Control commands: `rgb-status|rgb-scene-get|rgb-scene-set|rgb-look|rgb-devices|rgb-enable|rgb-disable|rgb-set`;
  Tauri `rgb_*` commands in `src-tauri/src/rgb.rs` are pass-throughs.
- Install (`ml-rgb/src/install.rs`, also `buffer-recorder rgb-install` / `rgb-uninstall`): OpenRGB sees the board and RAM only when
  elevated, so one UAC prompt does everything all-or-nothing: re-verify the pinned zip (held open read-only), unpack into
  `%ProgramFiles%\Buffer\OpenRGB` (admin-only write), register the on-demand task "Buffer OpenRGB" (highest privileges), install
  PawnIO 2.2.0 only if its service is missing (hash + Authenticode checked; never uninstalled). The elevated script is passed
  inline (`-EncodedCommand`), never as a file. The recorder starts/stops the server with `schtasks /Run` and `/End` (no prompt).
- Page `ui/js/rgb.js` ("Simple": colour, brightness, power per device, saved via `rgb_look`). Advanced (3D placement, synced effects)
  lives in `ui/js/rgb-adv.js` against the same scene.
- **Advanced layout C (2026-10-16):** device list (grouped by type, live strip, Arrange) | stage (3D/2D, 2D inset of the other plane) |
  inspector (Effect / Device) + scene settings (speed, brightness, fps). The selected layer is drawn as its shape in space ("field":
  wave = planes of its peaks, radial = sphere, scan = slab and trail, noise = sampled volume) with a draggable direction arrow; the
  origin can follow a device. Case templates (mid-tower, Mini-ITX, dual chamber, open frame, none) plus desk places give slots; a
  dragged device snaps into a free slot that fits and takes its shape (`pts`). Per device: LED 0 / order (rewrites `pts`), Blink
  (`rgb-blink`), layers that act on it (`layers[].skip`), own brightness, delay; the recorder applies them per device
  (`DevPlan.layers/gain/delay`), the page preview does the same.
- **Advanced, round 2 (2026-10-17):** moving is by Blender-style handles only (arrows X red / Y green / Z blue, a ring for the desk
  plane; a click on a device only selects it; the old body-drag intersected the floor plane and ran off at flat camera angles).
  F / double-click / a button centres on the selection; "Fit" frames the lit space. The 3D camera was rotated 180° before (right
  and up vectors had the wrong sign; fixed in `setupCam`). The case is an object (list row "Case"): template change asks first and
  arranges what lost its place (leftovers stand in a row beside it), it can be moved and turned 90° and carries its slots and the
  devices in them. One device can sit in several places (`parts`): the MSI B450 board is ONE LED in OpenRGB (zone "MSI Zone",
  1 LED), so fans and the CPU cooler on its header always show one shared colour; per-fan effects need a separate ARGB controller.
  Devices hidden on the Simple page are hidden here too (same `rgb_hide`). Simple outlines (fan frames, keyboard plate, card,
  stick, headset band). Colour calibration (`rgb-calib.js`, RGB header "Colour calibration"): per device r/g/b percent in
  `rgb.json: calib`, applied in the recorder's `take_over` in every mode; while the panel is open `rgb-test` lights every shown
  device in one test colour. It cannot fix a colour drifting along one long chain.
- **Round 3 (2026-10-17):** the device list groups by place (what stands in the case is indented under the case row, the rest
  "On the desk"); the case can be flipped (mirror, `caseFlip`); slots carry an outline (card, sticks, I/O) so a GPU reads as a
  card even with one LED; every template has a "Board I/O" slot. Sliders snap to round steps (15°, 5 %, 50 ms, 0.25×; Shift =
  fine), the open layer folds by clicking its header again. Share codes (`ui/js/share.js`): "BUF-FX1." / "BUF-EQ1." +
  base64url(deflate-raw(JSON)); the RGB effect menu and the EQ header ("⇄") copy or paste them, pasted ones are cleaned to known
  ranges and become "Mine" / a user preset.
- **Advanced, controls and effects (2026-10-17):** the stage: the left button picks and moves (a drag on empty space does nothing, a click on empty space lets go),
  the middle button orbits (Shift: pans) and never picks, the right button pans, the wheel zooms; the browser's auto-scroll on the middle button is off. A pick in the
  device list centres the camera (distance kept). A click picks a device by its LED dot or by the model drawn for it (card body, keyboard plate, stick), the smallest
  wins: a board spread over the whole case (dots everywhere) used to swallow every click. A card dragged out of a place only snaps into places of the same kind (the
  horizontal card does not flip upright; the Place chips do that). Scan sweeps over the driven devices (`Scene::fit` sets `Layer::ext`, never saved; `fitScan` and
  `scanHead` in `rgb-adv.js` are the same maths): from just before the first device until the tail has left the last, at 48 cm/s at 1x (24 mirrored). A scan, radial,
  sparkle or audio layer picks its colour by strength: the left stop is the end of the trail, the right one the head (a black last stop makes the head invisible with
  Add). No negative layer speed in the UI (old ones are turned: speed positive, direction + 180 degrees); a device's delay can be negative (-500..1000 ms) to run ahead
  of a device that reacts slowly.
- **Simple + Advanced merged (2026-10-17):** one scene, two views (RGB-SCENE.md "One scene, two views"): per device own look >
  effect > given back (`free`), `mode` is only the open view. Simple has a "Scene" effect choice (device follows the effect) and
  "scene" tags; Advanced has "Colour from: Effect / Own colour / Given back" in the inspector and previews looks with a port of
  `Look::at`. Migration (`Scene::migrate`, flag `merged`) kept exactly what was lit in the scene and both profiles (checked by
  comparing driven sets before/after). A deadlock in the first cut (`rgb-scene-set` held the state lock inside its `match` while
  `edit()` locked again) froze the recorder; fixed by copying the scene first.
- **UX pass (2026-10-17):** both views re-read the scene on a view switch / page entry (the Simple view used to show stale looks);
  "All devices" knows the effect (Scene choice, tag, one click puts every device back on it); when no online device plays the
  effect a bar above it says so ("All on the effect"); list rows show the drive state as a dot (lime / own colour / hollow), not
  long tags; removing a device or layer, Arrange and a case change have "Undo" in the toast (`App.toast(msg, kind, {label, run})`);
  dragging the selected device's body moves it (same solve as the ring); mid widths keep stage + inspector side by side; offline
  hidden devices stay hidden (`rgb-status: hidden`). The debug recorder's autostart goes through `conhost.exe --headless`.
- Measured on the dev PC: without admin OpenRGB sees the PNY RTX 3090 and the Arctis 5; the MSI B450 board and RAM need the elevated
  task. The Dark Project keyboard (eVision, `320F:706E`) has no OpenRGB controller.
- Tests: `cargo test -p ml-rgb` (engine, scene, protocol); ignored, hardware: `cargo test -p ml-recorder rgb_hosting -- --ignored --nocapture`
  (lists devices, drives the headset briefly, checks restore; needs the task installed or a server running) and
  `cargo test -p ml-rgb install_and_uninstall -- --ignored` (UAC click, changes the machine).

- **Simple page features (2026-10-16):** colour wheel + hex (HSV model: no saturation is white), effects Static / Breathing / Rainbow /
  Wave rendered by the recorder (no positions needed), optional second colour (breathing, wave, device effects) and direction,
  the device's OWN OpenRGB modes (`Look.native`; the recorder edits the mode bytes with `proto::edit_mode` and sends UPDATE_MODE,
  throttled 0.6 s, 2 s for modes with the AUTOMATIC_SAVE flag), zone length for resizable zones (`rgb-zone`, remembered in
  `rgb.json: zones` and re-applied at connect), profiles (`profiles`, `active_profile`) with per-game binding (`game_profiles`,
  uses the EQ game detection `eq_game_stem`; the profile is an overlay, never saved over the scene), automation (`ml_rgb::power`:
  lock via LogonUI.exe, idle via GetLastInputInfo, schedule; lights out = every device black, the keyboard too), resume after
  sleep (a loop gap > 25 s restarts the server), conflict status (`away.rs` process list: SignalRGB, iCUE, Armoury Crate, G HUB,
  Razer, NZXT CAM, MSI Center, SteelSeries GG ...), "Missing something?" opens a pre-filled GitHub issue (`open_issue_page`).
- Device facts measured on the dev PC: the 4 RAM sticks have 9 own modes and ALL of them have the AUTOMATIC_SAVE flag (every
  change is written to the stick's memory, so a native effect there survives boot and every update wears flash: do not run tests
  that apply native modes to RAM); the GPU has Breathing / Spectrum Cycle / Strobe (no save); headset, board and keyboard have
  Direct only. No zone is resizable on this PC (zone length is covered by a mocked browser test only).
- Before Buffer starts: devices show their own saved mode (RAM: its last saved mode). The recorder autostarts at login and the
  devices are under control about 5 s after it starts. After the window closes nothing changes; if the recorder quits,
  the devices go back to the mode they had.
## Design language
`DESIGN.md` (repo root) describes the look every UI follows, native windows included: lime "now" / faded "trail",
scan bars, the animated logo, the tokens. Read it before adding a screen.

## Audio tray icon and mixer (EarTrumpet style)
- The recorder shows a second tray icon for sound (`audio_tray`, on by default; the SoundSwitcher page). Both icons
  are always created in the same order (recorder first) and the audio one is only hidden when switched off, so
  Windows keeps the user's pinning. Its right-click menu holds the audio items (output, EQ, preset, hear my mic);
  with the icon off those items go back into the recorder's menu (muda allows one item in several menus).
- Left click opens `flyout.rs`: a layered Win32 window in the app's style (DESIGN.md: cards, scan bars, animated logo;
  slides and fades in). Each playback device (default first, lime dot) has a master scan bar and its apps
  (`mixer.rs`: IAudioSessionManager2 sessions grouped by exe path, system sounds first; `windows` 0.62.2, the
  version wasapi already uses). Click the device name = make it the default; click the speaker/icon = mute;
  drag or wheel = volume. It reads Core Audio only while open (~30 fps, list every 1 s) and closes on focus loss / Esc.
- Wheel over the tray icon: Windows sends no wheel messages to notification icons, so while the pointer is on the
  icon (tray-icon Enter/Leave) a Raw Input sink (`RIDEV_INPUTSINK`, no hook) is registered; 5 % per notch on the
  default output. EarTrumpet, if still running, reacts to the same wheel. While the icon is on, its picture and
  tooltip re-read the default output's volume every 2 s (one endpoint, no session enumeration).
- `win::pump_messages` reports only thread timers (`hwnd == null`): window timers (the flyout, tray-icon's hover
  tracking) belong to their windows and used to be taken for the 500 ms tick.
- Output shortcut: `output_cycle` (endpoint ids, the "Shortcut" chip on each SoundSwitcher card) limits the cycle; empty = all
  outputs minus a separate chat device. A chosen device that is unplugged is skipped, never replaced. Playing on a
  device outside the cycle, the shortcut goes to the cycle's first one (even a cycle of one); with nowhere to go it
  shows "Nothing to switch to" instead of doing nothing.
- Replaced output ids (`heal.rs`): Windows can give a device a new endpoint id without an unplug (KB5127936 did it to
  "Arctis 5 Game" on 03.11.2026, its Chat half kept its id). The recorder remembers every output's name in
  `outputs-seen.json` and, every 5 s, moves a referenced id that is gone (cycle, hidden, EQ channel) to the ONE output
  with that name never seen before; the move is remembered, so a stale copy the app writes back is fixed again. EQ
  then needs APO attached to the new id (the EQ page's attach chip, one UAC).
- Icon rail marks from `status.nav` (recorder, cached 5 s): `warn` = amber dot (EQ: APO not on the output that plays;
  SoundSwitcher: the switch shortcut has nowhere to go), `off` = greyed (EQ without APO, RGB without OpenRGB, Fans
  without the helper). The reason is the item's tooltip; the lime "new" dot gives way to the amber one.
- Device order (flyout and page): default output, the chat (communications) device, devices with apps, the rest;
  by name inside each group.
- App page SoundSwitcher (`ui/js/sound.js`, rail label "Sound\nSwitcher"): the same mixer through `mix-get|mix-set|mix-default|mix-icon`
  (recorder control, polled every 120 ms only while the page is open), the cycle chips, the switch-output shortcut,
  and the tray-icon toggle. Hiding Windows' own volume icon is not possible on Windows 11: the HideSCAVolume
  policy is ignored (tried 2026-10-16 with an Explorer restart), so that option was removed again.
- Loopback recorders (Buffer itself, SignalRGB, OBS) have a session on the output whose meter is the whole device
  mix; nothing in the session identifiers tells them apart (measured), so `Dev::app_peak` hides a meter that
  equals the device's exactly while the device volume is below 100 %.
- Shortcuts accept named keys (PgUp/PgDn, Home/End, Ins/Del, arrows, Space, Num0-9, ...); the settings field
  records `KeyboardEvent.code`, `win::parse_hotkey` reads the same names.
- `reload` restarts the capture only when a capture setting changed (`control::needs_restart`); shortcuts, mixer
  and popup options, language and theme apply live.
- Debug builds: `mix-flyout` (control) opens the flyout for screenshots.
- `tray::check_output` (every 500 ms tick and right after our own switches) notices a default-output change made
  anywhere (shortcut, flyout, app, Windows, plugging a headset): the EQ follows, `output_popup` (on) names the new
  device, `keep_volume` (off) gives it the old device's volume (`Dev::set_level`, mute untouched). The shortcut no
  longer pops up on its own; this one popup covers every source.
- `mic_hotkey` cycles the default microphone (every role, communications included) with a "Microphone switched"
  popup. `hidden_outputs`: left out of the flyout, the tray menu and the cycle (the default always shows); the page
  folds them under the list ("N hidden", Show).
- Shortcut keys also take media keys (VolUp/VolDown/VolMute, MediaNext/Prev/Play/Stop): a keyboard key remapped to
  "volume down" (the user's PgDn) sends VK_VOLUME_DOWN, so the binding is Ctrl+VolDown, as EarTrumpet took it.
- Flyout layout: centred animated logo + name on top, device cards, a full-width "Open SoundSwitcher" button at
  the bottom. It runs `buffer.exe --view=snd`; the app reads `--view=` at start (`startup_view`) and a second
  launch forwards it to the running window (single-instance â†’ `open-view` event).
- SoundSwitcher page layout: device cards on the left (the picked one has a lime-tinted border), the picked
  device's apps on the right; the tray-icon switch sits top right where the EQ page has its on/off switch.

## Fans page (LibreHardwareMonitor helper, opt-in)
- Plan, decisions and what was measured: `FANS-SPEC.md`. Crates: `ml-fans` (model, curve engine, discover, pair, import, client, install),
  `crates/ml-recorder/src/fans.rs` (hosting), `src-tauri/src/fans.rs` (pass-through), UI `ui/js/fans.js` (Simple + data + live loop)
  and `ui/js/fans-adv.js` (Advanced: Curves | Wiring | Case). The helper is `helpers/fans` (C#, LibreHardwareMonitorLib 0.9.6),
  built by `scripts/build-fans-helper.ps1` into `helpers/fans/dist/buffer-fans.zip` (7.5 MB).
- The recorder owns the loop (1 s): reads the helper over `\\.\pipe\buffer-fans`, `Engine::step`, sends changed speeds. Setup = `%APPDATA%\Buffer\fans.json`
  (`setup` + named `profiles`); control commands `fans-status|enable|disable|setup-set|detect|probe-start|profile|import-find|import`.
- Safety lives in the HELPER: heartbeat watchdog (10 s of silence -> every control Buffer touched goes to the fail-safe speed, default 100 %),
  `quit` and normal exit call `SetDefault()`. Never stop it with `schtasks /End` or a kill: LHM keeps the board's original registers only in
  that process (`ml_fans::install::stop` asks for `quit` first). The recorder only ever stops it that way.
- Conflicts per device: FanControl & co pause every control, the NVIDIA app only the GPU ones; MSI Afterburner too, but only while its software fan curve is on (`SwAutoFanControl=1` in its `MSIAfterburner.cfg`; merely running is no conflict) (`away::fan_conflicts`).
- Advanced = Panel (default; layout "C, controls first", decided 2026-10-17) | Curves | Wiring. Panel (`fans-adv.js viewPanel`): heat strip (CPU/GPU/Mix glow
  with their temperature, fail-safe speed, "Case in RGB", "Assign fans"), control cards (mini side view of the case with the control's fans lit, curve /
  Auto / Manual select, live % and rpm; click picks in place, drag reorders `setup.controls`), curve cards (source temperature, shape), and on the right the
  stage plus Curve | Tuning | Placement | History of the picked control. On Placement a click on a place adds it to the control or takes it away.
- Stage (`ui/js/fans-stage.js`): the RGB page's case (`App.RGBAdv.geom`: `slotsOf(tpl, caseState)`, `caseXf`; templates mid, full, matx, itx, sff, dual,
  open), read from `rgb_scene_get` and never written (`rgb-scene-set` replaces the whole scene; the case is changed in RGB, "Case in RGB" opens it with the
  case picked: `App.RGBAdv.openCase()`). Places: the template's rings + `cpu2` (push-pull behind the heatsink) + `gpuf1..3` on the card + extra places shown
  only while placing (`bottom1..3`, `rear2`, `side1..2`). One control = a GROUP of fans: `fans.json` `layout` is control -> comma-separated place ids (old
  `rear`/`gpu1`/`gpu2` map to `rear1`/`gpuf1`/`gpuf2`); a place belongs to one control. `setup.flow` = place -> "in" | "out" (front/bottom default in,
  rear/top/rad out; CPU cooler and card fans move air inside, the pump none). RGB devices standing in a place show as a ring in their look's colour.
- Airflow (`ui/js/fans-air.js`, pure, no DOM; `node scripts/check-air.js` times it and checks it): velocity field on a 14³ grid around the case rebuilt
  every 0.15 s (intakes = sources, exhausts = sinks, only felt inside the case; a jet along each fan's axis with suction behind it; curl-noise turbulence
  scaled by the local speed), up to 2400 particles with a little inertia, wall bounces, leaks through gaps when over-pressured. Particles carry a
  temperature: they heat inside the CPU heatsink and graphics card volumes towards that part's temperature (faster the hotter it is), cool slowly
  elsewhere; exhaust outlet temperature = average of what leaves there. Intake air = the coolest board sensor (else 25 °C, marked "~"). ~0.6 ms per
  step for ~1600 particles; the stage draws at about 30 fps, buckets trails by colour (one stroke each), lowers the particle count when a frame takes
  over 9 ms, pauses when hidden, and honours reduced motion / animations off. An estimate, not CFD (the tooltip says so).
- Assign fans wizard (`ui/js/fans-wizard.js`): every visible control in turn is spun up with `fans_boost` (recorder `fans-boost <id> <pct> <secs>`: at
  most 15 s, renewed every 4 s by the page, ends by itself; replaces that control's curve in the worker loop; refused while another fan program runs)
  and the user clicks where a fan turns; nothing is saved before the last step. The Detect dialog uses the same stage (compact).
- Detect: first connect pairs controls with fans by LHM numbering; the probe (`fans-probe-start`, only on a click, audible) drives each board
  control to 100 % for 7 s and pairs it with the fan whose rpm rose (`ml_fans::pair`, needs +150 rpm and 2x the runner-up). A control that
  did not move anything while its fan reads 0 is hidden; one that answered is shown. A single 0 rpm reading never hides anything.
- Import: `ml_fans::import` reads FanControl's `userConfig.json`; schema and enum values were taken from FanControl.Library.dll metadata
  (`MixFunction { Max=0, Sum=1, Average=2, Min=3, Subtract=4 }` for curves, `SensorMixFunction { Average=0, Max=1, Min=2, Sum=3, Subtract=4 }`
  for Mix sensors). Ids differ from LHM's (`/lpc/nct6797d/control/1` -> `/lpc/nct6797d/0/control/1`; `NVApiWrapper/0-GA102-A/control/0` ->
  `/gpu-nvidia/0/control/1`). Calibration tables, time-average / file / offset sensors are not imported.
- Share code `BUF-FAN1.` carries curves and Mix sensors only (control ids are hardware); a pasted code is clamped and its sensors checked.
- Measured on the dev PC (NCT6797D + RTX 3090): see FANS-SPEC.md "Verified". Not verified: AMD/Intel GPUs, other boards, AIO coolers.

## Not verified by hand / known gaps
- The installer itself was built but not run end-to-end on a clean machine (no sandbox available here).
- Lip-sync of recorded clips (audio vs video) â€” durations match to ~5 ms and there is an `audio_offset_ms` knob.
- Tray menu, global hotkey, popup over a real game, drag-out to Discord were not exercised by hand.
- Popup cannot appear above exclusive-fullscreen games (borderless is fine); windowed games need "Dodatkowe gry".
- Not done: multi-cut/join, captions, import of SteelSeries-saved edits.

## Languages (i18n)
- UI text lives in `ui/js/locales/<code>.js` (en = master/fallback, pl, de, es, fr, uk, tr, ja, zh); `ui/js/i18n.js` provides
  `App.t(key, vars)` (`{name}` placeholders, plural objects via `Intl.PluralRules`). Call `App.t()` at draw time, never at script load:
  the language is known only after the saved state loads.
- English is the default (`settings.lang`). The OS language is only *suggested*: a link on wizard step 1 for new users, a one-time
  dismissible bar for returning users (`settings.langHint` = handled). Switching saves state and reloads the window.
- All catalogs except `pl` are AI-translated; Settings â†’ App â†’ Language shows a small "AI-translated" note for them (`ai: true`).
- Add a key to `en.js` first, then every locale; `node scripts/check-i18n.js` checks key/placeholder/plural parity.
- The recorder (tray menu, tooltip, popups, overlay) uses `crates/ml-recorder/src/lang.rs` (same 9 languages); the app writes `lang` into recorder.json on boot and on language switch.
