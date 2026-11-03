"use strict";
/* SoundSwitcher page: Buffer's EarTrumpet. Left, every playback device as a compact card (default first, then the
   chat device, then devices with apps on them) with its volume; right, the apps of the device picked on the left.
   The default device and the switch-output shortcut's cycle are set here. Volume is a smooth lime line that the
   user moves; the scan under it only shows what plays (DESIGN.md). The recorder owns the audio (`mix-*` control
   commands, the same mixer as its tray flyout); this page polls it only while it is open and visible. */

(function () {
  const { h, S, api } = App;
  const U = App.ui;
  const POLL_MS = 100;

  let page = null, list = null, detail = null, opts = null, trayBox = null;
  let timer = 0, busy = false, shapeList = "", shapeDetail = "";
  let devs = [];
  let displayAudio = false;         // an HDMI / DisplayPort output exists: show "keep HDMI awake"
  let sel = "";                     // device shown on the right
  const icons = new Map();          // icon source -> data URL ("" = none, null = loading)
  const waiting = new Map();        // icon source -> fill functions of the boxes waiting for it
  const rows = new Map();           // "dev" / "dev|app" key -> row (see volRow)
  let dragging = null;              // row key whose line is held: the poll leaves its value alone
  let sending = null, queued = null;

  const cmd = (c) => api.invoke("recorder_cmd", { cmd: c });
  const cycle = () => (App.RecUI.cfg() || {}).output_cycle || [];

  /** Volume changes go out one at a time, the latest one wins (a dragged line fires far faster than COM needs). */
  function send(patch) {
    queued = patch;
    if (sending) return;
    const next = () => {
      if (!queued) { sending = null; return; }
      const p = queued; queued = null;
      sending = cmd("mix-set " + JSON.stringify(p)).catch(() => {}).then(next);
    };
    next();
  }

  /** The tray icon's scan-line speaker; waves above the volume fade like the logo's tails. */
  const SPEAKER = (vol, muted) => {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 32 32");
    svg.setAttribute("aria-hidden", "true");
    const waves = vol <= 0 ? 0 : vol < 0.34 ? 1 : vol < 0.67 ? 2 : 3;
    const rect = (x, y, w, cls) => {
      const r = document.createElementNS(ns, "rect");
      Object.entries({ x, y, width: w, height: 3, rx: 0.6 }).forEach(([k, v]) => r.setAttribute(k, v));
      if (cls) r.setAttribute("class", cls);
      svg.append(r);
    };
    [[12, 4], [9, 7], [3, 13], [3, 13], [3, 13], [9, 7], [12, 4]].forEach(([x, w], i) => {
      rect(x, 2 + i * 4, w);
      if (!muted) [[19, 2], [23.5, 1], [28, 0]].forEach(([wx, from], n) => {
        if (i >= from && i <= 6 - from) rect(wx, 2 + i * 4, 2.5, n < waves ? "" : "tail");
      });
    });
    if (muted) {
      const p = document.createElementNS(ns, "path");
      p.setAttribute("d", "M20 10.5l10 11M30 10.5l-10 11");
      p.setAttribute("class", "x");
      svg.append(p);
    }
    return svg;
  };

  function iconOf(src, name) {
    const box = h("span", { class: "sappico" });
    const fill = () => {
      const url = icons.get(src);
      if (url) box.replaceChildren(h("img", { src: url, alt: "" }));
      else box.replaceChildren(h("b", { text: (name || "?").slice(0, 1).toUpperCase() }));
    };
    // the same app on several devices asks once; every box waiting for it is filled when it arrives
    if (src && !icons.has(src)) {
      icons.set(src, null);
      const done = (url) => { icons.set(src, url); (waiting.get(src) || []).forEach((f) => f()); waiting.delete(src); };
      cmd("mix-icon " + src).then((r) => done(r && r.ok ? "data:image/png;base64," + r.png : "")).catch(() => done(""));
    }
    if (src && icons.get(src) === null) waiting.set(src, [...(waiting.get(src) || []), fill]);
    fill();
    return box;
  }

  /** One volume row: mute button (speaker or app icon), optional name, the line with its scan, the number. */
  function volRow(key, target, opt) {
    const segs = Array.from({ length: opt.segs }, () => h("i"));
    const fillEl = h("i", { class: "f" });
    const head = h("b");
    const line = h("div", { class: "sline", role: "slider", tabindex: 0, "aria-label": opt.name, "aria-valuemin": 0, "aria-valuemax": 100 },
      h("i", { class: "t" }), fillEl, h("div", { class: "m" }, ...segs), head);
    const num = h("span", { class: "snum mono" });
    const r = { key, segs, fillEl, head, line, num, v: 0, muted: false, peak: 0, isDev: !target.app, shown: "" };
    const lead = target.app ? iconOf(opt.icon, opt.name) : h("span", { class: "spk" });
    r.lead = lead;
    const mute = h("button", { class: "smute", title: App.t("snd.mute"), "aria-label": App.t("snd.mute") + ": " + opt.name, onClick: (e) => {
      e.stopPropagation();
      send({ ...target, muted: !r.muted });
      r.muted = !r.muted; paintRow(r, r.v, r.muted, 0);
    } }, lead);
    const setV = (v) => { send({ ...target, volume: v }); r.v = v; paintRow(r, v, r.muted, r.peak); };
    const fromEvent = (e) => {
      const box = line.getBoundingClientRect();
      setV(Math.round(Math.min(1, Math.max(0, (e.clientX - box.left) / box.width)) * 100) / 100);
    };
    line.addEventListener("pointerdown", (e) => { e.stopPropagation(); dragging = key; line.setPointerCapture(e.pointerId); line.classList.add("drag"); fromEvent(e); });
    line.addEventListener("pointermove", (e) => { if (dragging === key) fromEvent(e); });
    const release = () => { if (dragging === key) dragging = null; line.classList.remove("drag"); };
    line.addEventListener("pointerup", release);
    line.addEventListener("pointercancel", release);
    line.addEventListener("click", (e) => e.stopPropagation());
    // 5 % per wheel notch, snapped to the grid (as the tray icon)
    line.addEventListener("wheel", (e) => {
      e.preventDefault();
      setV(Math.min(1, Math.max(0, (Math.round(r.v * 20) + (e.deltaY < 0 ? 1 : -1)) * 0.05)));
    }, { passive: false });
    line.addEventListener("keydown", (e) => {
      const d = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[e.key];
      if (!d) return;
      e.preventDefault();
      setV(Math.min(1, Math.max(0, (Math.round(r.v * 20) + d) * 0.05)));
    });
    const name = opt.named ? h("span", { class: "sname" }, h("span", { class: "nm", text: opt.name, title: opt.name }), opt.chip || null) : null;
    r.wrap = h("div", { class: "svrow" + (opt.named ? " named" : "") + (opt.handle ? " movable" : "") }, opt.handle || null, mute, name, line, num);
    rows.set(key, r);
    return r;
  }

  function paintRow(r, v, muted, peak) {
    const pct = `${(v * 100).toFixed(2)}%`;
    r.fillEl.style.width = pct;
    r.head.style.left = pct;
    const level = muted ? 0 : v * peak;
    const n = r.segs.length;
    r.segs.forEach((el, i) => {
      const f = (i + 0.5) / n;
      el.style.opacity = f <= level ? (0.45 + 0.55 * f).toFixed(2) : "0.08";
    });
    r.line.setAttribute("aria-valuenow", Math.round(v * 100));
    r.num.textContent = String(Math.round(v * 100));
    r.wrap.classList.toggle("muted", muted);
    const look = `${muted}|${v <= 0 ? 0 : v < 0.34 ? 1 : v < 0.67 ? 2 : 3}`;
    if (r.isDev && r.shown !== look) {
      r.shown = look;
      r.lead.replaceChildren(SPEAKER(v, muted));
    }
  }

  async function setDefault(id) {
    await cmd("mix-default " + id).catch(() => {});
    shapeList = "";
    poll();
  }

  async function toggleCycle(id) {
    await App.RecUI.ensureLoaded();
    const cur = cycle();
    const next = cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id];
    paintCards(next);                       // shown at once; the save follows
    await App.RecUI.save({ output_cycle: next });
    paintCards();
  }

  /** Selection and cycle chips are painted in place: rebuilding the cards made them blink. */
  const cardEls = new Map();         // device id -> { card, cyc }
  function paintCards(cyc = cycle()) {
    for (const [id, el] of cardEls) {
      el.card.classList.toggle("sel", id === sel);
      el.cyc.classList.toggle("on", cyc.includes(id));
    }
  }

  /** "Firefox, SignalRGB +2": what plays on a device, for its card. */
  const appsText = (d) => {
    const names = d.apps.filter((a) => a.key !== "#system").map((a) => a.name);
    if (!names.length) return d.apps.length ? d.apps[0].name : App.t("snd.noApps");
    return names.slice(0, 2).join(", ") + (names.length > 2 ? ` +${names.length - 2}` : "");
  };

  /** The device cards on the left (rebuilt when a device, the default, the chat device or the cycle changes). */
  function buildList() {
    const cyc = cycle();
    const hid = hiddenIds();
    for (const k of [...rows.keys()]) if (!k.includes("|")) rows.delete(k);
    // a hidden device (never the default) is folded away under the visible ones
    const shown = devs.filter((d) => d.default || !hid.includes(d.id));
    const folded = devs.filter((d) => !d.default && hid.includes(d.id));
    cardEls.clear();
    const cards = shown.map((d) => {
      const master = volRow(d.id, { dev: d.id }, { segs: 24, name: d.name });
      const card = h("div", { class: "card2 scard" + (d.default ? " def" : ""), tabindex: 0, "data-id": d.id,
        "data-drop": App.t(d.default ? "snd.dropDefault" : "snd.dropHere"),
        onClick: () => { if (sel !== d.id) { sel = d.id; paintCards(); buildDetail(); } },
        onKeydown: (e) => { if (e.key === "Enter" && e.target === e.currentTarget) e.currentTarget.click(); } },
        h("div", { class: "shead" },
          h("b", { text: d.name, title: d.name }),
          d.comms ? h("span", { class: "chip", text: App.t("snd.chat") }) : null,
          d.default ? h("span", { class: "chip on", text: App.t("snd.default") }) : null),
        master.wrap,
        h("div", { class: "sfoot" },
          h("span", { class: "sapps", text: appsText(d) }),
          d.default ? null : h("button", { class: "chip click", text: App.t("snd.hide"), title: App.t("snd.hideTip"),
            onClick: (e) => { e.stopPropagation(); setHidden(d.id, true); } }),
          h("button", { class: "chip click", text: App.t("snd.cycle"), title: App.t("key.cycleTip"),
            onClick: (e) => { e.stopPropagation(); toggleCycle(d.id); } })));
      cardEls.set(d.id, { card, cyc: card.querySelector(".sfoot .chip:last-child") });
      return card;
    });
    const fold = folded.length ? h("div", { class: "sfold" + (showHidden ? " open" : "") },
      h("button", { class: "sfoldbtn", onClick: () => { showHidden = !showHidden; buildList(); } },
        h("span", { class: "cap", text: App.t("snd.hidden", { n: folded.length }) }), h("span", { class: "chev", text: showHidden ? "▾" : "▸" })),
      showHidden ? folded.map((d) => h("div", { class: "shid" },
        h("span", { text: d.name, title: d.name }),
        h("button", { class: "chip click", text: App.t("snd.show"), onClick: () => setHidden(d.id, false) }))) : null) : null;
    list.replaceChildren(...cards, ...(fold ? [fold] : []));
    paintCards(cyc);
    paintValues();
  }

  const hiddenIds = () => (App.RecUI.cfg() || {}).hidden_outputs || [];
  let showHidden = false;

  async function setHidden(id, hide) {
    await App.RecUI.ensureLoaded();
    const cur = hiddenIds();
    await App.RecUI.save({ hidden_outputs: hide ? [...new Set([...cur, id])] : cur.filter((x) => x !== id) });
    if (hide && sel === id) sel = "";
    shapeList = shapeDetail = "";
    poll();
  }

  /** The apps of the selected device on the right. */
  /** What the right panel is built from: the device, its apps and their pins, whether it is the default. */
  const sigDetail = () => {
    const d = devs.find((x) => x.id === sel);
    return JSON.stringify([sel, d ? d.apps.map((a) => [a.key, a.pinned_to]) : [], d && d.default, d ? (d.idle || []).map((p) => p.key) : []]);
  };

  /* ───── moving an app to another output: drag its row (grip) onto a device card ───── */

  const GRIP = () => {
    const ns = "http://www.w3.org/2000/svg";
    const svg = document.createElementNS(ns, "svg");
    svg.setAttribute("viewBox", "0 0 8 14");
    svg.setAttribute("aria-hidden", "true");
    for (const [x, y] of [[2, 2], [6, 2], [2, 7], [6, 7], [2, 12], [6, 12]]) {
      const c = document.createElementNS(ns, "circle");
      c.setAttribute("cx", x); c.setAttribute("cy", y); c.setAttribute("r", 1.3);
      svg.append(c);
    }
    return svg;
  };

  /** Short device name for a chip: "Zestaw słuchawkowy (Arctis 5 Chat)" -> "Arctis 5 Chat". */
  const shortDev = (id) => {
    const d = devs.find((x) => x.id === id);
    if (!d) return "?";
    const m = d.name.match(/\(([^)]+)\)/);
    return m ? m[1] : d.name;
  };

  function unpin(key) {
    cmd("mix-unpin " + key).catch(() => {}).then(poll);
  }

  /** The chip after a pinned app's name: pinned here, or pinned elsewhere and waiting for the app to reopen its sound. */
  function pinChip(dev, a) {
    if (!a.pinned) return null;
    const here = a.pinned_to === dev.id;
    return h("button", { class: "chip click pin" + (here ? " on" : ""), text: here ? App.t("snd.pinned") : "\u2192 " + shortDev(a.pinned_to),
      title: App.t(here ? "snd.pinnedTip" : "snd.pendingTip"), onClick: (e) => { e.stopPropagation(); unpin(a.key); } });
  }

  let drag = null;                   // { app, from, ghost, over }
  function startDrag(e, dev, a, iconBox) {
    e.preventDefault();
    e.stopPropagation();
    const ghost = h("div", { class: "sghost" }, iconBox.cloneNode(true), h("span", { text: a.name }));
    document.body.append(ghost);
    drag = { app: a, from: dev.id, ghost, over: null };
    list.classList.add("dragging");
    const move = (ev) => {
      ghost.style.transform = `translate(${ev.clientX + 12}px, ${ev.clientY + 8}px)`;
      const card = document.elementFromPoint(ev.clientX, ev.clientY)?.closest?.(".scard");
      const id = card && card.dataset.id !== drag.from ? card.dataset.id : null;
      if (id !== drag.over) {
        list.querySelectorAll(".scard.drop").forEach((c) => c.classList.remove("drop"));
        if (id) card.classList.add("drop");
        drag.over = id;
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      ghost.remove();
      list.classList.remove("dragging");
      list.querySelectorAll(".scard.drop").forEach((c) => c.classList.remove("drop"));
      const { app, over } = drag;
      drag = null;
      if (over) cmd("mix-move " + JSON.stringify({ app: app.key, to: over }))
        .then((r) => { if (r && !r.ok) App.toast(String(r.error || ""), "err"); })
        .catch((err) => App.toast(String(err), "err"))
        .then(() => { shapeDetail = ""; poll(); });
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    move(e);
  }

  function buildDetail() {
    shapeDetail = sigDetail();
    const d = devs.find((x) => x.id === sel);
    for (const k of [...rows.keys()]) if (k.includes("|")) rows.delete(k);
    if (!d) { detail.replaceChildren(); return; }
    const apps = d.apps.map((a) => {
      // system sounds can't be moved: Windows has no per-app output for them
      const handle = a.system ? h("span", { class: "sgrip off" }) : h("span", { class: "sgrip", title: App.t("snd.dragTip") }, GRIP());
      const r = volRow(d.id + "|" + a.key, { dev: d.id, app: a.key }, { segs: 40, name: a.name, icon: a.icon, named: true, handle, chip: pinChip(d, a) });
      if (!a.system) handle.addEventListener("pointerdown", (e) => startDrag(e, d, a, r.lead));
      return r.wrap;
    });
    // apps pinned here that aren't playing: greyed, with the way back
    const idle = (d.idle || []).map((p) => h("div", { class: "svrow named movable idle" },
      h("span", { class: "sgrip off" }), h("span", { class: "smute" }, iconOf(p.icon, p.name)),
      h("span", { class: "sname" }, h("span", { class: "nm", text: p.name }),
        h("button", { class: "chip click pin", text: App.t("snd.pinned") + " \u00b7 " + App.t("snd.idle"), title: App.t("snd.unpinIdle"),
          onClick: () => unpin(p.key) })),
      h("span"), h("span")));
    detail.replaceChildren(
      h("div", { class: "sdhead" },
        h("h2", { text: d.name, title: d.name }),
        d.default ? h("span", { class: "chip on", text: App.t("snd.default") })
          : h("button", { class: "btn sm", text: App.t("snd.makeDefault"), onClick: () => setDefault(d.id) }),
        h("span", { class: "sp" }),
        h("span", { class: "cap", text: App.t("snd.apps", { n: d.apps.length }) })),
      apps.length || idle.length ? h("div", { class: "sdapps" }, ...apps, ...idle) : h("div", { class: "muted sempty", text: App.t("snd.noApps") }));
    paintValues();
  }

  function paintValues() {
    for (const d of devs) {
      const m = rows.get(d.id);
      if (m) { if (dragging !== m.key) m.v = d.volume; m.muted = d.muted; m.peak = d.peak; paintRow(m, m.v, d.muted, d.peak); }
      for (const a of d.apps) {
        const r = rows.get(d.id + "|" + a.key);
        if (r) { if (dragging !== r.key) r.v = a.volume; r.muted = a.muted; r.peak = a.peak; paintRow(r, r.v, a.muted, a.peak); }
      }
    }
  }

  async function poll() {
    // a minimised or covered window shows nothing: don't make the recorder enumerate devices for it
    if (busy || S.view !== "snd" || document.hidden) return;
    busy = true;
    try {
      const r = await cmd("mix-get").catch(() => null);
      if (S.view !== "snd") return;
      if (!r || !r.ok) { noRecorder(); return; }
      devs = r.devices;
      if (!!r.display_audio !== displayAudio) { displayAudio = !!r.display_audio; drawOpts(); }
      const hid = hiddenIds();
      if (!devs.some((d) => d.id === sel && (d.default || !hid.includes(d.id)))) sel = (devs.find((d) => d.default) || devs[0] || {}).id || "";
      const sl = JSON.stringify([hid, showHidden, devs.map((d) => [d.id, d.name, d.default, d.comms, appsText(d)])]);
      const sd = sigDetail();
      const newList = sl !== shapeList, newDetail = sd !== shapeDetail;
      if (newList) { shapeList = sl; buildList(); }
      if (newDetail) { shapeDetail = sd; buildDetail(); }
      if (!newList && !newDetail) { paintValues(); paintCards(); }
    } finally { busy = false; }
  }

  function noRecorder() {
    stop();
    page.replaceChildren(App.emptyState(App.t("eq.noRec"), App.t("snd.noRecD"), App.t("eq.startRec"), async () => {
      await api.invoke("recorder_launch").catch(() => {});
      setTimeout(() => App.Sound.render(), 1500);
    }));
  }

  /** The tray icon's switch sits where the EQ page has its on/off switch; the shortcut card closes the list. */
  async function drawOpts() {
    await App.RecUI.ensureLoaded();
    const cfg = App.RecUI.cfg();
    trayBox.replaceChildren(h("label", { class: "eqmaster", title: App.t("beh.audioTrayD") }, h("span", { text: App.t("beh.audioTray") }),
      h("span", { class: "toggle" + (cfg.audio_tray !== false ? " on" : ""), onClick: async () => {
        await App.RecUI.save({ audio_tray: cfg.audio_tray === false }); drawOpts();
      } })));
    // the shortcuts are edited in Settings → Shortcuts only (one place); here they are shown, with the way there
    const keyRow = (label, tip, field, first) => {
      const row = U.srow(App.t(label), h("div", { class: "pathrow" },
        h("span", { class: "mono keyshow" + (cfg[field] ? "" : " dim"), text: cfg[field] ? String(cfg[field]).replaceAll(",", "   ·   ").replaceAll("+", " + ") : "—" }),
        first ? h("button", { class: "linkbtn", text: App.t("set.tab.keys") + " →", onClick: () => { App.Settings.tab = "keys"; App.go("set"); } }) : null));
      row.title = App.t(tip);
      return row;
    };
    const toggle = (label, desc, field, def) => U.trow(App.t(label), App.t(desc), cfg[field] ?? def,
      async (v) => { await App.RecUI.save({ [field]: v }); drawOpts(); });
    opts.replaceChildren(
      keyRow("key.output", "key.outputTip", "output_hotkey", true),
      keyRow("key.mic", "key.micTip", "mic_hotkey", false),
      h("div", { class: "tgl" },
        toggle("snd.notify", "snd.notifyD", "output_popup", true),
        toggle("snd.keepVol", "snd.keepVolD", "keep_volume", false),
        displayAudio ? toggle("snd.wake", "snd.wakeD", "wake_display_audio", true) : null));
  }

  document.addEventListener("visibilitychange", () => { if (!document.hidden && timer) poll(); });

  function stop() { clearInterval(timer); timer = 0; dragging = null; }

  App.Sound = {
    async render() {
      page = App.$("#v-snd");
      stop();
      shapeList = shapeDetail = "";
      list = h("div", { class: "slist intro" });
      setTimeout(() => list.classList.remove("intro"), 600);
      opts = h("div", { class: "card2 sopts" });
      detail = h("section", { class: "card2 sdetail" });
      trayBox = h("div", { class: "strayopt" });
      page.replaceChildren(h("div", { class: "snd" },
        h("div", { class: "eqhead" }, h("h1", { text: App.t("snd.title") }), h("span", { class: "sp" }), trayBox),
        h("div", { class: "sbody" }, h("div", { class: "sleft" }, list, opts), detail)));
      await App.RecUI.ensureLoaded().catch(() => {});
      drawOpts();
      await poll();
      if (S.view === "snd" && !timer && list.isConnected) timer = setInterval(poll, POLL_MS);
    },
    leave: stop,
  };
})();
