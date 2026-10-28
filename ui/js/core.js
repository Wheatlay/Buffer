"use strict";
/* Shared state, backend bridge and small helpers. Everything hangs off the global `App`. */

const App = {};
(function () {
  const T = window.__TAURI__;
  const invoke = T.core.invoke;

  App.api = {
    invoke,
    src: (p) => T.core.convertFileSrc(p),
    listen: (n, f) => T.event.listen(n, f),
    /** One file, e.g. a game's .exe; `ext` limits the dialog to those extensions. */
    pickFile: async (name, ext) => {
      const r = await T.dialog.open({ multiple: false, filters: [{ name, extensions: ext }] });
      return typeof r === "string" ? r : null;
    },
    pickFolder: async () => {
      const r = await T.dialog.open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    },
    startDrag: (path) => invoke("plugin:drag|start_drag", {
      item: [path], image: App.dragIcon(), onEvent: new T.core.Channel(),
    }),
  };

  /* ───── tiny DOM helper ───── */
  App.h = function h(tag, props, ...kids) {
    const el = document.createElement(tag);
    if (props) {
      for (const [k, v] of Object.entries(props)) {
        if (v == null || v === false) continue;
        if (k === "class") el.className = v;
        else if (k === "style" && typeof v === "object") Object.assign(el.style, v);
        else if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v);
        else if (k === "text") el.textContent = v;
        else if (k === "value") el.value = v;
        else if (k in el && k !== "list") el[k] = v;
        else el.setAttribute(k, v === true ? "" : v);
      }
    }
    for (const kid of kids.flat()) {
      if (kid == null || kid === false) continue;
      el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
    }
    return el;
  };
  const h = App.h;
  App.$ = (s, r = document) => r.querySelector(s);

  /** Rejects when `p` takes longer than `ms`: a hung background process must never make a screen wait for it. */
  App.timeout = (p, ms) => Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error("timeout")), ms))]);

  /** Per-clip track volumes (percent), 100 when the clip has none saved. */
  App.clipGain = (path, name) => ((App.P.clipGains || {})[path] || {})[name] ?? 100;
  App.setClipGain = (path, name, pct) => {
    const all = (App.P.clipGains ||= {});
    const mine = (all[path] ||= {});
    if (pct === 100) delete mine[name]; else mine[name] = pct;
    if (!Object.keys(mine).length) delete all[path];
    App.persist();
  };

  /* ───── formatting ───── */
  App.fmt = (s) => { s = Math.max(0, s || 0); const m = Math.floor(s / 60); return `${m}:${(s - m * 60).toFixed(1).padStart(4, "0")}`; };
  App.fmtS = (s) => { s = Math.round(s || 0); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`; };
  App.fmtMB = (b) => (b >= 1073741824 ? `${(b / 1073741824).toFixed(1)} GB` : `${(b / 1048576).toFixed(b < 10485760 ? 1 : 0)} MB`);
  App.date = (ts) => new Date(ts * 1000).toLocaleDateString(App.tag());
  App.baseName = (p) => p.slice(Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/")) + 1);

  /** `Risk-of-Rain-2__2025-04-22__14-41-40` -> "Risk of Rain 2" */
  const gameCache = new WeakMap();      // clip object -> game (asked for every clip on each sort, filter and render)
  App.gameOf = (clip) => {
    let g = gameCache.get(clip);
    if (g === undefined) {
      // Buffer names the file `Game__date__time`; anything else (SteelSeries Moments' "Moments clip from …", OBS dates)
      // takes the game from its folder, so a folder of foreign clips does not become one "game" per file
      const own = clip.name.includes("__");
      const folder = !own && (clip.group || "").split(/[\\/]/).filter(Boolean).pop();
      const raw = own ? clip.name.split("__")[0] : folder || (/^moments\s*clip/i.test(clip.name) ? "" : clip.name);
      g = raw.replace(/-+/g, " ").replace(/\s+/g, " ").trim();
      // Xbox Game Bar and Nvidia name clips "<Game> 2025.07.08 23.40.22.01 [.DVR Trim]": the game is what comes before the date
      const dated = g.replace(/\s+\d{4}[.\-_]\d{2}[.\-_]\d{2}\b.*$/, "").trim();
      if (dated) g = dated;
      // clips saved with no game in front are named `Desktop__…` by the recorder: shown in the UI language, file name unchanged
      if (own && g === "Desktop") g = App.t("lib.desktop");
      gameCache.set(clip, g);
    }
    return g || App.t("clip.unknown");
  };
  /** What the user sees as the clip's name: their own title, else the game derived from the file name. */
  App.titleOf = (clip) => App.customTitle(clip) || App.gameOf(clip);
  /** The clip's own name (empty until the user sets one): the part after the time stamp in the file name
   *  (`Game__2026-10-14__22-38-28__Title`), else a title saved by an earlier version. The game is never edited here. */
  App.customTitle = (clip) => {
    const m = clip.name.match(/__\d{4}-\d{2}-\d{2}__\d{2}-\d{2}-\d{2}__(.+)$/);
    return m ? m[1] : App.P.titles[clip.path] || "";
  };

  /** Renames the file on disk and carries everything saved for the old path (favourite, trim, gains, export links)
   *  over to the new one. Resolves with the new path, or null when it failed. */
  App.renameClip = async (path, title) => {
    try {
      const to = await invoke("rename_clip", { path, title });
      if (to !== path) {
        const P = App.P, S = App.S;
        for (const k of ["favs", "titles", "trims", "clipGains", "meta"]) {
          if (P[k] && path in P[k]) { P[k][to] = P[k][path]; delete P[k][path]; }
        }
        delete P.titles[to];                       // the name now lives in the file
        P.newSet = P.newSet.map((p) => (p === path ? to : p));
        for (const e of P.exports) if (e.source === path) e.source = to;
        if (S.sel.delete(path)) S.sel.add(to);
        if (S.active === path) S.active = to;
        App.persist();
      }
      delete App.P.titles[path];
      App.persist();
      await App.Lib.load();
      const cur = App.Ed && App.Ed.current();
      if (cur && cur.clip.path === path) await App.Ed.open(to);   // reload the editor on the renamed file
      return to;
    } catch (e) {
      App.toast(App.t("rn.fail", { err: e }), "err");
      return null;
    }
  };
  App.whenOf = (clip) => new Date(clip.modified * 1000).toLocaleString(App.tag(), { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

  /** A small pencil drawn next to a name that can be clicked to edit it. Only a hint: the name itself is the control. */
  App.pencil = () => {
    const el = h("span", { class: "pen", "aria-hidden": "true" });
    el.innerHTML = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/></svg>';
    return el;
  };

  /** Turns `el` into a text field to rename the clip; Enter/blur saves, Esc cancels, empty resets. */
  App.editTitle = (el, path, done) => {
    const clip = App.S.byPath.get(path);
    if (!clip) return;
    const input = h("input", { class: "field titlein", value: App.customTitle(clip), placeholder: App.t("clip.namePh"), maxLength: 80, spellcheck: false });
    let finished = false;
    const finish = (save) => {
      if (finished) return;
      finished = true;
      const v = input.value.trim();
      if (save && v !== App.customTitle(clip)) {
        // plain files without a recorder name cannot lose their name: an empty title is ignored
        if (!v && !/__\d{4}-\d{2}-\d{2}__\d{2}-\d{2}-\d{2}/.test(clip.name)) { done(); return; }
        App.renameClip(path, v).then(() => done());
        return;
      }
      done();
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
    });
    input.addEventListener("blur", () => finish(true));
    el.replaceWith(input);
    input.focus();
    input.select();
  };

  App.hueOf = (s) => { let x = 0; for (const c of s) x = (x * 31 + c.charCodeAt(0)) >>> 0; return x % 360; };

  /* ───── persisted state (debounced) ───── */
  const DEFAULTS = {
    settings: {
      theme: "ink", anim: true, lite: false, thumb: "M",
      sub: true, hw: true,
      normDef: false, drag: true, defTarget: 10, outDir: "", onboarded: false, lang: "en", langHint: false, presets: [10, 50, 500], autoCleanDays: 7,
      // sidebar: modules hidden with a right click, and modules opened at least once (the rest carry a "new" dot)
      navHidden: [], navSeen: {},
    },
    libDirs: [],
    favs: {}, titles: {}, trims: {}, clipGains: {}, newSet: [], libSeenAt: 0, exports: [], meta: {}, gains: { Game: 100, Chat: 100, Mic: 100 },
  };
  App.P = structuredClone(DEFAULTS);

  let saveTimer = 0;
  App.persist = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => invoke("save_state", { state: App.P }).catch(() => {}), 400);
  };
  App.loadPersisted = async () => {
    try {
      const s = await invoke("load_state");
      App.fresh = Object.keys(s).length === 0;
      // v2 of the look: the brand lime replaces the old orange default (only if it was never changed).
      if (s.settings && s.settings.brand !== 2) {
        if (s.settings.accent === "#ff6a1f") s.settings.accent = "#c6f432";
        s.settings.brand = 2;
      }
      // installs from before the "new" dots already know their modules: no dot for them
      if (s.settings && s.settings.onboarded && !s.settings.navSeen) s.settings.navSeen = { eq: 1, snd: 1, rgb: 1, fans: 1 };
      App.P = { ...DEFAULTS, ...s, settings: { ...DEFAULTS.settings, ...(s.settings || {}) },
        gains: { ...DEFAULTS.gains, ...(s.gains || {}) } };
    } catch { /* first run */ }
  };

  /* ───── runtime state ───── */
  App.S = {
    view: "lib", clips: [], byPath: new Map(), cat: "all", q: "", sort: "newest",
    sel: new Set(), active: null, listError: "",
    exporting: null,           // { name, pct }
    recorder: null,            // last status from the recorder, or null if it is not running
  };

  /* ───── theme ───── */
  App.applyTheme = () => {
    const s = App.P.settings, r = document.documentElement;
    // Two themes only (Ink, OLED); the brand lime is fixed. Older saved themes fall back to Ink.
    r.dataset.theme = s.theme === "oled" ? "oled" : "";
    r.dataset.anim = s.anim ? "on" : "off";
  };

  /* ───── "NOWE" clips: added since the library was last opened, until opened themselves ───── */
  App.isNew = (path) => App.P.newSet.includes(path);
  App.markSeen = (path) => {
    const i = App.P.newSet.indexOf(path);
    if (i >= 0) { App.P.newSet.splice(i, 1); App.persist(); }
  };
  App.computeNew = (clips) => {
    if (App.newComputed) return;
    App.newComputed = true;
    const known = new Set(clips.map((c) => c.path));
    const keep = App.P.newSet.filter((p) => known.has(p));
    const since = App.P.libSeenAt;
    if (since) for (const c of clips) if (c.modified > since && !keep.includes(c.path)) keep.push(c.path);
    App.P.newSet = keep.slice(-200);
    App.P.libSeenAt = Math.floor(Date.now() / 1000);
    App.persist();
  };

  /** Animated logo trail + message, used for empty and loading states. `more`: further buttons, in one row after the
   *  main action (a way out next to "try again"). */
  App.emptyState = (title, sub, actionLabel, onAction, more = null) => {
    const tails = [30, 10, 38, 18, 26, 6, 34, 14, 40];
    const rows = tails.map((t, i) => {
      const w = 12 + (4 - Math.abs(i - 4)) * 14;
      return h("div", { class: "l" },
        h("div", { class: "t" }, h("i", { style: { width: `${t}px`, animationDelay: `${i * 0.07}s` } })),
        h("div", { class: "w" }, h("i", { style: { width: `${w}px` } })));
    });
    const main = actionLabel ? h("button", { class: "btn", text: actionLabel, onClick: onAction }) : null;
    const extra = (more || []).filter(Boolean);
    return h("div", { class: "empty" }, h("div", { class: "lines" }, rows),
      h("b", { text: title }), sub ? h("div", { class: "muted", text: sub }) : null,
      extra.length ? h("div", { class: "acts" }, main, ...extra) : main);
  };

  /* ───── settings building blocks (used by views.js and recording.js) ───── */
  App.ui = {
    cap: (t) => h("div", { class: "cap", text: t }),
    card: (...kids) => h("div", { class: "card2" }, ...kids),
    srow: (label, ctrl) => h("div", { class: "srow" }, h("span", { class: "lb", text: label }), ctrl),
    segc: (items, cur, on) => h("div", { class: "seg" }, items.map(([v, t]) =>
      h("button", { class: String(cur) === String(v) ? "on" : "", text: t, onClick: () => on(v) }))),
    trow: (title, desc, on, set) => h("div", { class: "trow", onClick: () => set(!on) },
      h("div", null, h("div", { class: "tt", text: title }), desc ? h("div", { class: "ds", text: desc }) : null),
      h("span", { class: "toggle lg" + (on ? " on" : "") })),
    /** A button for something hard to take back (an elevated uninstall): the first click turns it into "Sure?" for a
     *  few seconds, only a second click runs `run(button)`. */
    sure: (label, tip, run) => {
      let armed = 0;
      const b = h("button", { class: "btn", text: label, title: tip || null, onClick: () => {
        if (b.disabled) return;
        if (!armed) {
          b.textContent = App.t("common.sure"); b.classList.add("warn");
          armed = setTimeout(() => { armed = 0; b.textContent = label; b.classList.remove("warn"); }, 3500);
          return;
        }
        clearTimeout(armed); armed = 0;
        b.textContent = label; b.classList.remove("warn"); b.disabled = true;
        run(b);
      } });
      return b;
    },
  };

  /** Words for an error of the RGB / Fans install (ml-core `elevate`): those send short codes (`hash_mismatch`,
   *  `install_failed:12`, ...). A declined UAC prompt is the user's own "no", a quiet cancel: "" (show nothing).
   *  Anything else (a raw system message) is passed through. */
  App.installError = (e) => {
    const s = String(e && e.message ? e.message : e), i = s.indexOf(":");
    const code = i < 0 ? s : s.slice(0, i), n = i < 0 ? "" : s.slice(i + 1);
    if (code === "uac_declined") return "";
    if (code === "install_failed" || code === "uninstall_failed") return App.t("inst." + code, { code: n });
    const known = ["download_failed", "hash_mismatch", "hash_failed", "pawnio_check", "pawnio_failed", "unpack_failed", "task_failed", "not_in_place", "interrupted", "no_build"];
    return known.includes(code) ? App.t("inst." + code) : s;
  };

  /* ───── toast ───── */
  /** `action` = { label, run }: a button in the toast (e.g. "Undo"); the toast then stays a little longer */
  App.toast = (msg, kind = "", action = null) => {
    const box = App.$("#toast");
    const t = App.h("div", { class: `t ${kind}`, text: msg });
    if (action) t.append(App.h("button", { class: "tact", text: action.label, onClick: () => { t.remove(); action.run(); } }));
    box.append(t);
    setTimeout(() => t.remove(), kind === "err" ? 7000 : action ? 7000 : 3500);
  };

  /* An uncaught error would otherwise only reach the devtools console: say so, at most once per 10 s. */
  let lastUiErr = 0;
  const uiError = (e) => {
    const msg = String((e && (e.message || e.reason?.message || e.reason)) || e);
    console.error("[ui]", e);
    if (Date.now() - lastUiErr < 10000 || !App.t) return;
    lastUiErr = Date.now();
    try { App.toast(App.t("err.ui", { err: msg.slice(0, 160) }), "err"); } catch { /* toast host not ready */ }
  };
  /* "ResizeObserver loop ..." is the browser noting that a layout settled over two frames: harmless, nothing broke. */
  window.addEventListener("error", (ev) => {
    if (/^ResizeObserver loop/.test(ev.message || "")) { ev.stopImmediatePropagation(); return; }
    uiError(ev.error || ev.message);
  });
  window.addEventListener("unhandledrejection", (ev) => uiError(ev.reason));

  /* ───── helpers shared by several views ───── */
  App.dragIcon = (() => {
    let url = "";
    return () => {
      if (url) return url;
      const c = document.createElement("canvas");
      c.width = c.height = 64;
      const g = c.getContext("2d");
      g.fillStyle = "#c6f432";
      g.beginPath(); g.roundRect(4, 4, 56, 56, 12); g.fill();
      g.fillStyle = "#111";
      g.beginPath(); g.moveTo(25, 18); g.lineTo(25, 46); g.lineTo(47, 32); g.closePath(); g.fill();
      return (url = c.toDataURL("image/png"));
    };
  })();

  /** Logo 5a "buffer": a play triangle made of scan-lines, each trailing a faded tail. */
  App.logo = (size = 36, color = "#c6f432") => {
    const tails = [30, 10, 38, 18, 26, 6, 34, 14, 40, 22, 8, 32, 16];
    const NS = "http://www.w3.org/2000/svg";
    const W = 44 + 84, H = 13 * 6 + 12 * 3;
    const svg = document.createElementNS(NS, "svg");
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`);
    svg.setAttribute("width", size);
    svg.setAttribute("height", Math.round((size * H) / W));
    svg.setAttribute("aria-hidden", "true");
    for (let i = 0; i < 13; i++) {
      const w = 12 + (6 - Math.abs(i - 6)) * 12, y = i * 9;
      const tail = document.createElementNS(NS, "rect");
      tail.setAttribute("x", 44 - tails[i]); tail.setAttribute("y", y);
      tail.setAttribute("width", tails[i]); tail.setAttribute("height", 6); tail.setAttribute("rx", 3);
      tail.setAttribute("fill", color); tail.setAttribute("opacity", ".3");
      const bar = document.createElementNS(NS, "rect");
      bar.setAttribute("x", 44); bar.setAttribute("y", y);
      bar.setAttribute("width", w); bar.setAttribute("height", 6); bar.setAttribute("rx", 3);
      bar.setAttribute("fill", color);
      svg.append(tail, bar);
    }
    return svg;
  };

  /** Loads scripts after start-up, run in the given order; each path is fetched only once. */
  const loaded = new Map();
  App.loadScripts = (paths) => Promise.all(paths.map((p) => {
    if (!loaded.has(p)) {
      loaded.set(p, new Promise((res, rej) => {
        const s = Object.assign(document.createElement("script"), { src: p, async: false });
        s.onload = res;
        s.onerror = () => { loaded.delete(p); rej(new Error("failed to load " + p)); };
        document.body.append(s);
      }));
    }
    return loaded.get(p);
  }));

  /** RGB and Fans share code (the fan stage draws with the RGB scene), so they load together, on first use. */
  const HW = ["rgb-wheel", "rgb-auto", "rgb", "rgb-adv", "rgb-calib", "fans", "fans-air", "fans-stage", "fans-wizard", "fans-adv"];
  App.loadHw = () => App.loadScripts(HW.map((n) => `js/${n}.js`));

  /** Run async jobs with a concurrency cap; later calls can be cancelled while still queued. */
  App.queue = (limit) => {
    const waiting = [];
    let running = 0;
    const pump = () => {
      while (running < limit && waiting.length) {
        const job = waiting.shift();
        running++;
        job().finally(() => { running--; pump(); });
      }
    };
    return (fn) => new Promise((res, rej) => {
      waiting.push(() => fn().then(res, rej));
      pump();
    });
  };

  App.exportDirFor = () => {
    const s = App.P.settings;
    if (s.outDir) return s.outDir.replace(/[\\/]+$/, "");
    // no library folder yet: next to where the recorder saves on a fresh install (%USERPROFILE%\Videos\Buffer)
    const base = (App.P.libDirs[0] || App.clipsHome || "").replace(/[\\/]+$/, "");
    return `${base}\\Exported Clips`;
  };

  /** The three export size presets (MB). Saved values are validated so a bad state file can't break the panel. */
  App.presets = () => {
    const p = App.P.settings.presets;
    return Array.isArray(p) && p.length === 3 && p.every((n) => Number.isFinite(n) && n >= 1 && n <= 2000) ? p.map(Math.round) : [10, 50, 500];
  };
  /** Well-known destinations get their name; any other size is just shown in MB. */
  App.presetName = (mb) => ({ 10: "Discord", 50: "Boost" })[mb] || "";

  /** Bitrate maths shared by editor panel and batch export; mirrors ml-core's budget. */
  App.kbps = (targetMb, seconds, speed = 1) =>
    Math.max(0, Math.floor((targetMb * 1048576 * 8 * 0.92) / (seconds / speed) / 1000) - 128);
  App.quality = (kbps) =>
    kbps < 60 ? [App.t("q.bad"), "var(--err)"] : kbps < 300 ? [App.t("q.low"), "var(--warn)"]
      : kbps < 1000 ? [App.t("q.good"), "var(--ok)"] : [App.t("q.great"), "var(--ok)"];
})();

