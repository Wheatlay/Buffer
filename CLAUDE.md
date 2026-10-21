# Buffer: notes for Claude

Windows clip recorder/editor, live EQ, sound switcher and RGB control. Rust workspace + Tauri v2, plain HTML/CSS/JS UI in
`ui/` (no bundler). Architecture and measured facts live in `STATE.md`; the look in `DESIGN.md`. Read both before UI work.

## How to work here
- **UI design:** use Buffer's own language (`DESIGN.md`): lime `#c6f432` = "now", faded lime = "trail", olive neutrals, scan
  bars, animated logo. Applies to native windows too. Verify with a screenshot (app: CDP, see below) before handing over.
- **Text:** keep visible UI text short. Essential info stays as a few words; explanations go in `title=` tooltips. Do not
  strip all descriptions either. Every string through `App.t()` in all 9 locales, `node scripts/check-i18n.js` must pass.
- **No needless toggles:** a behaviour that should always be on gets no Settings toggle; check that each toggle is read
  somewhere (grep) and delete dead ones.
- **Lists and cards must not refresh when clicked.** Selecting or toggling an item must not rebuild the whole list or replay
  its entrance animation (it flashes and loses scroll). Update in place (classes, the detail pane); if a rebuild is
  unavoidable, keep scroll position and suppress the entrance animation (`.rlist.still` in `rgb.js` is the pattern). This
  regressed more than once: after any list/card change, click through it in the real window and watch for a flash.
- **Small targets:** clickable glyphs (chevrons, close buttons) need a hit area of about 32 px, not the glyph's size.
- **Never release without being asked** (they once clicked a release by accident). The `.cmd` helpers are in
  `.git/info/exclude`.

## Dev loop
- `scripts/dev.ps1 [-Inspect]` (or `Run Buffer (dev).cmd`): UI live-reloads, Rust rebuilds in debug. `-Inspect` opens
  WebView2's debug port 9222: drive and screenshot the real window over CDP (`Runtime.evaluate`, `Page.captureScreenshot`)
  instead of Playwright. Native windows (tray flyout, popups) cannot be seen by CDP.
- The running dev `buffer.exe` / `buffer-recorder.exe` lock `target\debug`. **Closing them (even mid-recording) is fine**: stop
  both, build in the normal target dir, start again. The dev app relaunches a quit recorder within seconds.
  `cargo build -p ml-recorder` must really replace the exe: check its timestamp, an old recorder silently lacks new commands.
- The login autostart (`HKCU\...\Run\BufferRecorder`) must point at the INSTALLED recorder (`%LOCALAPPDATA%\Buffer`). A debug
  one at login opens a window that needs the `tauri dev` server (ERR_CONNECTION_REFUSED after every reboot); a debug
  `install-autostart` therefore registers the installed copy.
- Test with the user's LIVE state in mind (they edit EQ and lights while testing): back up and restore only what a test
  changed. Hardware tests must put devices back (RGB: "Give back"; keyboard: restore its saved profile).
- Releases: `scripts\release.ps1 -Local` (~3 min, builds here) or `scripts\release.ps1` (GitHub Actions, ~10 min). Both take
  `-DryRun`; `bump-version.ps1` sets the version everywhere. Notes: `release-notes/<version>.md` in the format of
  `release-notes/TEMPLATE.md` (summary line, then New / Improved / Fixed / Removed / Heads-up bullets), committed first;
  no free-form `-Notes` any more (`RELEASING.md` "Release notes"). Only the latest release is kept. Installed copies read
  `latest.json` from `releases/latest`; the updater never goes down in version. Builds are unsigned (SignPath Foundation
  rejected the application on 2026-10-16; may reapply once the project has more visibility).

## Fans in short
- `FANS-SPEC.md` has the decisions and measurements; `STATE.md` "Fans page" the architecture. The helper (`helpers/fans`, C#) runs elevated from
  the task "Buffer Fans" and owns the safety (watchdog, `SetDefault`). **Never hard-kill it** (`schtasks /End`, `Stop-Process`): ask it to `quit`
  (`ml_fans::install::stop`, or `fans-disable` / recorder `quit`), otherwise the board's original fan settings are lost until a reboot.
- **Defender**: a freshly built `buffer-recorder.exe` is blocked for ~20-30 s (cloud first-sight, `Trojan:Win32/Bearfoos.B!ml`) and has been
  quarantined, and the `BufferRecorder` Run value emptied (`buffer-recorder install-autostart` restores it). After each build wait ~30 s before
  starting it. The recorder has NO install command on purpose (scripts that register elevated tasks make it look like a dropper): install from
  the app, or `cargo test -p ml-fans dev_install -- --ignored`. Check a release build against Defender before publishing.
- The dev app (`buffer.exe`) relaunches a quit recorder, so a recorder rebuild needs both stopped (`rc quit`, stop `buffer.exe`), then
  `scripts\dev.ps1 -Inspect`. Hardware tests: stop `FanControl.Service` first (Buffer pauses while it runs), start it again afterwards.
- Never put a single 0 rpm reading to use (Fan #6 read 0 once while spinning). IDs are LibreHardwareMonitor identifiers, never list positions.

## RGB (OpenRGB) in short
- OpenRGB (GPL-2.0) is never bundled or linked: installed on request by one elevated, all-or-nothing step into
  `%ProgramFiles%\Buffer\OpenRGB` (admin-only write; an elevated task must never run a user-writable exe), plus an on-demand
  scheduled task "Buffer OpenRGB" the recorder starts/stops without UAC. PawnIO is installed only if missing and never
  removed. Elevated scripts go inline (`-EncodedCommand`), never as files. The recorder hosts OpenRGB and the render loop, so
  lights outlive the window. Details: `STATE.md` "RGB page", `RGB-SCENE.md`, `docs-rgb-keyboard.md`.
- The Dark Project keyboard (`342D:E40F`) is not an OpenRGB device: every lighting write rewrites its STORED profile. The
  recorder saves the profile before the first write (`rgb.json: kb_backup`), writes at most every 2 s, and "Give back"
  restores it byte for byte. Never send profile-switch, reset, key-binding or firmware commands to it.
- Hardware seen on the dev PC: board + 4 RAM sticks (elevated only), GPU, headset, the keyboard. UI must not promise every device.
- Equalizer APO setup is one UAC prompt with no wizard (`ml-eq/src/apo/setup.rs`); a test run once uninstalled the user's real
  APO. Before any install/uninstall test, snapshot what is installed first.
- OpenRGB cannot read some devices' real mode: an Aerox 9 Wireless left in its own Rainbow Breathing is reported as
  "Direct", so `SET_CUSTOM_MODE` alone did nothing and Buffer's colours never showed. `take_over` therefore sends the
  Direct mode itself (`proto::direct_mode`) before the first colours of a session.
- Recorder state keyed by DEVICE KEY (`name#n`), never by list index: the OpenRGB list can change while it runs (a false
  detection once shifted every index and put the GPU into "Off" on restore). RAM sticks save every native mode to their own
  memory: never test native effects on them. Always check `modes` example output (`cargo run -p ml-rgb --example modes`) after
  hardware tests and repair a device with `--example setcolor` if needed.