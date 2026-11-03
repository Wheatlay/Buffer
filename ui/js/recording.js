"use strict";
/* Settings → Nagrywanie and the first-run setup wizard (design: "Settings regroup + Setup wizard"). */

App.RecUI = (function () {
  const { h, S, api } = App;
  const U = App.ui;
  const FOLLOW = "@default";            // "whatever Windows currently uses" (speakers, headset…)
  const SRC = [
    ["game", "Game", "src.game", "render", "#1fb89a", "src.gameTip"],
    ["chat", "Chat", "src.chat", "render", "#4a9ae0", "src.chatTip"],
    ["mic", "Mic", "src.mic", "capture", "#e0a23a", "src.micTip"],
  ];
  const CHAT_APPS = () => [["", App.t("chat.none")], ["discord", "Discord"], ["teamspeak", "TeamSpeak"], ["ts3client_win64", "TeamSpeak 3"],
    ["teams", "Microsoft Teams"], ["zoom", "Zoom"], ["telegram", "Telegram"], ["skype", "Skype"]];
  /** A program's name from its exe stem ("discord" → "Discord"). */
  const appName = (stem) => (stem && CHAT_APPS().find(([v]) => v === stem)?.[1]) || stem || "";
  const BASE = {
    output_dir: "", per_game_folders: true, post_roll_seconds: 15, game_only: false, follow_game_monitor: true, chat_app: "",
    buffer_seconds: 120, fps: 30, width: 1920, video_kbps: 14000, quality: "std", monitor: 0,
    encoder: "auto", audio_offset_ms: 0, hotkey: "Alt+Q", record_hotkey: "Alt+R", output_hotkey: "", output_cycle: [], audio_tray: true, keep_volume: false, wake_display_audio: true, output_popup: true, mic_hotkey: "", hidden_outputs: [], popup: true, sounds: true, game_sound: true, sound_level: "normal", extra_games: [], ignore_apps: [],
    game: { primary: null, secondary: null }, chat: { primary: null, secondary: null }, mic: { primary: null, secondary: null },
  };
  const RES = () => [[1280, "720p"], [1920, "1080p"], [2560, "1440p"], [0, App.t("res.native")]];
  const FPS = [[30, "30 fps"], [60, "60 fps"]];
  const QUAL = () => [["low", App.t("qual.low")], ["std", App.t("qual.std")], ["high", App.t("qual.high")]];
  const QUAL_FACTOR = { low: 0.55, std: 1, high: 1.4 };
  // Old configs stored an absolute bitrate; map the three known values to a level, keep anything else as "custom".
  const LEGACY_QUAL = { 8000: "low", 14000: "std", 30000: "high" };
  const LEN = () => [[30, App.t("unit.s", { n: 30 })], [60, App.t("unit.min", { n: 1 })], [120, App.t("unit.min", { n: 2 })], [300, App.t("unit.min", { n: 5 })]];
  const POST_ROLL = () => [[0, App.t("roll.now")], ...[15, 30, 60].map((n) => [n, App.t("unit.s", { n })])];
  const PRESETS = [
    ["eco", "pre.eco", "pre.ecoD", { width: 1920, fps: 30, quality: "std" }, "1080p · 30 fps"],
    ["bal", "pre.bal", "pre.balD", { width: 2560, fps: 30, quality: "high" }, "1440p · 30 fps"],
    ["max", "pre.max", "pre.maxD", { width: 0, fps: 60, quality: "high" }, null],
  ];

  let cfg = { ...BASE };
  let devices = { render: [], capture: [], default: {} };
  let monitors = [];
  let autostart = false;
  let tools = null;
  let meterTimer = 0;
  let openKey = null;               // which audio row is expanded

  const nameOf = (list, id) => (list.find((d) => d.id === id) || {}).name || "";
  const fmtBuf = (s) => `${Math.floor(s / 60)}:${String(Math.round(s) % 60).padStart(2, "0")}`;
  const fmtMb = (mb) => (mb >= 1000 ? `${(mb / 1000).toFixed(1)} GB` : `${Math.round(mb)} MB`);
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  /** replaceChildren that skips null/false (a bare null would be printed as the text "null"). */
  const put = (el, ...kids) => el.replaceChildren(...kids.filter((k) => k != null && k !== false));

  async function load() {
    const [c, d, m, a, dir, t] = await Promise.all([
      api.invoke("recorder_config").catch(() => ({})),
      api.invoke("recorder_devices").catch(() => ({ render: [], capture: [], default: {} })),
      api.invoke("recorder_monitors").catch(() => []),
      api.invoke("recorder_autostart", { enable: null }).catch(() => false),
      api.invoke("default_clips_dir").catch(() => ""),
      api.invoke("tools_status").catch(() => null),
    ]);
    cfg = { ...BASE, output_dir: dir, ...c };
    if (c.quality === undefined) cfg.quality = LEGACY_QUAL[cfg.video_kbps] || "";
    for (const [k] of SRC) cfg[k] = { primary: null, secondary: null, ...(c[k] || {}) };
    devices = { render: [], capture: [], default: {}, ...d };
    monitors = m; autostart = a; tools = t;
  }

  async function save(patch, hint = "") {
    cfg = { ...cfg, ...patch };
    // an empty "backup" placeholder (shown but never filled in) must not be persisted
    const clean = JSON.parse(JSON.stringify(cfg));
    for (const [k] of SRC) if (clean[k].secondary && !clean[k].secondary.id) clean[k].secondary = null;
    await api.invoke("set_recorder_config", { config: clean });
    if (S.recorder) {
      await App.timeout(api.invoke("recorder_cmd", { cmd: "reload" }), 4000).catch(() => {});
      if (hint) App.toast(hint);
    }
    App.Rec.refresh();
  }

  /* ───────────── shared pieces ───────────── */

  const defaultFor = (kind, key) =>
    kind === "capture" ? devices.default.capture : key === "chat" ? devices.default.communications : devices.default.render;

  /* A binding is a comma-separated list with at most one entry per kind: keyboard ("Alt+Q"),
     mouse ("Mouse4", "Ctrl+Mouse5") and gamepad chord ("Pad:View+RB"). Rust (input.rs) reads the same text. */
  const PAD_NAMES = ["A", "B", "X", "Y", "LB", "RB", "LT", "RT", "View", "Menu", "L3", "R3", "Up", "Down", "Left", "Right"];
  const kindOf = (t) => (t.startsWith("Pad:") ? "pad" : /Mouse\d/.test(t) ? "mouse" : "key");
  const bindParts = (v) => String(v || "").split(",").map((x) => x.trim()).filter(Boolean);
  const bindText = (v) => bindParts(v).map((t) => t.replace(/^Pad:/, "🎮 ").replaceAll("+", " + ")).join("   ·   ");

  /** Field that records the next keyboard shortcut, mouse button or gamepad chord. A new input replaces
      the entry of its own kind; Delete clears it (a required binding falls back to `fallback`).
      `onSet(binding, keyboardChanged)`; the recorder applies every kind live (`reload`, no restart). */
  /** The recorder's name for a physical key (KeyboardEvent.code, so the layout and Shift don't change it);
      null for keys a global shortcut can't use. Mirrors `parse_hotkey` in the recorder's win.rs. */
  const NAMED_KEYS = { PageUp: "PgUp", PageDown: "PgDn", Home: "Home", End: "End", Insert: "Ins", Delete: "Del",
    ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right", Space: "Space", Pause: "Pause",
    ScrollLock: "ScrollLock", NumpadAdd: "NumAdd", NumpadSubtract: "NumSub", NumpadMultiply: "NumMul", NumpadDivide: "NumDiv",
    // media keys, also what a keyboard's remapped key sends (e.g. PgDn set to "volume down", as EarTrumpet took it)
    AudioVolumeDown: "VolDown", AudioVolumeUp: "VolUp", AudioVolumeMute: "VolMute", MediaTrackNext: "MediaNext",
    MediaTrackPrevious: "MediaPrev", MediaPlayPause: "MediaPlay", MediaStop: "MediaStop" };
  const keyName = (code) => {
    let m;
    if ((m = /^Key([A-Z])$/.exec(code)) || (m = /^Digit(\d)$/.exec(code))) return m[1];
    if ((m = /^Numpad(\d)$/.exec(code))) return "Num" + m[1];
    if (/^F([1-9]|1\d|2[0-4])$/.test(code)) return code;
    return NAMED_KEYS[code] || null;
  };

  const hotkeyField = (value, onSet, fallback) => {
    let cur = String(value || "");
    const set = (kind, token) => {
      cur = [...bindParts(cur).filter((t) => kindOf(t) !== kind), ...(token ? [token] : [])].join(", ");
      input.value = bindText(cur);
      onSet(cur, kind === "key");
    };
    let padTimer = 0;
    const stopPad = () => { clearInterval(padTimer); padTimer = 0; };
    const modsOf = (e) => [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Win"].filter(Boolean);
    const input = h("input", { class: "field keyfield", value: bindText(cur), readOnly: true,
      placeholder: App.t("hk.ph"), title: App.t("hk.tip"),
      onBlur: stopPad,
      onFocus: () => {
        stopPad();
        let peak = 0, seen = 0;
        const bits = (x) => x.toString(2).replaceAll("0", "").length;
        padTimer = setInterval(() => {
          const pad = [...(navigator.getGamepads?.() || [])].find((g) => g && g.connected);
          const mask = pad ? pad.buttons.slice(0, 16).reduce((m, b, i) => (b.pressed ? m | (1 << i) : m), 0) : 0;
          if (bits(mask) > bits(peak)) peak = mask;
          if (mask) { seen = Date.now(); return; }
          if (peak && Date.now() - seen > 80) {
            if (bits(peak) >= 2) set("pad", "Pad:" + PAD_NAMES.filter((_, i) => peak & (1 << i)).join("+"));
            else App.toast(App.t("hk.pad2"), "err");
            peak = 0;
          }
        }, 40);
      },
      onKeydown: (e) => {
        e.preventDefault();
        if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
        const mods = modsOf(e);
        if (!mods.length && (e.key === "Delete" || e.key === "Backspace")) {
          cur = "";
          input.value = bindText(cur);
          onSet(cur, true);
          return;
        }
        if (!mods.length) { App.toast(App.t("hk.needMod"), "err"); return; }
        const k = keyName(e.code) || keyName(e.key);   // media keys may come without a code
        if (!k) { App.toast(App.t("hk.allowed"), "err"); return; }
        set("key", [...mods, k].join("+"));
      },
      onMousedown: (e) => {
        if (e.button === 0 || e.button === 2) return;         // left / right click only focus the field
        e.preventDefault();
        const mods = modsOf(e);
        if (e.button === 1 && !mods.length) { App.toast(App.t("hk.midMod"), "err"); return; }
        set("mouse", [...mods, "Mouse" + ({ 1: 3, 3: 4, 4: 5 })[e.button]].join("+"));
      },
      // the browser would also navigate back / forward on the side buttons
      onMouseup: (e) => { if (e.button >= 1) e.preventDefault(); },
      onAuxclick: (e) => e.preventDefault() });
    return input;
  };

  /** Amber line "Shortcut taken by another program — Change", hidden until the recorder reports `hotkey_ok: false`
   *  (`paint` shows it). `change` focuses the shortcut field. */
  const takenWarn = (id, change) => h("div", { id, class: "hktaken", hidden: true, title: App.t("hk.takenTip") },
    h("span", { text: App.t("hk.taken") + " — " }),
    h("button", { class: "linkbtn", text: App.t("common.change"), onClick: change }));

  /** Seconds left before a pressed save is written (the post-roll), or 0. */
  const rollLeft = (r) => (r && r.post_roll_left_ms > 0 ? Math.ceil(r.post_roll_left_ms / 1000) : 0);

  /** Select of devices; `allowNone` adds an empty choice (used for the optional backup device). */
  function deviceSelect(key, kind, cur, onPick, allowNone) {
    const list = kind === "render" ? devices.render : devices.capture;
    const defName = nameOf(list, defaultFor(kind, key));
    const real = cur && cur.id && cur.id !== FOLLOW;
    const match = real && list.find((d) => d.id === cur.id || d.name.toLowerCase() === (cur.name || "").toLowerCase());
    const opts = [];
    if (allowNone) opts.push(h("option", { value: "", text: App.t("dev.pick"), selected: !cur || !cur.id }));
    opts.push(h("option", { value: FOLLOW, selected: !!cur && cur.id === FOLLOW,
      text: App.t("dev.default") + (defName ? " " + App.t("dev.now", { name: defName }) : "") }));
    if (real && !match) opts.push(h("option", { value: cur.id, text: App.t("dev.gone", { name: cur.name }), selected: true }));
    for (const d of list) opts.push(h("option", { value: d.id, text: d.name, selected: !!match && match.id === d.id }));
    return h("select", { class: "field", onChange: (e) => {
      const v = e.target.value;
      if (!v) return onPick(null);
      if (v === FOLLOW) return onPick({ id: FOLLOW, name: App.t("dev.default") });
      const d = list.find((x) => x.id === v);
      onPick(d ? { id: d.id, name: d.name } : cur);
    } }, opts);
  }

  /** Do the game and chat tracks currently resolve to the same playback device? */
  function sharedOutput(c = cfg) {
    const idOf = (key) => {
      const p = c[key].primary;
      if (!p || !p.id) return "";
      if (p.id !== FOLLOW) return p.id;
      return key === "chat" ? devices.default.communications : devices.default.render;
    };
    const g = idOf("game"), ch = idOf("chat");
    return !!g && g === ch;
  }

  /** Name of the device a source will use, plus a short note when something needs attention. */
  function deviceLabel(c, key, kind) {
    const list = kind === "render" ? devices.render : devices.capture;
    if (key === "chat" && c.chat_app) return { text: App.t("dev.sep", { app: appName(c.chat_app) }), note: "" };
    const p = c[key].primary;
    let text = App.t("dev.unset");
    if (p && p.id === FOLLOW) text = nameOf(list, defaultFor(kind, key)) || App.t("dev.winDef");
    else if (p && p.name) text = p.name;
    let note = "";
    if (key === "game" && c.chat_app) note = App.t("dev.noChat");
    if (key === "chat" && !c.chat_app && sharedOutput(c)) note = App.t("dev.shared");
    return { text, note };
  }

  /** 10-bar live level meter; `big` = wizard size. */
  const meterEl = (color, big) => h("span", { class: "mtr" + (big ? " big" : "") },
    Array.from({ length: 10 }, (_, j) => h("i", { style: { height: `${35 + ((j + 1) / 10) * 65}%`, background: color } })));
  const paintMeter = (el, level) => {
    const lv = Math.min(1, level * 1.4);
    [...el.children].forEach((b, j) => { b.style.opacity = lv * 0.9 + 0.1 > (j + 1) / 10 ? 0.95 : 0.15; });
  };

  /** Tokens for the level meter of each source (works with or without the recorder running). */
  function meterTokens(c) {
    return SRC.map(([key, , , kind]) => {
      if (c.chat_app && key === "chat") return "@app:" + c.chat_app;
      if (c.chat_app && key === "game") return "@appx:" + c.chat_app;
      const p = c[key].primary;
      if (!p || !p.id) return "";
      if (p.id !== FOLLOW) return p.id;
      return kind === "capture" ? "@default:capture" : key === "chat" ? "@default:comm" : "@default:render";
    });
  }

  /** Does the Game level move with the mic level? That is the sign of the mic being played back on the output
      (interface monitoring, "Listen to this device") and so recorded a second time inside the Game track. */
  function bleedDetector() {
    const mic = [], game = [], N = 30;
    const mean = (a) => a.reduce((x, y) => x + y, 0) / a.length;
    return (g, m) => {
      mic.push(m); game.push(g);
      if (mic.length > N) { mic.shift(); game.shift(); }
      if (mic.length < N) return null;
      const mm = mean(mic), gm = mean(game);
      let cov = 0, vm = 0, vg = 0;
      for (let i = 0; i < N; i++) {
        cov += (mic[i] - mm) * (game[i] - gm);
        vm += (mic[i] - mm) ** 2;
        vg += (game[i] - gm) ** 2;
      }
      if (vm / N < 0.002 || vg / N < 0.0005 || gm < 0.02) return false;   // no speech yet, or the game track is silent
      return cov / Math.sqrt(vm * vg) > 0.85;
    };
  }

  /** Warning row for the audio section; `set(true)` shows it. */
  function bleedWarn() {
    const el = h("div", { class: "bleedwarn", title: App.t("aud.bleedTip"), hidden: true }, "⚠ " + App.t("aud.bleed"));
    return { el, set: (on) => { el.hidden = !on; } };
  }

  /** Polls levels and paints `[data-src] .mtr` under `rootSel`; `ms` = measuring window, `gap` = pause between rounds.
      `onBleed(bool)` hears whether the mic seems to be inside the Game track. */
  function pollMeters(getCfg, rootSel, ms, gap, onBleed) {
    clearTimeout(meterTimer);
    let alive = true;
    const detect = bleedDetector();
    const run = async () => {
      const rows = [...document.querySelectorAll(`${rootSel} [data-src]`)];
      if (!alive || !rows.length) return;
      const ids = meterTokens(getCfg());
      const levels = await api.invoke("recorder_levels", { ids: ids.filter(Boolean), ms }).catch(() => ({}));
      if (!alive) return;
      for (const row of rows) {
        const i = SRC.findIndex((s) => s[1] === row.dataset.src);
        const m = row.querySelector(".mtr");
        if (m) paintMeter(m, levels[ids[i]] ?? 0);
      }
      const c = getCfg();
      if (onBleed && ids[0] && ids[2] && !c.game_only) {
        const v = detect(levels[ids[0]] ?? 0, levels[ids[2]] ?? 0);
        if (v !== null) onBleed(v);
      }
      meterTimer = setTimeout(run, gap);
    };
    run();
    return () => { alive = false; clearTimeout(meterTimer); };
  }

  /* ───────────── video: rows, sizes ───────────── */

  /** The presets for this screen: none asks for more than the monitor has, so a 1080p screen is offered 1080p,
      and the label is the real output size (an ultrawide's "1080p" is 1920×804). */
  function presetsFor(c) {
    const mw = outputSize({ ...c, width: 0 })[0];
    const steps = RES().map(([v]) => v).filter((v) => v > 0 && v <= mw);   // resolutions this screen can use, ascending
    const fitW = (v) => (v > mw ? steps[steps.length - 1] || 0 : v);
    const balW = fitW(PRESETS[1][3].width);
    return PRESETS.map(([id, name, desc, p]) => {
      let width = fitW(p.width);
      // Light must stay lighter than Balanced: when the screen squeezed them to the same size, go one step down
      if (id === "eco" && width >= balW) { const below = steps.filter((v) => v < balW).pop(); if (below) width = below; }
      if (!width) return [id, name, desc, { ...p, width }, null];
      const [ow, oh] = outputSize({ ...c, width });
      const size = Math.abs(ow / oh - 16 / 9) < 0.02 ? `${oh}p` : `${ow}×${oh}`;
      return [id, name, desc, { ...p, width }, `${size} · ${p.fps} fps`];
    });
  }

  function outputSize(c) {
    const mon = monitors.find((m) => m.index === c.monitor) || monitors[0] || { width: 1920, height: 1080 };
    const w = c.width > 0 ? Math.min(c.width, mon.width) : mon.width;
    return [w, Math.round((mon.height * w) / mon.width / 2) * 2];
  }

  /** Same formula as ml-core::bitrate::recording_kbps (a Rust test pins its values). */
  function videoKbps(c, w, hgt) {
    if (!QUAL_FACTOR[c.quality]) return c.video_kbps;     // custom absolute value
    const k = 14000 * Math.pow((w * hgt) / (1920 * 804), 0.75) * Math.pow(c.fps / 30, 0.77) * QUAL_FACTOR[c.quality];
    return Math.min(120000, Math.max(2000, Math.round(k)));
  }

  function estimates(c) {
    const [w, hgt] = outputSize(c);
    const kbps = videoKbps(c, w, hgt);
    const clipMb = ((kbps + 3 * 192) * c.buffer_seconds) / 8000;
    const diskMb = clipMb * (1 + 8 / c.buffer_seconds);                  // plus the segments being written
    const ramMb = 200 + 170 * ((w * hgt * c.fps) / (1920 * 804 * 30));   // calibrated on one measurement (367 MB @ 1080p30)
    return { w, hgt, kbps, clipMb, diskMb, ramMb };
  }

  /** "1: PHL 34M2C8600 · 3440×1440 ★ · po lewej" — the name and the side tell identical screens apart. */
  function monitorLabel(m) {
    const prim = monitors.find((x) => x.primary) || monitors[0];
    let side = "";
    if (prim && m.index !== prim.index) {
      const dx = m.left - prim.left, dy = m.top - prim.top;
      side = Math.abs(dx) >= Math.abs(dy) ? (dx < 0 ? App.t("mon.left") : App.t("mon.right")) : (dy < 0 ? App.t("mon.above") : App.t("mon.below"));
    }
    const generic = !m.name || /generic|pnp|default monitor/i.test(m.name);
    return [`${m.index + 1}`, generic ? App.t("mon.generic") : m.name.replace(/\s*\(.*\)\s*$/, "")].join(": ") + ` · ${m.width}×${m.height}${m.primary ? " ★" : ""}${side ? " · " + side : ""}`;
  }

  const ENCODERS = () => [["auto", App.t("enc.auto")], ["h264_nvenc", "NVIDIA"], ["h264_amf", "AMD"], ["h264_qsv", "Intel"], ["libx264", App.t("enc.cpu")]];
  /** Will the video be encoded on the GPU? Honors the encoder picked in Settings, else the detected one. */
  const hw = () => {
    const e = cfg.encoder && cfg.encoder !== "auto" ? cfg.encoder : tools && tools.encoder;
    return !!(e && e !== "libx264");
  };

  /** What goes into a clip: how much before the shortcut and how much after it. */
  function clipRows(c, set) {
    const roll = U.srow(App.t("key.roll"), U.segc(POST_ROLL(), c.post_roll_seconds, (v) => set({ post_roll_seconds: +v })));
    roll.title = App.t("key.rollTip");
    return [U.srow(App.t("v.len"), U.segc(LEN(), c.buffer_seconds, (v) => set({ buffer_seconds: +v }))), roll];
  }

  /** Resolution, frame rate, quality and (when there is more than one) the screen. */
  function pictureRows(c, set) {
    const customQ = !QUAL_FACTOR[c.quality];
    const rows = [
      U.srow(App.t("v.res"), U.segc(RES(), c.width, (v) => set({ width: +v }))),
      U.srow(App.t("v.fps"), U.segc(FPS, c.fps, (v) => set({ fps: +v }))),
      U.srow(App.t("v.quality"), U.segc([...QUAL(), ...(customQ ? [["", App.t("v.custom", { n: c.video_kbps / 1000 })]] : [])], c.quality || "", (v) => { if (v) set({ quality: v }); })),
    ];
    if (monitors.length) {
      rows.push(U.srow(App.t("v.screen"), h("div", { class: "screenrow" },
        U.segc(monitors.map((m) => [m.index, monitorLabel(m)]), c.monitor, (v) => set({ monitor: +v })),
        monitors.length > 1 ? h("button", { class: "linkbtn", text: App.t("v.identify"), title: App.t("v.identifyTip"),
          onClick: () => api.invoke("identify_monitors").catch(() => {}) }) : null)));
    }
    return rows;
  }

  /** The encoder the recorder would pick right now, for the status next to the encoder choice. */
  const encoderName = () => ({ h264_nvenc: "NVIDIA NVENC", h264_amf: "AMD AMF", h264_qsv: "Intel QuickSync", libx264: App.t("enc.cpu") })[tools && tools.encoder] || App.t("enc.unknown");

  const bars20 = (v, max) => {
    const n = Math.round(Math.min(1, v / max) * 20);
    return Array.from({ length: 20 }, (_, i) => (i < n ? 0.35 + (i / 20) * 0.65 : 0.12));
  };

  /** Sticky "Co to zajmie" card; call update(c) after any video change. */
  let sumLoad = null;            // the "Load" row of the summary card: { text, meters, cpu, gpu, row }; paint() feeds it live values
  let defaultsAt = 0;
  function summaryCard() {
    const defs = [[App.t("sum.clip"), 500], [App.t("sum.disk"), 700], [App.t("sum.ram"), 1400]];
    const refs = defs.map(([label]) => ({
      val: h("span", { class: "sv" }),
      bars: Array.from({ length: 20 }, () => h("i")),
      note: h("div", { class: "sn" }),
      label,
    }));
    const loadEl = h("span", { class: "mono lime", style: { fontWeight: 600 } });
    const lbar = () => h("span", { class: "lbar" }, Array.from({ length: 10 }, () => h("i")));
    const cpuBar = lbar(), gpuBar = lbar();
    const meters = h("span", { class: "loadm", hidden: true }, h("small", { text: "CPU" }), cpuBar, h("small", { text: "GPU" }), gpuBar);
    const capEl = h("div", { class: "sc" });
    const el = h("div", { class: "sumside" }, U.cap(App.t("sum.cap")),
      ...refs.map((r) => h("div", null,
        h("div", { class: "sh" }, h("b", { text: r.label }), r.val),
        h("div", { class: "sb" }, r.bars), r.note)),
      h("div", { class: "sl", title: App.t("sum.loadTip") }, h("b", { text: App.t("sum.load") }), loadEl, meters), capEl);
    sumLoad = { text: loadEl, meters, cpu: cpuBar, gpu: gpuBar, row: loadEl.parentElement };
    const update = (c) => {
      const e = estimates(c);
      const vals = [e.clipMb, e.diskMb, e.ramMb];
      const notes = [
        e.clipMb <= 10 ? App.t("sum.d10") : e.clipMb <= 50 ? App.t("sum.d50") : App.t("sum.big"),
        App.t("sum.temp"), App.t("sum.est")];
      refs.forEach((r, i) => {
        r.val.textContent = (i === 2 ? "≈ " : "") + fmtMb(vals[i]);
        bars20(vals[i], defs[i][1]).forEach((o, j) => { r.bars[j].style.opacity = o; });
        r.note.textContent = notes[i];
      });
      loadEl.textContent = hw() ? "GPU" : "CPU";
      capEl.textContent = App.t("sum.caption", { w: e.w, h: e.hgt, fps: c.fps, size: fmtMb(e.clipMb * 10) });
    };
    update(cfg);
    return { el, update };
  }

  /* ───────────── Settings → Nagrywanie · Dźwięk · Skróty ───────────── */

  /** `which`: "rec" (clip, picture, games), "aud" (tracks, sync) or "keys" (shortcuts). All three share the status bar. */
  async function renderTab(pane, which = "rec") {
    await load();
    const sum = which === "rec" ? summaryCard() : null;

    // sticky status bar
    const dot = h("span", { class: "sdot" });
    const title = h("b", { text: "…" });
    const line = h("div", { class: "mono muted", style: { fontSize: "11.5px" } });
    const saveBtn = h("button", { class: "btn acc", text: App.t("rec.save"), onClick: async () => {
      try {
        const r = await api.invoke("recorder_cmd", { cmd: "save" });
        if (r.ok) { App.toast(App.t("rec.savedToast", { name: App.baseName(r.path) }), "ok"); setTimeout(() => App.Lib.load(), 800); }
        else App.toast(App.t("rec.notSaved", { err: r.error }), "err");
      } catch (e) { App.toast(App.t("rec.notRunning", { err: e }), "err"); }
    } });
    const toggleBtn = h("button", { class: "btn", onClick: async () => {
      if (S.recorder) { App.Rec.expectDown = Date.now(); await api.invoke("recorder_cmd", { cmd: "quit" }).catch(() => {}); }
      else await api.invoke("recorder_launch").catch((e) => App.toast(String(e), "err"));
      setTimeout(App.Rec.refresh, 1500);
    } });
    const longBtn = h("button", { class: "btn", title: App.t("long.tip"),
      onClick: async () => {
        const on = !!(S.recorder && S.recorder.long && S.recorder.long.active);
        await api.invoke("recorder_cmd", { cmd: on ? "record-stop" : "record-start" }).catch((e) => App.toast(App.t("rec.notRunning", { err: e }), "err"));
        setTimeout(App.Rec.refresh, 300);
      } });
    const bar = h("div", { class: "statusbar", id: "recState" }, dot, h("div", null, title, line), h("span", { class: "sp" }), longBtn, saveBtn, toggleBtn);
    bar._refs = { dot, title, line, saveBtn, toggleBtn, longBtn };
    const group = (cap, ...kids) => h("div", null, U.cap(App.t(cap)), ...kids);

    if (which === "rec") {
      // KLIP: what goes into a clip (before and after the shortcut)
      const clipBox = h("div", { class: "card2" });
      const drawClip = () => put(clipBox, ...clipRows(cfg, (patch) => { save(patch); drawClip(); sum.update(cfg); }));
      drawClip();

      // OBRAZ: picture, the screen, and the encoder next to what was detected
      const picBox = h("div", { class: "card2" });
      const drawPic = () => put(picBox,
        ...pictureRows(cfg, (patch) => { save(patch); drawPic(); sum.update(cfg); }),
        U.trow(App.t("beh.follow"), App.t("beh.followD"), cfg.follow_game_monitor, (v) => { save({ follow_game_monitor: v }); drawPic(); }),
        U.srow(App.t("adv.encoder"), h("div", { class: "pathrow" },
          h("select", { class: "field", title: App.t("adv.encoderTip"), onChange: (e) => { save({ encoder: e.target.value }); sum.update(cfg); drawPic(); } },
            ENCODERS().map(([v, t]) => h("option", { value: v, text: t, selected: (cfg.encoder || "auto") === v }))),
          h("span", { class: "mono " + (hw() ? "lime" : "warn"), style: { fontSize: "12px" },
            text: "● " + (cfg.encoder && cfg.encoder !== "auto" ? ENCODERS().find(([v]) => v === cfg.encoder)?.[1] : encoderName()) + (tools && tools.gpu && hw() ? " · " + tools.gpu : "") }))));
      drawPic();

      // GRY: what the recorder counts as a game
      const gamesBox = h("div", { class: "card2 tgl" });
      const drawGames = () => put(gamesBox,
        U.trow(App.t("beh.gameOnly"), App.t("beh.gameOnlyD"), cfg.game_only, (v) => { save({ game_only: v }); drawGames(); }),
        U.srow(App.t("adv.games"), h("input", { class: "field", placeholder: App.t("adv.gamesPh"),
          title: App.t("adv.gamesTip"),
          value: (cfg.extra_games || []).join(", "),
          onChange: (e) => save({ extra_games: e.target.value.split(",").map((x) => x.trim().toLowerCase().replace(/\.exe$/, "")).filter(Boolean) }) })));
      drawGames();

      pane.append(bar, h("div", { class: "setgrid" },
        h("div", { class: "setmain" }, group("cap.clip", clipBox), group("cap.picture", picBox), group("cap.games", gamesBox)),
        sum.el));
    } else if (which === "aud") {
      // ŚCIEŻKI
      const audioBox = h("div", { class: "card2 audio", id: "recSources" });
      const bleed = bleedWarn();
      const persistSources = () => save({ game: cfg.game, chat: cfg.chat, mic: cfg.mic, chat_app: cfg.chat_app });
      const drawAudio = () => {
        audioBox.replaceChildren(...SRC.map(([key, name, label, kind, color, tip]) => {
          const open = openKey === key;
          const dev = deviceLabel(cfg, key, kind);
          const head = h("div", { class: "arow", onClick: () => { openKey = open ? null : key; drawAudio(); } },
            h("span", { class: "sq", style: { background: color } }),
            h("b", { text: App.t(label), title: App.t(tip) }),
            h("span", { class: "dev" }, dev.text, dev.note ? h("em", { text: dev.note }) : null),
            meterEl(color, false),
            h("span", { class: "arr", text: open ? "▴" : "▾" }));
          let body = null;
          if (open) {
            const c = cfg[key];
            body = h("div", { class: "abody" },
              deviceSelect(key, kind, c.primary, (v) => { cfg[key] = { ...cfg[key], primary: v }; persistSources(); drawAudio(); }, false),
              c.secondary
                ? h("div", { class: "pathrow", title: App.t("aud.backupTip") },
                  deviceSelect(key, kind, c.secondary, (v) => { cfg[key] = { ...cfg[key], secondary: v }; persistSources(); drawAudio(); }, true),
                  h("button", { class: "btn ghost", text: "✕", title: App.t("aud.backupRemove"), onClick: () => { cfg[key] = { ...cfg[key], secondary: null }; persistSources(); drawAudio(); } }))
                : h("button", { class: "linkbtn", text: App.t("aud.backup"), title: App.t("aud.backupTip"),
                  onClick: () => { cfg[key] = { ...cfg[key], secondary: { id: "", name: "" } }; drawAudio(); } }),
              key === "chat" && (sharedOutput() || cfg.chat_app)
                ? h("div", { class: "srow", title: App.t("aud.chatAppTip") },
                  h("span", { class: "lb", text: App.t("aud.chatApp") }),
                  h("select", { class: "field", onChange: (e) => { cfg.chat_app = e.target.value; persistSources(); drawAudio(); } },
                    CHAT_APPS().map(([v, t]) => h("option", { value: v, text: t, selected: (cfg.chat_app || "") === v }))))
                : null);
          }
          return h("div", { class: "aitem", "data-src": name }, head, body);
        }));
        pollMeters(() => cfg, "#recSources", 350, 350, bleed.set);
      };
      drawAudio();

      // SYNCHRONIZACJA
      // slider and number field show the same value; it is saved once the slider is let go (or the number is entered)
      const clampOff = (v) => Math.max(-500, Math.min(500, Math.round(+v || 0)));
      const offFill = (v) => offRange.style.setProperty("--p", (v + 500) / 10 + "%");
      const offNum = h("input", { class: "field monoin", type: "number", min: -500, max: 500, step: 10, value: cfg.audio_offset_ms, title: App.t("adv.offsetTip"),
        onChange: (e) => { const v = clampOff(e.target.value); e.target.value = v; offRange.value = v; offFill(v); save({ audio_offset_ms: v }); } });
      const offRange = h("input", { class: "rline offrange", type: "range", min: -500, max: 500, step: 10, value: cfg.audio_offset_ms || 0, title: App.t("adv.offsetTip"),
        "aria-label": App.t("adv.offset"),
        onInput: (e) => { offNum.value = e.target.value; offFill(+e.target.value); },
        onChange: (e) => save({ audio_offset_ms: clampOff(e.target.value) }) });
      offFill(cfg.audio_offset_ms || 0);
      const syncBox = h("div", { class: "card2" },
        U.srow(App.t("adv.offset"), h("div", { class: "pathrow" }, offRange, offNum, h("span", { class: "muted", text: "ms" }))));

      pane.append(bar, h("div", { class: "setcol" }, group("cap.audio", audioBox, bleed.el), group("cap.sync", syncBox)));
    } else {
      // SKRÓTY: every global shortcut of the recorder in one place
      /** One shortcut row: the capture field plus a clear button once something is set ("no shortcut" is allowed). */
      const keyRow = (label, key, value, redraw, tip) => {
        const field = hotkeyField(value, (c) => { save({ [key]: c }); redraw(); });   // the recorder applies shortcuts live
        if (key === "hotkey") field.id = "hkSave";
        const clear = value ? h("button", { class: "linkbtn", text: "✕", title: App.t("hk.clear"),
          onClick: () => { save({ [key]: "" }); redraw(); } }) : null;
        const row = U.srow(label, h("div", { class: "pathrow" }, field, clear));
        if (tip) row.title = App.t(tip);
        return row;
      };
      const recBox = h("div", { class: "card2" });
      const devBox = h("div", { class: "card2" });
      const drawKeys = () => {
        put(recBox,
          keyRow(App.t("key.save"), "hotkey", cfg.hotkey, drawKeys, "key.saveTip"),
          takenWarn("hkTaken", () => App.$("#hkSave")?.focus()),
          keyRow(App.t("key.record"), "record_hotkey", cfg.record_hotkey, drawKeys, "long.tip"));
        put(devBox,
          keyRow(App.t("key.output"), "output_hotkey", cfg.output_hotkey || "", drawKeys, "key.outputTip"),
          keyRow(App.t("key.mic"), "mic_hotkey", cfg.mic_hotkey || "", drawKeys, "key.micTip"));
      };
      drawKeys();
      pane.append(bar, h("div", { class: "setcol" }, group("cap.recKeys", recBox), group("cap.devKeys", devBox)));
    }
    paint();
  }

  /** Ogólne: the two blocks that live in the recorder's config: starting with Windows (appended to `pre`, the rows the
   *  caller builds) and the notifications (popup, cues, their volume). */
  function generalBlocks(pre) {
    const main = h("div", { class: "card2 tgl" });
    const drawMain = () => put(main, ...pre, U.trow(App.t("beh.autostart"), App.t("beh.autostartD"), autostart, (v) => {
      const was = autostart;
      autostart = v;                       // shown at once; the registry write follows
      drawMain();
      App.timeout(api.invoke("recorder_autostart", { enable: v }), 6000)
        .then((now) => { autostart = now; })
        .catch(() => { autostart = was; })
        .finally(drawMain);
    }));
    drawMain();
    // the game cue and the volume only exist while the cues are on
    const notify = h("div", { class: "card2 tgl" });
    const drawNotify = () => put(notify,
      U.trow(App.t("beh.popup"), App.t("beh.popupD"), cfg.popup, (v) => { save({ popup: v }); drawNotify(); }),
      U.trow(App.t("beh.sounds"), App.t("beh.soundsD"), cfg.sounds, (v) => { save({ sounds: v }); drawNotify(); }),
      cfg.sounds ? U.trow(App.t("beh.gameSound"), App.t("beh.gameSoundD"), cfg.game_sound, (v) => { save({ game_sound: v }); drawNotify(); }) : null,
      cfg.sounds ? U.srow(App.t("beh.volume"), U.segc(
        [["low", App.t("vol.low")], ["normal", App.t("vol.normal")], ["high", App.t("vol.high")]],
        cfg.sound_level || "normal",
        async (v) => {
          await save({ sound_level: v });
          drawNotify();
          // hear the new level at once (a recorder that is not running just has nothing to play)
          api.invoke("recorder_cmd", { cmd: "test-sound" }).catch(() => {});
        })) : null);
    drawNotify();
    return { main, notify };
  }

  /** Updates the live parts (status bar, device names, diagnostics) without rebuilding the tab. */
  function paint() {
    // the diagnostics line lives on the Advanced tab, which has no status bar
    const r0 = S.recorder;
    // Shortcuts tab: the save shortcut is held by another program; the field gets the focus once, when that appears
    const taken = App.$("#hkTaken");
    if (taken) {
      const show = !!r0 && r0.hotkey_ok === false;
      if (show && taken.hidden) setTimeout(() => App.$("#hkSave")?.focus(), 0);
      taken.hidden = !show;
    }
    const diag = App.$("#recDiag");
    if (diag) diag.textContent = r0 ? App.t("adv.diagFmt", { n: r0.restarts, enc: r0.encoder || "?", cap: r0.capture || "?", rate: r0.kbps ? (r0.kbps / 1000).toFixed(1) + " Mb/s" : "?" }) : "—";
    const bar = App.$("#recState");
    if (!bar || !bar._refs) return;
    const { dot, title, line, saveBtn, toggleBtn, longBtn } = bar._refs;
    const r = S.recorder;
    const lg = r && r.long && r.long.active ? r.long : null;
    const clock = (sec) => `${sec >= 3600 ? Math.floor(sec / 3600) + ":" : ""}${String(Math.floor(sec / 60) % 60).padStart(2, "0")}:${String(sec % 60).padStart(2, "0")}`;
    if (paint.wasLong && !lg) setTimeout(() => App.Lib.load(), 6000);   // the finished MP4 appears after the remux
    paint.wasLong = !!lg;
    const down = r ? r.sources.filter(App.srcFaulty).map((s) => s.name) : [];
    let state = "on", t = App.t("st.on"), l = "";
    if (!r) { state = "off"; t = App.t("st.off"); l = App.t("st.offL"); }
    else if (r.idle) { state = "idle"; t = App.t("st.idle"); l = App.t("st.idleL"); }
    else if (!r.recording) { state = "off"; t = App.t("st.stopped"); l = r.last_error || ""; }
    else if (down.length) { state = "warn"; t = App.t("st.warn"); l = App.t("st.down", { list: down.join(", ") }); }
    if (lg) { state = "on"; t = App.t("long.on"); l = `${clock(lg.elapsed_s || 0)} · ${lg.mb} MB`; }
    else if (r && r.long && r.long.finalizing) t = App.t("long.fin");
    const left = rollLeft(r);
    if (left && !lg) { t = App.t("rec.pill.saving"); l = App.t("roll.left", { n: left }); }
    line.title = left ? App.t("roll.again") : "";
    if (r && !l) l = r.hotkey ? App.t("st.buffer", { cur: fmtBuf(r.buffer_s), target: fmtBuf(r.buffer_target_s), hk: String(r.hotkey).replaceAll("+", " + ") })
      : App.t("st.bufferNoKey", { cur: fmtBuf(r.buffer_s), target: fmtBuf(r.buffer_target_s) });
    dot.className = `sdot ${state}`;
    title.textContent = t;
    line.textContent = l;
    saveBtn.disabled = !r || (!r.recording && !r.idle);
    longBtn.disabled = !r || (!r.recording && !r.idle && !lg);
    longBtn.textContent = lg ? App.t("long.stop") : App.t("long.start");
    toggleBtn.textContent = r ? App.t("st.stop") : App.t("st.start");
    toggleBtn.title = r ? App.t("st.stopTip") : "";
    if (sumLoad && sumLoad.text.isConnected) {
      // measured while the recorder runs (bars, exact values in the tooltip); before that, the estimate from the encoder
      if (r && r.cpu_pct != null) {
        const paintBar = (bar, v) => {
          const lit = Math.min(10, Math.ceil(v / 10));
          [...bar.children].forEach((b, i) => {
            b.style.opacity = i < lit ? 0.95 : 0.15;
            b.style.background = v >= 60 ? "var(--warn)" : "var(--acc)";
          });
        };
        paintBar(sumLoad.cpu, r.cpu_pct);
        paintBar(sumLoad.gpu, r.gpu_pct ?? 0);
        const pc = (v) => (v < 10 ? v.toFixed(1) : String(Math.round(v)));
        sumLoad.row.title = App.t("sum.loadTip") + "\nCPU " + pc(r.cpu_pct) + "%" + (r.gpu_pct != null ? " \u00B7 GPU " + pc(r.gpu_pct) + "%" : "");
        sumLoad.meters.hidden = false;
        sumLoad.text.hidden = true;
      } else {
        sumLoad.meters.hidden = true;
        sumLoad.text.hidden = false;
        sumLoad.text.textContent = hw() ? "GPU" : "CPU";
      }
    }
    // while the recorder is idle its sources are not connected, so the names it reports are from the last
    // session; show the Windows default as it is right now (EarTrumpet and friends switch it at any time)
    const liveRec = !!(r && r.recording);
    if (Date.now() - defaultsAt > 2500) {   // also while recording: the dropdown below shows the default too
      defaultsAt = Date.now();
      api.invoke("recorder_devices").then((d) => {
        if (d && d.default) { devices = { ...devices, ...d }; paint(); }
      }).catch(() => {});
    }
    for (const row of document.querySelectorAll("#recSources .aitem")) {
      const src = r && r.sources.find((s) => s.name === row.dataset.src);
      const dev = row.querySelector(".dev");
      // the open dropdown's "Windows default device (now: ...)" entry follows the default as well
      const pick = SRC.find((x) => x[1] === row.dataset.src);
      const followOpt = pick && row.querySelector('select option[value="' + FOLLOW + '"]');
      if (followOpt) {
        const list = pick[3] === "render" ? devices.render : devices.capture;
        const defName = nameOf(list, defaultFor(pick[3], pick[0]));
        const text = App.t("dev.default") + (defName ? " " + App.t("dev.now", { name: defName }) : "");
        if (followOpt.textContent !== text) followOpt.textContent = text;
      }
      if (dev && !liveRec) {
        const def = SRC.find((s) => s[1] === row.dataset.src);
        if (def) {
          const d = deviceLabel(cfg, def[0], def[3]);
          dev.replaceChildren(...[d.text, d.note ? h("em", { text: d.note }) : null].filter(Boolean));
        }
        continue;
      }
      if (!src || !dev) continue;
      const base = dev.firstChild ? dev.firstChild.textContent : "";
      const st = srcState(src);
      const name = st.name || (src.role === "appx" ? App.t("src.allBut", { app: appName(src.device) }) : src.role === "app" && src.device ? appName(src.device) : src.device) || base;
      const note = st.note ? h("em", { class: App.srcFaulty(src) ? "bad" : "", text: st.note, title: st.tip || null }) : null;
      dev.replaceChildren(...[name, note].filter(Boolean));   // replaceChildren(null) would print "null"
    }
  }

  /** What a source's status code means, in words: the note after its device name, and a device name to show instead
   *  when it waits for one. The recorder sends codes (`wait_device:<name>`, `wait_app:<stem>`, ...), never text. */
  function srcState(src) {
    if (src.live) return { note: src.role === "secondary" ? App.t("src.backup") : "" };
    const s = String(src.status || ""), i = s.indexOf(":");
    const code = i < 0 ? s : s.slice(0, i), arg = i < 0 ? "" : s.slice(i + 1);
    switch (code) {
      case "wait_device": return { note: App.t("src.waitDev"), name: arg };
      case "wait_app": return { note: App.t("src.waitApp", { app: appName(arg) }), tip: App.t("src.waitTip") };
      case "wait_game": return { note: App.t("src.waitGame") };
      case "no_device": return { note: App.t("src.noDevice"), tip: App.t("src.noDeviceTip") };
      case "starting": return { note: App.t("src.starting") };
      default: return { note: App.t("src.error"), tip: arg || s };
    }
  }

  /* ───────────── first-run wizard ───────────── */

  /* Four steps, about a minute, no permission prompt: Start (everything detected, shown as values), Sound (the three
   * tracks with live meters), Picture (three profiles; its button starts recording) and Shortcut (press it, see the
   * clip that was really saved). The step is remembered while the wizard is unfinished, so a closed window resumes there. */
  const STEPS = ["start", "audio", "picture", "key"];
  const KEY_STEP = 3;

  async function wizard() {
    await load();
    const found = await api.invoke("detect_clip_folders").catch(() => []);
    const others = found.filter((f) => f.source !== "Buffer");     // clip folders of other programs
    const discord = await api.invoke("discord_installed").catch(() => false);
    const fresh = !App.P.settings.onboarded;
    const resumeAt = fresh ? Math.min(KEY_STEP, Math.max(0, +App.P.settings.wizStep || 0)) : 0;
    const w = {
      step: resumeAt, dir: cfg.output_dir, cfg: JSON.parse(JSON.stringify(cfg)), autostart: true, adv: false,
      checks: [], checksDone: resumeAt > 0, running: false, hits0: null, saved0: null, tested: false, clip: null, editKey: false,
    };
    // First choices: follow the Windows default output, a dedicated "chat" endpoint if there is one, the default mic.
    const hasChat = devices.render.find((d) => /chat|czat|communic/i.test(d.name));
    const follow = { id: FOLLOW, name: App.t("dev.default") };
    if (!w.cfg.game.primary) w.cfg.game.primary = follow;
    if (!w.cfg.chat.primary) w.cfg.chat.primary = hasChat ? { id: hasChat.id, name: hasChat.name } : follow;
    if (!w.cfg.mic.primary) w.cfg.mic.primary = follow;
    const presets = presetsFor(w.cfg);
    // the recommended profile: Balanced with a GPU encoder, Light on the processor (x264 is capped at 1080p30 anyway)
    const recId = tools && tools.encoder && tools.encoder !== "libx264" ? "bal" : "eco";
    // a resumed wizard already saved the user's picks: only a new start takes the recommended profile and the chat split
    if (fresh && !resumeAt) {
      Object.assign(w.cfg, presets.find((p) => p[0] === recId)[3]);
      // One shared output is the common case: split a chat app out of the game track by default.
      // only when Discord is installed: otherwise the Chat track would wait for a program that never starts
      if (sharedOutput(w.cfg) && !w.cfg.chat_app && discord) w.cfg.chat_app = "discord";
    }

    const overlay = h("div", { class: "wz" });
    document.body.append(overlay);
    let stopMeters = () => {};
    let statusTimer = 0;
    // it can always be closed (✕ or Esc): every step already saved its picks, and a first run resumes at that step next time
    const onEsc = (e) => { if (e.key === "Escape") close(); };
    const close = () => { stopMeters(); clearInterval(statusTimer); document.removeEventListener("keydown", onEsc); overlay.remove(); };
    document.addEventListener("keydown", onEsc);
    const presetOf = () => presets.find((p) => p[3].width === w.cfg.width && p[3].fps === w.cfg.fps && p[3].quality === w.cfg.quality);
    const lenText = (s) => (LEN().find(([v]) => v === s) || [0, fmtBuf(s)])[1];

    /* ----- pieces ----- */
    const rail = () => h("div", { class: "wzrail" }, STEPS.map((step, i) => h("div", { class: "wzstep" },
      h("div", { class: "bars" }, Array.from({ length: 10 }, (_, j) => h("i", { style: {
        opacity: i < w.step ? 0.45 : i === w.step ? (0.4 + j * 0.066).toFixed(2) : 0.1, transitionDelay: `${j * 0.03}s` } }))),
      h("span", { text: App.t("wz.step." + step), style: { color: i === w.step ? "#c6f432" : i < w.step ? "#8e9484" : "#6f7566" } }))));

    const bigLogo = () => {
      const tails = [30, 10, 38, 18, 26, 6, 34, 14, 40, 22, 8, 32, 16];
      return h("div", { class: "biglogo" }, tails.map((t, i) => {
        const d = Math.abs(i - 6), wd = 10 + (6 - d) * 11, dl = `${i * 0.045}s`;
        return h("div", { class: "l" },
          h("div", { class: "t" }, h("i", { style: { width: `${t * 0.8}px`, animationDelay: dl } })),
          h("i", { class: "w", style: { width: `${wd}px`, animationDelay: dl } }));
      }));
    };

    /* Step 1: nothing to answer. What was detected is shown as values; each row explains itself in its tooltip. */
    const stepStart = () => {
      const enc = tools && tools.encoder;
      const gpu = !!(enc && enc !== "libx264");
      const nDev = (devices.render.length ? 2 : 0) + (devices.capture.length ? 1 : 0);
      const mon = monitors.find((m) => m.index === w.cfg.monitor) || monitors.find((m) => m.primary) || monitors[0];
      // [label, value, note, tooltip, ok]
      const rows = [
        tools && !tools.ffmpeg_found ? ["ffmpeg", App.t("wz.noFfmpeg"), "", "", false] : null,
        [App.t("adv.encoder"), gpu ? encoderName() : App.t("enc.cpu"), App.t(gpu ? "wz.gpu" : "wz.cpu"), App.t("wz.encTip"), gpu],
        [App.t("wz.audio"), App.t("wz.tracks"), App.t("wz.nOfM", { n: nDev, m: 3 }), App.t("wz.soundTip"), nDev >= 3],
        mon ? [App.t("wz.monitor"), `${mon.width}×${mon.height}` + (mon.primary ? " · " + App.t("wz.main") : ""),
          monitors.length > 1 ? App.t("wz.nOfM", { n: 1, m: monitors.length }) : "", App.t("wz.monTip"), true] : null,
        others.length ? [App.t("wz.oldClips"), [...new Set(others.map((f) => f.source))].join(", "), App.t("wz.oldAdd"), App.t("wz.oldTip"), true] : null,
      ].filter(Boolean);
      const els = rows.map(([label, , , tip]) => {
        const dot = h("span", { class: "cdot" }), val = h("span", { class: "mono cval" }), note = h("span", { class: "cnote" });
        return { dot, val, note, row: h("div", { class: "crow wait", title: tip }, dot, h("span", { class: "clabel", text: label }), val, note) };
      });
      const count = h("span", { class: "mono faint" });
      const paintRow = (i) => {
        const st = w.checks[i] || { state: w.checksDone ? (rows[i][4] ? "ok" : "warn") : "wait" };
        const e = els[i];
        e.row.className = `crow ${st.state}`;
        e.dot.textContent = st.state === "ok" ? "✓" : st.state === "warn" ? "!" : "";
        e.val.textContent = st.state === "active" ? App.t("wz.checking") : st.state === "wait" ? "" : rows[i][1];
        e.note.textContent = st.state === "ok" || st.state === "warn" ? rows[i][2] : "";
        count.textContent = App.t("wz.checked", { n: w.checksDone ? rows.length : w.checks.filter((c) => c && c.state !== "active").length, m: rows.length });
      };
      const run = async () => {
        if (w.checksDone || w.running) return;
        w.running = true;
        for (let i = 0; i < rows.length; i++) {
          w.checks[i] = { state: "active" };
          paintRow(i);
          await sleep(380);
          if (!overlay.isConnected) return;
          w.checks[i] = { state: rows[i][4] ? "ok" : "warn" };
          paintRow(i);
        }
        w.checksDone = true;
        w.running = false;
        if (w.step === 0) renderFooter();
      };
      els.forEach((_, i) => paintRow(i));
      if (!w.checksDone) setTimeout(run, 700);          // let the logo build first
      return h("div", { class: "stack16" },
        h("div", { class: "wzhero" }, bigLogo(),
          h("div", { class: "wzhello" }, h("div", { class: "big", text: App.t("wz.hello") }),
            h("div", { class: "muted", text: App.t("wz.helloD") }))),
        h("div", { class: "stack8" }, h("div", { class: "between" }, U.cap(App.t("wz.found")), count), els.map((e) => e.row)),
        h("div", { class: "between faint" }, h("span", { text: App.t("wz.noInstall") }), App.i18n.hintLink()));
    };

    /* Step 2: the three tracks, already picked, with live meters. */
    const stepAudio = () => {
      // a pick only updates the notes (the meters read w.cfg each round), so the step does not replay its animation
      const notes = {};
      const bleed = bleedWarn();
      const paintNotes = () => {
        for (const [key, , , kind] of SRC) {
          // the chat app split is said in the Chat row itself: its sound goes to a track of its own
          const split = key === "chat" && w.cfg.chat_app;
          const n = split ? deviceLabel(w.cfg, key, kind).text : deviceLabel(w.cfg, key, kind).note;
          notes[key].textContent = n;
          if (split) notes[key].title = App.t("wz.splitTip", { app: appName(w.cfg.chat_app) });
          else notes[key].removeAttribute("title");     // an empty title would hide the row's own tooltip
          notes[key].hidden = !n;
        }
      };
      const rows = SRC.map(([key, name, label, kind, color, tip], i) => {
        notes[key] = h("em", { class: "wznote" });
        return h("div", { class: "wzaudio", "data-src": name, title: App.t(tip), style: { animationDelay: `${0.1 + i * 0.1}s` } },
          h("span", { class: "sq", style: { background: color } }), h("b", { text: App.t(label) }),
          deviceSelect(key, kind, w.cfg[key].primary, (v) => { w.cfg[key] = { ...w.cfg[key], primary: v }; paintNotes(); }, false),
          notes[key], meterEl(color, true));
      });
      paintNotes();
      setTimeout(() => { if (w.step === 1) stopMeters = pollMeters(() => w.cfg, ".wzbody", 140, 40, bleed.set); }, 400);
      return h("div", { class: "stack12" },
        h("div", null, h("div", { class: "big", text: App.t("wz.audioQ") }),
          h("div", { class: "muted", text: App.t("wz.audioD") })),
        ...rows, bleed.el,
        h("div", { class: "faint", text: App.t("wz.audioHint") }));
    };

    /* Step 3: three profiles (the recommended one marked), how much to keep and what one clip weighs. The rest is folded. */
    const stepPicture = () => {
      const cardsBox = h("div", { class: "presets3" });
      const lenBox = h("div");
      const sizeVal = h("span", { class: "mono" }), sizeNote = h("span", { class: "muted", title: App.t("wz.fitsTip") });
      const advBox = h("div");
      const refresh = () => {
        put(lenBox, U.srow(App.t("v.len"), U.segc(LEN(), w.cfg.buffer_seconds, (v) => { w.cfg.buffer_seconds = +v; refresh(); drawAdv(false); })));
        const cur = presetOf();
        [...cardsBox.children].forEach((c, i) => c.classList.toggle("on", cur === presets[i]));
        const mb = estimates(w.cfg).clipMb;
        sizeVal.textContent = "≈ " + fmtMb(mb);
        sizeNote.textContent = App.t(mb <= 10 ? "wz.fitsD" : mb <= 50 ? "wz.fitsB" : "wz.over");
      };
      presets.forEach((p, i) => {
        cardsBox.append(h("div", { class: "pcard", style: { animationDelay: `${0.05 + i * 0.07}s` }, onClick: () => { Object.assign(w.cfg, p[3]); refresh(); drawAdv(false); } },
          h("b", null, App.t(p[1]), p[0] === recId ? h("span", { class: "recbadge", text: App.t("wz.recommended") }) : null),
          h("div", { class: "muted", text: App.t(p[2]) }), h("div", { class: "mono spec", text: p[4] || `${App.t("res.native").toLocaleLowerCase(App.tag())} · ${p[3].fps} fps` })));
      });
      // "Dostosuj": folder, games only, post-roll, resolution, fps, quality, screen and the encoder
      const drawAdv = (anim) => {
        const set = (patch) => { Object.assign(w.cfg, patch); refresh(); drawAdv(false); };
        const roll = U.srow(App.t("key.roll"), U.segc(POST_ROLL(), w.cfg.post_roll_seconds, (v) => set({ post_roll_seconds: +v })));
        roll.title = App.t("key.rollTip");
        const dirEl = h("span", { class: "mono path", text: w.dir });
        put(advBox,
          h("button", { class: "advlink", "aria-expanded": String(w.adv), text: `${App.t("wz.customize")} ${w.adv ? "▴" : "▾"}`, onClick: () => { w.adv = !w.adv; drawAdv(true); } }),
          w.adv ? h("div", { class: "card2 compact" + (anim ? " rise" : "") },
            U.srow(App.t("wz.folder"), h("div", { class: "pathrow" }, dirEl, h("button", { class: "linkbtn", text: App.t("common.change"), onClick: async () => {
              const d = await api.pickFolder();
              if (d) { w.dir = d; dirEl.textContent = d; }
            } }))),
            U.trow(App.t("beh.gameOnly"), App.t("beh.gameOnlyD"), w.cfg.game_only, (v) => set({ game_only: v })),
            roll,
            ...pictureRows(w.cfg, set),
            U.srow(App.t("adv.encoder"), h("select", { class: "field", title: App.t("adv.encoderTip"), onChange: (e) => { w.cfg.encoder = e.target.value; } },
              ENCODERS().map(([v, t]) => h("option", { value: v, text: t, selected: (w.cfg.encoder || "auto") === v })))))
            : h("div", { class: "faint", text: App.t("wz.customizeD") }));
      };
      refresh(); drawAdv(false);
      return h("div", { class: "stack12" },
        h("div", null, h("div", { class: "big", text: App.t("wz.picQ") }), h("div", { class: "muted", text: App.t("wz.picRec") })),
        cardsBox,
        h("div", { class: "card2" }, lenBox, U.srow(App.t("sum.clip"), h("div", { class: "pathrow" }, sizeVal, sizeNote))),
        advBox);
    };

    /* Step 4, the finish line: recording already runs; press the shortcut and see the clip that was really saved. */
    const stepKey = () => {
      const bufBars = Array.from({ length: 20 }, () => h("i"));
      const bufText = h("span");
      const keysBox = h("div", { class: "wzkeys" });
      const strip = h("div", { class: "keystrip" });
      const clipBox = h("div");
      const drawKeys = () => {
        if (w.editKey) {
          put(keysBox, hotkeyField(w.cfg.hotkey, (c) => { w.cfg.hotkey = c || "Alt+Q"; w.editKey = false; w.tested = false; w.clip = null; paintKey(); drawKeys(); applyKey(); }));
          keysBox.querySelector("input")?.focus();
          return;
        }
        const first = bindParts(w.cfg.hotkey)[0] || "Alt+Q";
        const parts = first.replace(/^Pad:/, "").split("+");
        put(keysBox, ...parts.flatMap((k, i) => [i ? h("span", { class: "plus", text: "+" }) : null, h("kbd", { text: k })]));
      };
      drawKeys();
      w.paintBuf = () => {
        const r = S.recorder;
        const cur = r ? r.buffer_s || 0 : 0, target = r ? r.buffer_target_s || w.cfg.buffer_seconds : w.cfg.buffer_seconds;
        const lit = Math.round(Math.min(1, cur / Math.max(1, target)) * 20);
        bufBars.forEach((b, j) => { b.style.opacity = j < lit ? (0.4 + j * 0.03).toFixed(2) : 0.1; });
        bufText.textContent = r ? App.t("wz.buf", { cur: fmtBuf(cur), target: fmtBuf(target) }) : App.t("wz.starting");
      };
      const editKey = () => { w.editKey = true; drawKeys(); };
      const paintKey = () => {
        const r = S.recorder, left = rollLeft(r);
        // the shortcut is held by another program: say so in amber and open the field, instead of waiting forever
        if (r && r.hotkey_ok === false && !w.clip) {
          if (!strip.classList.contains("warn")) {
            strip.className = "keystrip warn";
            const warn = takenWarn("", editKey); warn.hidden = false;
            strip.replaceChildren(warn);
            if (!w.editKey) editKey();
          }
          return;
        }
        strip.className = "keystrip" + (w.clip ? " ok" : w.tested ? " busy" : "");
        strip.title = !w.clip && left ? App.t("roll.again") : "";
        strip.textContent = w.clip ? "✓ " + App.t("wz.keyOk") : w.tested ? (left ? App.t("wz.saving") + " · " + App.t("roll.left", { n: left }) : App.t("wz.saving")) : App.t("wz.waitKey");
        if (!w.clip) { put(clipBox); return; }
        const thumb = h("div", { class: "wzthumb" });
        const meta = h("span", { class: "mono faint" });
        put(clipBox, h("div", { class: "wzclip" }, thumb,
          h("div", { class: "nm" }, h("span", { class: "mono", text: App.baseName(w.clip) }), meta),
          h("button", { class: "btn", text: App.t("wz.open"), onClick: () => finish(w.clip) })));
        App.Lib.thumbUrl(w.clip).then((u) => { if (u) thumb.style.backgroundImage = `url("${u}")`; });
        api.invoke("probe_clip", { path: w.clip }).then((i) => {
          meta.textContent = App.t("wz.clipMeta", { dur: fmtBuf(i.duration || 0), res: `${i.width}×${i.height} · ${Math.round(i.fps || 0)} fps`, n: (i.audio || []).length });
        }).catch(() => {});
      };
      w.paintKey = paintKey;
      w.paintBuf();
      paintKey();
      return h("div", { class: "stack16" },
        h("div", { class: "wzbuf mono faint" }, h("span", { class: "livedot" }), h("span", { class: "muted", text: App.t("wz.recBg") }),
          h("span", { class: "bufbars" }, bufBars), bufText),
        h("div", { class: "wztry" },
          h("div", { class: "big", text: App.t("wz.tryKey") }),
          keysBox,
          h("div", { class: "muted" }, App.t("wz.keyD", { len: lenText(w.cfg.buffer_seconds) }), " ",
            h("button", { class: "linkbtn lime", text: App.t("common.change"), title: App.t("hk.tip"), onClick: () => { w.editKey = true; drawKeys(); } }))),
        strip, clipBox,
        h("div", { class: "faint", text: App.t("wz.more") }));
    };

    /* ----- shell ----- */
    const body = h("div", { class: "wzbody" });
    const foot = h("div", { class: "wzfoot" });
    const railBox = h("div");
    overlay.append(h("div", { class: "wzcard" }, h("div", { class: "wzsweep" }, h("div")), h("button", { class: "wzx", text: "✕", title: App.t("common.close"), "aria-label": App.t("common.close"), onClick: close }), railBox, body, foot));

    /** Moves to a step. The picks are saved on every move (so a closed window loses nothing) and, while the wizard is
        unfinished, the step too: the next start resumes there. */
    function go(step) {
      w.step = step;
      if (fresh) { App.P.settings.wizStep = step; App.persist(); }
      persist().catch(() => {});
      renderAll();
    }

    function renderFooter() {
      const last = w.step === KEY_STEP;
      const ready = w.step !== 0 || w.checksDone;
      const label = last ? App.t("wz.finish") : w.step === 2 ? App.t("wz.startRec") : App.t("wz.next");
      put(foot,
        last ? h("span", { class: "faint", title: fresh ? App.t("wz.closeOkTip") : "", text: App.t("wz.closeOk") })
          : h("button", { class: "btn ghost", text: App.t("wz.skip"), title: App.t("wz.skipTip"), onClick: () => go(KEY_STEP) }),
        h("span", { class: "sp" }),
        w.step > 0 ? h("button", { class: "btn", text: App.t("wz.back"), onClick: () => go(w.step - 1) }) : null,
        h("button", { class: "btn nextbtn" + (ready ? " acc" : " off"), disabled: !ready, text: label,
          title: w.step === 2 ? App.t("wz.startRecTip") : "",
          onClick: () => { if (!ready) return; if (last) finish(); else go(w.step + 1); } }));
    }

    function renderStep() {
      stopMeters();
      clearInterval(statusTimer);
      const el = h("div", { class: "stepbox stepin" }, [stepStart, stepAudio, stepPicture, stepKey][w.step]());
      body.replaceChildren(el);                // a new node each time restarts the animation
      if (w.step === KEY_STEP) armRecorder();
    }

    function renderAll() { railBox.replaceChildren(rail()); renderFooter(); renderStep(); }

    async function persist() {
      const c = JSON.parse(JSON.stringify(w.cfg));
      c.output_dir = w.dir;
      c.lang = App.i18n.current();
      for (const [k] of SRC) if (c[k].secondary && !c[k].secondary.id) c[k].secondary = null;
      cfg = { ...cfg, ...c };
      await api.invoke("set_recorder_config", { config: c });
    }
    /** Remembers where the shortcut count and the last saved clip stand, so the next save is the user's test. */
    const mark = () => {
      w.hits0 = S.recorder ? S.recorder.hotkey_hits : null;
      w.saved0 = S.recorder ? S.recorder.last_saved || "" : null;
    };
    /** A new keyboard shortcut needs a recorder restart. */
    async function applyKey() {
      await persist();
      // the recorder re-registers shortcuts on `reload` (within half a second), so the buffer is kept
      if (S.recorder) await api.invoke("recorder_cmd", { cmd: "reload" }).catch(() => {});
      else { await api.invoke("recorder_launch").catch(() => {}); await sleep(1500); }
      await sleep(700);
      await App.Rec.refresh();
      mark();
    }
    /* The last step starts recording with the chosen settings, so the shortcut can really be tested. */
    async function armRecorder() {
      await persist();
      if (S.recorder) await api.invoke("recorder_cmd", { cmd: "reload" }).catch(() => {});
      else { await api.invoke("recorder_launch").catch((e) => App.toast(String(e), "err")); await sleep(1500); }
      await App.Rec.refresh();
      if (w.hits0 == null) mark();
      clearInterval(statusTimer);
      if (!overlay.isConnected || w.step !== KEY_STEP) return;   // finished or left the step while the recorder started
      statusTimer = setInterval(async () => {
        if (!overlay.isConnected) { clearInterval(statusTimer); return; }
        await App.Rec.refresh();
        if (w.step !== KEY_STEP || !overlay.isConnected) return;
        const r = S.recorder;
        if (r && w.hits0 == null) mark();      // the recorder came up late
        if (r && w.hits0 != null && r.hotkey_hits > w.hits0 && !w.tested) { w.tested = true; w.paintKey(); }
        // the clip itself: the recorder names it once the save (and its post-roll) is done
        if (r && w.saved0 != null && r.last_saved && r.last_saved !== w.saved0 && !/\.rec\./.test(r.last_saved) && r.last_saved !== w.clip) {
          w.clip = r.last_saved; w.tested = true; w.paintKey();
        } else if (!w.clip) w.paintKey();      // the post-roll countdown, or a shortcut another program holds
        w.paintBuf();
      }, 500);
    }

    /** `open`: the saved clip to open in the editor afterwards. */
    async function finish(open) {
      w.cfg.output_dir = w.dir;
      await persist();
      App.P.libDirs = [...new Set([w.dir, ...found.map((f) => f.path), ...App.P.libDirs.filter((d) => !fresh || d === w.dir)])].filter(Boolean);
      App.P.settings.onboarded = true;
      delete App.P.settings.wizStep;
      App.persist();
      await api.invoke("recorder_autostart", { enable: w.autostart }).catch(() => {});
      if (S.recorder) await api.invoke("recorder_cmd", { cmd: "reload" }).catch(() => {});
      else await api.invoke("recorder_launch").catch((e) => App.toast(String(e), "err"));
      setTimeout(App.Rec.refresh, 1500);
      close();
      App.toast(App.t("wz.toastDone"), "ok");
      await App.Lib.load();
      if (open && S.byPath.has(open)) App.Ed.open(open);
      else if (open) App.go("lib");            // not in the list (yet): the Library shows it with the newest clips
      else if (S.view === "set") App.Settings.render();
    }

    renderAll();
  }

  /** Tells the recorder which language to use for its tray menu and popups (no-op when already in sync). */
  async function syncLang() {
    await load();
    const lang = App.i18n.current();
    // the popup card and the mixer flyout follow the app theme
    const theme = ["oled", "slate"].includes(App.P.settings.theme) ? App.P.settings.theme : "ink";
    if (cfg.lang !== lang || cfg.theme !== theme) await save({ lang, theme });
  }

  return {
    renderTab, generalBlocks, paint, wizard, syncLang, hotkeyField,
    ensureLoaded: load,
    cfg: () => cfg,
    save,
    toolsInfo: () => tools,
  };
})();

