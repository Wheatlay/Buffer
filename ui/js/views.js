"use strict";
/* Chrome around the views: top bar (search, REC pill, export chip), icon rail, export popover + history
   drawer, and settings. "Lime means now": playhead, selection, active item, progress. */

(function () {
  const { h, S, api } = App;
  const VIEWS = { lib: "#v-lib", ed: "#v-ed", eq: "#v-eq", snd: "#v-snd", rgb: "#v-rgb", fans: "#v-fans", set: "#v-set" };

  /* ───── navigation ───── */

  App.go = (view) => {
    if (view === "exp") view = "lib";                       // the standalone exports page is gone
    if (view === "ed" && !App.Ed.current()) view = "lib";
    if ((view === "rgb" || view === "fans") && !App.Fans) {
      App.loadHw().then(() => App.go(view), (e) => App.toast(String(e), "err"));
      return;
    }
    if (S.view === "ed" && view !== "ed") App.Ed.leave();
    if (S.view === "eq" && view !== "eq") App.EQ.leave();
    if (S.view === "snd" && view !== "snd") App.Sound.leave();
    if (S.view === "rgb" && view !== "rgb") App.RGB.leave();
    if (S.view === "fans" && view !== "fans") App.Fans.leave();
    const changed = S.view !== view;
    const prev = S.view;
    document.querySelectorAll("#views .view.leaving").forEach((e) => e.classList.remove("leaving"));
    S.view = view;
    for (const [v, sel] of Object.entries(VIEWS)) App.$(sel).hidden = v !== view;
    if (changed && VIEWS[prev] && App.P.settings.anim !== false) keepUnderneath(prev);
    if (view === "lib") App.Lib.render(false);
    if (view === "set") Settings.render(true);
    if (view === "eq" && changed) App.EQ.render();
    if (view === "snd" && changed) App.Sound.render();
    if (view === "rgb" && changed) App.RGB.render();
    if (view === "fans" && changed) App.Fans.render();
    const seen = App.P.settings.navSeen || {};
    if (MODULES.includes(view) && !seen[view]) { App.P.settings.navSeen = { ...seen, [view]: 1 }; App.persist(); }
    renderRail();
  };

  /* ───── sidebar modules: hide / show, "new" until first opened ───── */

  /** Modules the user can hide from the sidebar (Library and Settings always stay). */
  const MODULES = ["eq", "snd", "rgb", "fans"];
  const NAV_KEY = { eq: "nav.eq", snd: "nav.sound", rgb: "nav.rgb", fans: "nav.fans" };
  const navName = (v) => App.t(NAV_KEY[v]).replace("\n", " ");
  App.navHidden = () => App.P.settings.navHidden || [];
  /** Hides or shows one module; a hidden module that is open sends the user to the Library. */
  App.setNavHidden = (view, hide) => {
    const cur = App.navHidden().filter((v) => v !== view);
    App.P.settings.navHidden = hide ? [...cur, view] : cur;
    App.persist();
    if (hide && S.view === view) App.go("lib");
    else renderRail();
    if (S.view === "set") Settings.render();
  };

  let navMenuEl = null;
  function closeNavMenu() {
    if (!navMenuEl) return;
    navMenuEl.remove();
    navMenuEl = null;
    document.removeEventListener("pointerdown", navAway, true);
    document.removeEventListener("keydown", navEsc, true);
    window.removeEventListener("blur", closeNavMenu);
  }
  const navAway = (e) => { if (navMenuEl && !navMenuEl.contains(e.target)) closeNavMenu(); };
  const navEsc = (e) => { if (e.key === "Escape") closeNavMenu(); };
  /** A small right-click menu at the pointer; `items` are [label, action, extraClass?] or "-" for a divider. */
  App.ctxMenu = (e, items) => {
    e.preventDefault();
    closeNavMenu();
    const kids = items.map((it) => (it === "-" ? h("hr") :
      h("button", { class: it[2] || "", onClick: () => { closeNavMenu(); it[1](); } }, it[0])));
    if (!kids.length) return;
    navMenuEl = h("div", { class: "eqmenu navmenu", style: { left: `${e.clientX}px`, top: `${e.clientY}px` } }, kids);
    document.body.append(navMenuEl);
    const r = navMenuEl.getBoundingClientRect();
    if (r.bottom > innerHeight - 8) navMenuEl.style.top = `${innerHeight - r.height - 8}px`;
    if (r.right > innerWidth - 8) navMenuEl.style.left = `${innerWidth - r.width - 8}px`;
    setTimeout(() => {
      document.addEventListener("pointerdown", navAway, true);
      document.addEventListener("keydown", navEsc, true);
      window.addEventListener("blur", closeNavMenu);
    });
  };

  /** Right click on a sidebar item: hide it (modules only) and bring back the hidden ones. */
  function navMenu(e, view) {
    e.preventDefault();
    closeNavMenu();
    const hidden = App.navHidden();
    const items = [];
    if (MODULES.includes(view)) items.push(h("button", { title: App.t("nav.hideTip"), onClick: () => { closeNavMenu(); App.setNavHidden(view, true); } }, App.t("nav.hide")));
    if (hidden.length) {
      if (items.length) items.push(h("hr"));
      for (const v of hidden) items.push(h("button", { onClick: () => { closeNavMenu(); App.setNavHidden(v, false); } }, App.t("nav.show", { name: navName(v) })));
    }
    if (!items.length) return;
    navMenuEl = h("div", { class: "eqmenu navmenu", style: { left: `${e.clientX}px`, top: `${e.clientY}px` } }, items);
    document.body.append(navMenuEl);
    const r = navMenuEl.getBoundingClientRect();
    if (r.bottom > innerHeight - 8) navMenuEl.style.top = `${innerHeight - r.height - 8}px`;
    setTimeout(() => {
      document.addEventListener("pointerdown", navAway, true);
      document.addEventListener("keydown", navEsc, true);
      window.addEventListener("blur", closeNavMenu);
    });
  }

  /** The "back" action shared by the mouse Back button and Alt+Left. */
  let forwardTo = null;                       // the view Back just left: Forward returns to it
  App.back = () => {
    if (App.Exports.closeAny()) return;
    if (S.view !== "lib") { forwardTo = S.view; App.go("lib"); }
  };
  /** Mouse Forward / Alt+Right: back into the editor or settings that Back just closed. */
  App.forward = () => {
    if (forwardTo && S.view === "lib") App.go(forwardTo);
    forwardTo = null;
  };

  /** The view being left stays on screen under the one fading in, so a switch is a dissolve and not a dark gap
   *  (the gap made the library look like it blinked out while the editor faded in). */
  let leaveTimer = 0;
  function keepUnderneath(prev) {
    const el = App.$(VIEWS[prev]);
    el.hidden = false;
    el.classList.add("leaving");
    clearTimeout(leaveTimer);
    leaveTimer = setTimeout(() => {
      el.classList.remove("leaving");
      if (S.view !== prev) el.hidden = true;
    }, 400);
  }

  /** The icon rail is built once; later calls only move the "on" state (rebuilding it flashed the Library button). */
  let railItems = null;
  function renderRail() {
    const nav = App.$("#nav");
    const defs = [
      ["lib", "▦", "nav.library", S.view === "lib" || S.view === "ed"],
      ["eq", "≋", "nav.eq", S.view === "eq"],
      ["snd", "♪", "nav.sound", S.view === "snd"],
      ["rgb", "◈", "nav.rgb", S.view === "rgb"],
      ["fans", "✲", "nav.fans", S.view === "fans"],
      ["set", "⚙", "nav.settings", S.view === "set"],
    ];
    if (!railItems || !railItems.every((el) => el.isConnected)) {
      railItems = defs.map(([view, icon]) => h("button", { class: "item", onClick: () => App.go(view), onContextmenu: (e) => navMenu(e, view) },
        h("span", { class: "ic", text: icon }), h("span", { class: "lb" })));
      // everything but the last item sits at the top; settings stays pinned to the bottom
      nav.replaceChildren(...railItems.slice(0, -1), h("div", { class: "grow" }), railItems[railItems.length - 1]);
    }
    const hidden = App.navHidden(), seen = App.P.settings.navSeen || {};
    defs.forEach(([view, , key, on], i) => {
      const el = railItems[i], label = App.t(key);
      el.classList.toggle("on", on);
      const isMod = MODULES.includes(view);
      el.hidden = isMod && hidden.includes(view);
      el.classList.toggle("new", isMod && !seen[view]);
      el.title = isMod && !seen[view] ? App.t("nav.newTip") : "";
      el.setAttribute("aria-label", label.replace("\n", ""));
      const lb = el.querySelector(".lb");
      if (lb.textContent !== label) lb.textContent = label;
      lb.classList.toggle("two", label.includes("\n"));         // "Sound\nSwitcher": two lines instead of an ellipsis
    });
  }

  /** Own window buttons: the native title bar is turned off. */
  function windowControls() {
    const w = window.__TAURI__?.window?.getCurrentWindow?.();
    const btn = (cls, title, fn) => h("button", { class: "wc " + cls, title, "aria-label": title, onClick: () => w && fn(w) }, h("i"));
    return h("div", { class: "winctl" },
      btn("min", App.t("win.min"), (x) => x.minimize()),
      btn("max", App.t("win.max"), (x) => x.toggleMaximize()),
      btn("cls", App.t("win.close"), (x) => x.close()));
  }

  /* ───── top bar ───── */

  const recPill = h("button", { class: "recpill off", onClick: () => onRecClick() });
  // the EQ status next to it: a status light too, a click opens the EQ page (hidden without Equalizer APO)
  const eqPill = h("button", { class: "recpill eqpill", hidden: true, onClick: () => App.go("eq") });
  const chip = h("button", { class: "expchip", onClick: () => Exports.togglePop() });

  function buildTopbar() {
    const widths = [5, 9, 13, 17, 13, 9, 5];
    const search = h("input", { id: "topSearch", placeholder: App.t("search.ph"), spellcheck: false,
      onInput: (e) => {
        S.q = e.target.value;
        if (S.view !== "lib") App.go("lib"); else App.Lib.render(false);
      } });
    // left: the name and the status lights (recorder, EQ); middle: search; right: updates, exports, window buttons
    App.$("#topbar").replaceChildren(
      h("div", { class: "lhs", "data-tauri-drag-region": true },
        h("div", { class: "brand", "data-tauri-drag-region": true },
          h("div", { class: "mini" }, widths.map((w) => h("i", { style: { width: `${w}px` } }))),
          h("b", { text: "Buffer" }),
          App.isDev ? h("span", { class: "devbadge", text: "DEV", title: App.t("dev.badgeTip") }) : null),
        recPill, eqPill),
      h("label", { class: "search" }, h("span", { text: "⌕" }), search, h("kbd", { text: "Ctrl K" })),
      h("div", { class: "right", "data-tauri-drag-region": true }, App.i18n.topbarChip(), App.Update.chip, chip, windowControls()));
    paintRec();
    paintChip();
  }
  App.focusSearch = () => App.$("#topSearch")?.focus();

  const fmtBuf = (s) => `${Math.floor(s / 60)}:${String(Math.round(s) % 60).padStart(2, "0")}`;
  /** A source worth a warning: down for a real reason. One that waits for a chat app that is not running, or for a
   *  kind of device the PC does not have, is down but fine (an older recorder has no `fault`: then down = fault). */
  const faulty = (App.srcFaulty = (s) => (s.fault ?? !s.live));

  function paintRec() {
    const r = S.recorder;
    const down = r ? r.sources.filter(faulty).map((s) => s.name) : [];
    let cls, text;
    if (!r) { cls = "off"; text = App.t("rec.pill.off"); }
    else if (r.post_roll_left_ms > 0) { cls = "saving"; text = App.t("rec.pill.saveIn", { n: Math.ceil(r.post_roll_left_ms / 1000) }); }
    else if (r.saving) { cls = "saving"; text = App.t("rec.pill.saving"); }
    else if (r.idle) { cls = "idle"; text = App.t("rec.pill.idle"); }
    else if (!r.recording) { cls = "bad"; text = App.t("rec.pill.stopped"); }
    else if (down.length) { cls = "warn"; text = App.t("rec.pill.warn"); }
    else { cls = "on"; text = App.t("rec.pill.on", { t: fmtBuf(r.buffer_s) }); }
    // rebuilt only when it changes: a new dot element restarts its pulse animation, and this runs on every poll
    const sig = cls + "|" + text;
    recPill.title = r && r.post_roll_left_ms > 0 ? App.t("roll.again") : recTooltip();
    if (recPill._sig === sig) return;
    // only the text changed (the countdown ticking): keep the dot, so its pulse is not restarted every second
    if (recPill._cls === cls && recPill.lastChild && recPill.lastChild.nodeType === 3) { recPill._sig = sig; recPill.lastChild.textContent = text; return; }
    recPill._sig = sig;
    recPill._cls = cls;
    recPill.className = `recpill ${cls}`;
    recPill.replaceChildren(h("span", { class: "dot" }), text);
  }

  /** EQ on / off, from the recorder's status. Shown only when there is an EQ to speak of: the recorder runs and
   *  Equalizer APO is installed. */
  function paintEq() {
    const r = S.recorder;
    eqPill.hidden = !(r && r.eq_apo);
    if (eqPill.hidden) return;
    const on = !!r.eq_on;
    const sig = on + "|" + App.i18n.current();             // the label is rebuilt when the language changes too
    eqPill.title = App.t("eq.pill.tip");
    if (eqPill._sig === sig) return;
    eqPill._sig = sig;
    eqPill.className = `recpill eqpill ${on ? "eqon" : "eqoff"}`;
    eqPill.replaceChildren(h("span", { class: "dot" }), App.t(on ? "eq.pill.on" : "eq.pill.off"));
  }

  function recTooltip() {
    const r = S.recorder;
    if (!r) return App.t("rec.tip.off");
    if (r.idle) return App.t("rec.tip.idle");
    if (!r.recording) return r.last_error || App.t("rec.tip.stopped");
    const down = r.sources.filter(faulty).map((s) => s.name);
    const base = down.length ? App.t("rec.tip.down", { list: down.join(", ") }) : App.t("rec.tip.on", { cur: r.buffer_s, target: r.buffer_target_s });
    return App.t("rec.tip.save", { base });
  }

  /** The pill is a status light: a click opens the recording settings (saving a clip is the hotkey or the button there). */
  function onRecClick() {
    App.Settings.tab = "rec";
    App.go("set");
  }

  function paintChip() {
    const ex = S.exporting;
    if (!ex) chip._live = null;
    chip.className = "expchip" + (ex ? " busy" : S.lastDone && Date.now() - S.lastDone.at < 7000 ? " done" : "") + (popOpen ? " open" : "");
    chip.title = popOpen ? App.t("common.close") : "";
    if (ex) {
      if (!chip._live) {
        const label = document.createTextNode("");
        const bars = h("span", { class: "bars" }, Array.from({ length: 8 }, () => h("i")));
        chip.replaceChildren(label, bars);
        chip._live = { label, bars };
      }
      const shown = ex.shown ?? ex.pct;
      chip._live.label.textContent = `⇪ ${Math.round(shown)}%`;
      [...chip._live.bars.children].forEach((b, i) => { b.style.opacity = 0.15 + 0.85 * fill(shown, 8, i); });
    } else if (S.lastDone && Date.now() - S.lastDone.at < 7000) {
      chip.replaceChildren(`✓ ${S.lastDone.size} MiB`);
    } else {
      chip.replaceChildren("⇪ " + App.t("exp.chip"));
    }
  }

  /* ───── recorder polling ───── */

  const Rec = (App.Rec = {});
  let pollTimer = 0;
  Rec.refresh = async () => {
    // a recorder that stops answering counts as not running, instead of freezing the poll loop
    try { S.recorder = await App.timeout(api.invoke("recorder_cmd", { cmd: "status" }), 4000); } catch { S.recorder = null; }
    watchdog();
    paintRec();
    paintEq();
    // a clip the recorder just finished appears in the library without waiting for the window to regain focus
    const saved = S.recorder ? S.recorder.last_saved : "";
    if (Rec.lastSaved !== undefined && saved && saved !== Rec.lastSaved) {
      App.toast(App.t("rec.savedToast", { name: App.baseName(saved) }), "ok");
      App.Lib.load();
    }
    if (saved || Rec.lastSaved === undefined) Rec.lastSaved = saved;
    if (S.view === "set" && Settings.live()) Settings.paintRecorder();
  };
  /* A recorder that was up and then vanished (crash, killed by another tool) is relaunched, and the user is told.
     A stop the user asked for is left alone: from this window (Rec.expectDown), and from the tray or anywhere else
     (the recorder leaves a marker when it is closed on purpose, a crash leaves none). */
  let wasUp = false, downPolls = 0, lastRelaunch = 0;
  Rec.expectDown = 0;
  function watchdog() {
    if (S.recorder) { wasUp = true; downPolls = 0; return; }
    if (!wasUp || ++downPolls < 3) return;
    const now = Date.now();
    if (now - Rec.expectDown < 20000 || now - lastRelaunch < 30000) return;
    lastRelaunch = now;
    wasUp = false;
    api.invoke("recorder_quit_deliberate").catch(() => false)
      .then((deliberate) => (deliberate ? null : api.invoke("recorder_launch").then(() => { App.toast(App.t("rec.relaunched"), "err"); setTimeout(Rec.refresh, 2500); })))
      .catch((e) => App.toast(App.t("rec.relaunchFail", { err: e }), "err"));
  }
  Rec.start = () => {
    const loop = async () => {
      if (!document.hidden) await Rec.refresh();
      pollTimer = setTimeout(loop, S.view === "set" && Settings.live() ? 700 : 1200);
    };
    clearTimeout(pollTimer);
    loop();
  };

  /* ───── exports: run, popover, drawer ───── */

  const Exports = (App.Exports = {});
  const exQueue = [];
  let popOpen = false;
  let popHover = false;
  let popUntil = 0;
  let drawerOpen = false;

  const ago = (ts) => {
    const m = Math.round((Date.now() - ts) / 60000);
    return m < 1 ? App.t("ago.now") : m < 60 ? App.t("ago.min", { n: m }) : m < 1440 ? App.t("ago.h", { n: Math.round(m / 60) }) : App.t("ago.d", { n: Math.round(m / 1440) });
  };

  /** Runs one export with the editor's or the quick-export defaults. Resolves with the finished file or null. */
  Exports.run = async (o) => {
    if (S.exporting) { App.toast(App.t("exp.busy"), "err"); return null; }
    const clip = S.byPath.get(o.path);
    if (!clip) return null;
    const s = App.P.settings;
    const mb = o.target ?? s.defTarget;
    const dir = App.exportDirFor();
    const suffix = (o.wide != null ? "_16x9" : "") + (o.speed && o.speed !== 1 ? `_${o.speed}x` : "");
    const output = `${dir}\\${clip.name}${suffix}_${mb}MB.mp4`;
    const volumes = o.volumes || ["Game", "Chat", "Mic"].map((n) => [n, App.clipGain(clip.path, n) / 100]);
    const opts = {
      input: clip.path, output, start: o.start ?? 0, end: o.end ?? null,
      target_bytes: Math.round(mb * 1048576), max_height: o.maxHeight ?? null,
      codec: o.codec || "auto", audio_kbps: 128, volumes,
      crop_wide: o.wide ?? null, speed: o.speed ?? 1, normalize: o.normalize ?? s.normDef, hwaccel: s.hw,
    };
    S.exporting = { name: App.titleOf(clip), target: mb, pct: 0, source: clip.path, startedAt: Date.now() };
    S.lastDone = null;
    Exports.openPop();
    notifyState();
    try {
      const r = await api.invoke("export_clip", { opts });
      const secs = ((Date.now() - S.exporting.startedAt) / 1000).toFixed(1);
      const entry = { id: `e${Date.now()}`, name: output.slice(output.lastIndexOf("\\") + 1), path: output,
        source: clip.path, size: +(r.size / 1048576).toFixed(2), target: mb,
        dur: ((o.end ?? (App.Lib.meta(clip) || {}).dur ?? 0) - (o.start ?? 0)) / (o.speed || 1), created: Date.now() };
      App.P.exports.unshift(entry);
      App.P.exports = App.P.exports.slice(0, 200);
      App.persist();
      S.lastDone = { ...entry, secs, at: Date.now(), h: o.maxHeight ?? null, tries: r.attempts, over: r.size > opts.target_bytes };
      if (S.lastDone.over) App.toast(App.t("exp.over", { size: entry.size, target: mb }), "err");
      popUntil = Date.now() + 7000;
      setTimeout(() => Exports.autoClean(), 0);
      return { name: entry.name, path: output, size: entry.size };
    } catch (e) {
      if (String(e).includes("cancelled")) App.toast(App.t("exp.cancelled"));
      else {
        App.toast(App.t("exp.failed", { err: e }), "err");
        if (App.Ed.current()) App.Ed.current().error = String(e);
      }
      return null;
    } finally {
      S.exporting = null;
      notifyState();
    }
  };

  Exports.cancel = () => api.invoke("cancel_export").catch(() => {});

  /* ───── optional automatic delete of old exports (moved to the Recycle Bin) ───── */
  const DAY = 86400000;
  const oldExports = (days) => App.P.exports.filter((e) => Date.now() - e.created > days * DAY);
  const bytesOf = (list) => list.reduce((a, e) => a + (e.size || 0), 0) * 1048576;
  /** Moves the files to the Recycle Bin and drops them from the history. Returns how many were handled. */
  Exports.remove = async (list) => {
    if (!list.length) return 0;
    try { await api.invoke("delete_clips", { paths: list.map((e) => e.path) }); } catch { /* already gone: forget them anyway */ }
    const gone = new Set(list.map((e) => e.id));
    App.P.exports = App.P.exports.filter((e) => !gone.has(e.id));
    App.persist();
    return list.length;
  };
  /** Runs at start and after each export; does nothing unless "Delete old exports" is set in Settings. */
  Exports.autoClean = async () => {
    const days = App.P.settings.autoCleanDays || 0;
    if (!days || S.exporting) return;
    const n = await Exports.remove(oldExports(days));
    if (n) { App.toast(App.t("clean.done", { n }), "ok"); notifyState(); }
  };

  function notifyState() {
    App.Ed.onExportState();
    paintChip();
    renderPop();
    if (S.view === "lib") App.Lib.renderSide();
  }

  Exports.quick = async (path) => { await Exports.run({ path }); };

  Exports.batch = async (paths) => {
    exQueue.push(...paths);
    let n = 0;
    const total = exQueue.length;
    while (exQueue.length) {
      const p = exQueue.shift();
      n++;
      App.toast(App.t("exp.batch", { n, total }));
      await Exports.run({ path: p });
    }
    App.Lib.clearSel();
  };

  /** How full bar `i` of `n` is at `pct`: 0..1, so the leading bar fills gradually instead of switching on. */
  const fill = (pct, n, i) => Math.max(0, Math.min(1, (pct / 100) * n - i));

  /* The encoder reports progress in steps; showing them directly looks jerky. Each report only moves a target and
     this loop eases what is on screen towards it (it stops by itself once it has arrived). */
  let smoothRaf = 0;
  function smoothTick() {
    smoothRaf = 0;
    const ex = S.exporting;
    if (!ex) return;
    const from = ex.shown ?? 0;
    const gap = ex.pct - from;
    ex.shown = Math.abs(gap) < 0.04 ? ex.pct : from + gap * 0.1;
    paintChip();
    updatePop();
    App.Ed.onProgress(ex.shown);
    if (ex.shown !== ex.pct) smoothRaf = requestAnimationFrame(smoothTick);
  }

  api.listen("export-progress", (e) => {
    if (!S.exporting) return;
    S.exporting.pct = e.payload * 100;
    if (!smoothRaf) smoothRaf = requestAnimationFrame(smoothTick);
  });

  // The first file came out over the target, so it is encoded again: the bar restarts, say why.
  api.listen("export-retry", (e) => {
    const ex = S.exporting;
    if (!ex) return;
    ex.retry = e.payload; ex.pct = 0; ex.shown = 0; ex.startedAt = Date.now();
    App.toast(App.t("exp.retry", { n: e.payload }));
    updatePop();
    if (!smoothRaf) smoothRaf = requestAnimationFrame(smoothTick);
  });

  Exports.togglePop = () => { popOpen ? closePop() : Exports.openPop(true); };
  Exports.openPop = (manual) => {
    popOpen = true;
    popUntil = manual ? 0 : Date.now() + 60000;   // auto-opened popovers close themselves after the result
    renderPop();
  };
  function closePop() { popOpen = false; renderPop(); }
  /** Closes the history drawer, else the popover. True when something was open. */
  Exports.closeAny = () => {
    if (drawerOpen) { drawerOpen = false; popUntil = Date.now() + 3000; renderDrawer(); return true; }
    if (popOpen) { closePop(); return true; }
    return false;
  };
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && popOpen && !drawerOpen) closePop();
  });

  setInterval(() => {
    if (popOpen && popUntil && !popHover && !S.exporting && !drawerOpen && Date.now() > popUntil) closePop();
    if (S.lastDone && Date.now() - S.lastDone.at >= 7000 && chip.classList.contains("done")) paintChip();
  }, 500);

  /** What the export decided on its own: a smaller picture, extra passes, or a file that missed the target. */
  const doneNote = (d) => {
    const bits = [];
    if (d.h) bits.push(`${d.h}p`);
    if (d.tries > 1) bits.push(App.t("exp.tries", { n: d.tries }));
    if (d.over) bits.push(App.t("exp.over", { size: d.size, target: d.target }));
    return bits.length ? h("div", { class: "mono muted", style: { fontSize: "11px", marginTop: "2px", ...(d.over ? { color: "var(--warn)" } : {}) }, text: bits.join(" · ") }) : "";
  };

  const dragHandle = (path, cls, text) => {
    const el = h("span", { class: cls, text, title: App.t("exp.dragTip"), "data-handle": "1" });
    el.addEventListener("pointerdown", (ev) => {
      if (ev.button !== 0) return;
      ev.preventDefault();
      api.startDrag(path).catch((err) => App.toast(App.t("exp.dragFail", { err }), "err"));
    });
    return el;
  };

  // one delegated listener per card: a click opens the export in the default app, pressing and moving drags it out
  for (const id of ["#pop", "#drawer"]) {
    App.$(id).addEventListener("pointerdown", (ev) => {
      const card = ev.target.closest("[data-path]");
      if (!card || ev.button !== 0 || ev.target.closest("button, .linkbtn, [data-handle]")) return;
      const x0 = ev.clientX, y0 = ev.clientY, path = card.dataset.path;
      const done = () => { removeEventListener("pointermove", move); removeEventListener("pointerup", up); };
      const move = (e) => {
        if (Math.hypot(e.clientX - x0, e.clientY - y0) < 6) return;
        done();
        api.startDrag(path).catch((err) => App.toast(App.t("exp.dragFail", { err }), "err"));
      };
      const up = () => { done(); api.invoke("open_file", { path }).catch((err) => App.toast(String(err), "err")); };
      addEventListener("pointermove", move);
      addEventListener("pointerup", up);
    });
  }

  function thumbBox(path, cls) {
    const el = h("div", { class: cls });
    App.Lib.thumbUrl(path).then((u) => { if (u) el.style.backgroundImage = `url("${u}")`; });
    return el;
  }

  let popLive = null;   // the nodes that change while an export runs
  /** Progress tick: patch numbers and bars in place (rebuilding the popover on every tick made it flash). */
  function updatePop() {
    const ex = S.exporting;
    if (!popLive || !ex || !popLive.pct.isConnected) return;
    const p = ex.shown ?? ex.pct;
    popLive.pct.textContent = `${Math.round(p)}%`;
    popLive.bars.forEach((b, i) => { b.style.opacity = 0.12 + (0.35 + 0.65 * (i / 30) - 0.12) * fill(p, 30, i); });
    const real = ex.pct;
    const elapsed = (Date.now() - ex.startedAt) / 1000;
    const eta = real > 3 ? Math.max(0, Math.round((elapsed * (100 - real)) / real)) : null;
    popLive.eta.textContent = ex.retry ? App.t("exp.retry", { n: ex.retry }) + (eta == null ? "" : ` · ≈ ${eta} s`)
      : eta == null ? App.t("exp.calc") : `≈ ${eta} s`;
  }

  function renderPop() {
    popLive = null;
    paintChip();
    const host = App.$("#pop");
    if (!popOpen) { host.replaceChildren(); renderDrawer(); return; }
    const ex = S.exporting;
    const done = !ex && S.lastDone;
    const recent = App.P.exports.filter((e) => !done || e.id !== S.lastDone.id).slice(0, 2);
    const parts = [];
    if (ex) {
      const p = ex.shown ?? ex.pct;
      const elapsed = (Date.now() - ex.startedAt) / 1000;
      const eta = p > 3 ? Math.max(0, Math.round((elapsed * (100 - p)) / p)) : null;
      const pctEl = h("span", { class: "mono lime", text: `${Math.round(p)}%` });
      const barEls = Array.from({ length: 30 }, (_, i) =>
        h("i", { style: { opacity: 0.12 + (0.35 + 0.65 * (i / 30) - 0.12) * fill(p, 30, i) } }));
      const etaEl = h("span", { text: ex.retry ? App.t("exp.retry", { n: ex.retry }) + (eta == null ? "" : ` · ≈ ${eta} s`)
        : eta == null ? App.t("exp.calc") : `≈ ${eta} s` });
      popLive = { pct: pctEl, bars: barEls, eta: etaEl };
      parts.push(h("div", null,
        h("div", { class: "pr1" }, h("b", { text: `${ex.name} · ${ex.target} MB` }), pctEl),
        h("div", { class: "scan" }, barEls),
        h("div", { class: "pr3 mono muted" }, etaEl,
          h("button", { class: "linkbtn", text: App.t("common.cancel"), onClick: Exports.cancel }))));
    }
    if (done) {
      parts.push(h("div", { class: "donecard", "data-path": done.path },
        h("div", { style: { minWidth: 0 } }, h("div", { class: "nm", text: done.name }),
          h("div", { class: "mono lime", text: App.t("exp.done", { secs: done.secs, size: done.size }) }),
          doneNote(done)),
        dragHandle(done.path, "big", "⇪ " + App.t("exp.dragBig"))));
    }
    for (const e of recent) {
      parts.push(h("div", { class: "prow", "data-path": e.path }, thumbBox(e.path, "tn"),
        h("div", { style: { minWidth: 0 } }, h("div", { class: "nm", text: e.name }),
          h("div", { class: "mono muted", text: `${e.size} MiB · ${ago(e.created)}` })),
        dragHandle(e.path, "lime", "⇪")));
    }
    if (!ex && !done && !recent.length) parts.push(h("div", { class: "muted", text: App.t("exp.empty") }));
    parts.push(h("button", { class: "histlink", text: App.t("exp.history", { n: App.P.exports.length }),
      onClick: () => { drawerOpen = true; renderDrawer(); } }));
    const head = h("div", { class: "phead" }, h("b", { text: App.t("exp.chip") }),
      h("button", { class: "pclose", title: App.t("common.close"), "aria-label": App.t("common.close"), text: "✕", onClick: closePop }));
    host.replaceChildren(h("div", { class: "popover",
      onMouseenter: () => { popHover = true; }, onMouseleave: () => { popHover = false; popUntil = Date.now() + 3000; } }, head, parts));
    renderDrawer();
  }

  function renderDrawer() {
    const host = App.$("#drawer");
    if (!drawerOpen) { host.replaceChildren(); return; }
    const close = () => { drawerOpen = false; popUntil = Date.now() + 3000; renderDrawer(); };
    host.replaceChildren(h("div", { class: "scrim", onClick: close },
      h("aside", { class: "drawer", onClick: (e) => e.stopPropagation() },
        h("div", { class: "dhead" }, h("b", { text: App.t("exp.historyTitle") }), h("button", { class: "linkbtn", text: "✕", onClick: close })),
        ...(App.P.exports.length ? App.P.exports.map((e) => h("div", { class: "drow", "data-path": e.path }, thumbBox(e.path, "tn"),
          h("div", { style: { minWidth: 0 } }, h("div", { class: "nm", text: e.name }),
            h("div", { class: "mono muted", text: `${e.size} MiB · ${ago(e.created)}` })),
          h("div", { class: "acts" },
            h("button", { class: "linkbtn", text: App.t("exp.show"), onClick: () => api.invoke("reveal", { path: e.path }) }),
            dragHandle(e.path, "lime", "⇪")))) : [h("div", { class: "muted", text: App.t("exp.none") })]))));
  }

  /* ───── settings: Nagrywanie · Dźwięk · Skróty | Biblioteka · Eksport · Ogólne · Zaawansowane ───── */

  const Settings = (App.Settings = { tab: "rec" });
  const U = App.ui;
  Settings.paintRecorder = () => App.RecUI.paint();   // called by the recorder poll while the tab is open
  const TABS = ["rec", "aud", "keys", "|", "lib", "exp", "gen", "adv", "about"];   // "|" = the line between recording and the rest
  /** Tabs whose content follows the recorder (status bar, meters, diagnostics): the poll runs faster there. */
  Settings.live = () => ["rec", "aud", "keys", "adv"].includes(Settings.tab);
  const THEMES = [["ink", "#0b0c0a"], ["oled", "#000000"]];

  /** Settings pane; only a tab switch plays the slide-in, edits inside a tab re-render quietly. */
  let renderSeq = 0;
  Settings.render = async (animate = false) => {
    const root = App.$("#v-set");
    const scroller = () => root.querySelector(".setscroll");
    const keep = !animate && scroller() ? scroller().scrollTop : 0;      // an edit inside a tab must not jump to the top
    const pane = h("div", { class: "pane" + (animate ? " stepin" : "") });
    const mount = () => {
      root.replaceChildren(h("div", { class: "set" },
        h("div", { class: "tabs" }, h("h1", { text: App.t("set.title") }),
          TABS.map((k) => (k === "|" ? h("div", { class: "tdiv" }) : h("button", { class: Settings.tab === k ? "on" : "", text: App.t("set.tab." + k),
            onClick: () => { Settings.tab = k; Settings.render(true); } }))),
          h("div", { class: "grow" }),
          h("button", { class: "wizbtn", text: App.t("set.wizard"), onClick: () => App.RecUI.wizard() })),
        h("div", { class: "setscroll" }, pane)));
    };
    const restore = () => { if (keep && scroller()) scroller().scrollTop = keep; };
    const mine = ++renderSeq;
    if (["rec", "aud", "keys"].includes(Settings.tab)) {
      // the recorder's tabs look their own elements up in the document while they build (meters, status bar): attach first
      mount();
      await App.RecUI.renderTab(pane, Settings.tab);
    } else {
      // the other tabs wait for their data (tools, cache size, clips) and are swapped in finished, in one go:
      // attaching an empty pane first showed as the whole panel blinking on every edit
      const build = { lib: renderLibraryTab, exp: renderExportTab, gen: renderGeneralTab, adv: renderAdvancedTab, about: renderAboutTab }[Settings.tab];
      await build(pane);
      if (mine !== renderSeq) return;                                     // a newer render took over meanwhile
      mount();
      if (Settings.tab === "adv") App.RecUI.paint();                      // the diagnostics line is filled by the recorder poll
    }
    restore();
  };

  /** Replaces one export preset; the default target follows its slot if its old value disappeared. */
  function setPreset(i, v) {
    const s = App.P.settings, old = App.presets();
    if (!Number.isFinite(v) || v < 1 || v > 2000) { Settings.render(); return; }
    const next = old.slice();
    next[i] = Math.round(v);
    const slot = old.indexOf(s.defTarget);
    s.presets = next;
    if (!next.includes(s.defTarget)) s.defTarget = next[slot >= 0 ? slot : 0];
    App.persist();
    Settings.render();
  }

  /* Biblioteka: one folder list (the radio marks where new recordings go), library toggles, the look of the grid. */
  async function renderLibraryTab(pane) {
    await App.RecUI.ensureLoaded();
    const rc = App.RecUI.cfg();
    const s = App.P.settings;
    const rec = rc.output_dir;
    const dirs = [...new Set([rec, ...App.P.libDirs])].filter(Boolean);
    const reload = async () => { App.persist(); await App.Lib.load(); Settings.render(); };
    const setRecDir = async (d) => {
      await App.RecUI.save({ output_dir: d });
      if (!App.P.libDirs.includes(d)) App.P.libDirs.unshift(d);
      await reload();
    };
    const folderRow = (d) => {
      const isRec = d === rec;
      return h("div", { class: "dirrow" },
        h("button", { class: "radio" + (isRec ? " on" : ""), "aria-label": App.t("lib.recHere"), title: isRec ? App.t("lib.recHereOn") : App.t("lib.recHere"),
          onClick: () => { if (!isRec) setRecDir(d); } }),
        h("span", { class: "mono path" + (isRec ? "" : " dim"), text: d }),
        isRec ? h("span", { class: "tag lime", text: App.t("lib.tagNew") }) : h("span", { class: "tag", text: App.t("lib.tagOnly") }),
        isRec
          ? h("button", { class: "linkbtn", text: App.t("common.change"), onClick: async () => {
            const n = await api.pickFolder();
            if (!n) return;
            App.P.libDirs = App.P.libDirs.map((x) => (x === d ? n : x));
            await setRecDir(n);
          } })
          : h("button", { class: "linkbtn x", text: "✕", title: App.t("lib.removeTip"), onClick: async () => {
            App.P.libDirs = App.P.libDirs.filter((x) => x !== d); await reload();
          } }));
    };
    // optional: take the icons of installed games for the games that have none yet (Steam, Epic, usual folders)
    const findIcons = async (ev) => {
      const btn = ev.currentTarget;
      const todo = App.Lib.games().map(([g]) => g).filter((g) => !App.Lib.hasIcon(g));
      if (!todo.length) { App.toast(App.t("lib.iconsNone")); return; }
      btn.disabled = true;
      try {
        const r = await api.invoke("import_game_icons", { names: todo });
        await App.Lib.loadIcons();
        App.toast(App.t("lib.iconsDone", { n: r.found.length, m: todo.length }), r.found.length ? "ok" : "");
      } catch (e) {
        App.toast(String(e), "err");
      } finally {
        btn.disabled = false;
      }
    };
    pane.append(h("div", { class: "setcol" },
      h("div", null, U.cap(App.t("lib.folders")),
        h("div", { class: "card2 pad folders" }, ...dirs.map(folderRow),
          h("button", { class: "linkbtn", text: App.t("lib.add"), onClick: async () => {
            const n = await api.pickFolder();
            if (n && !App.P.libDirs.includes(n)) { App.P.libDirs.push(n); await reload(); }
          } }))),
      h("div", { class: "card2 tgl" },
        U.trow(App.t("lib.perGame"), App.t("lib.perGameD"), rc.per_game_folders, async (on) => { await App.RecUI.save({ per_game_folders: on }); Settings.render(); }),
        U.trow(App.t("lib.scan"), App.t("lib.scanD"), s.sub, async (on) => { s.sub = on; await reload(); })),
      h("div", null, U.cap(App.t("cap.look")),
        h("div", { class: "card2" },
          U.srow(App.t("app.thumb"), U.segc(["S", "M", "L"].map((k) => [k, k]), s.thumb, (k) => { s.thumb = k; App.persist(); Settings.render(); })),
          U.srow(App.t("lib.iconsLabel"), h("button", { class: "btn", text: App.t("lib.iconsFind"), title: App.t("lib.iconsTip"), onClick: findIcons }))))));
  }

  /* Eksport: size, folder, loudness, decoding and clean-up. */
  async function renderExportTab(pane) {
    const s = App.P.settings;
    pane.append(h("div", { class: "setcol" },
      h("div", null, U.cap(App.t("exp.cap")),
        h("div", { class: "card2" },
          U.srow(App.t("exp.defTarget"), h("div", { class: "pathrow" },
            Settings.presetsOpen
              ? h("div", { class: "pathrow" }, ...App.presets().map((mb, i) => h("input", { class: "field monoin", type: "number", min: 1, max: 2000, value: mb,
                style: { width: "74px" }, title: "MB", onChange: (e) => setPreset(i, +e.target.value) })), h("span", { class: "muted", text: "MB" }))
              : U.segc(App.presets().map((mb) => [mb, App.presetName(mb) ? `${App.presetName(mb)} · ${mb} MB` : `${mb} MB`]), s.defTarget, (mb) => { s.defTarget = mb; App.persist(); Settings.render(); }),
            h("button", { class: "linkbtn", text: Settings.presetsOpen ? "✓" : App.t("set.presetsEdit"),
              onClick: () => { Settings.presetsOpen = !Settings.presetsOpen; Settings.render(); } }))),
          U.srow(App.t("exp.outDir"), h("div", { class: "pathrow" },
            h("span", { class: "mono path", text: App.exportDirFor() }),
            h("button", { class: "linkbtn", text: App.t("common.change"), onClick: async () => {
              const d = await api.pickFolder();
              if (d) { s.outDir = d; App.persist(); Settings.render(); }
            } }),
            s.outDir ? h("button", { class: "linkbtn", text: App.t("common.default"), onClick: () => { s.outDir = ""; App.persist(); Settings.render(); } }) : null)),
          U.trow(App.t("exp.norm"), App.t("exp.normD"), s.normDef, (on) => { s.normDef = on; App.persist(); Settings.render(); }),
          U.trow(App.t("hw.dec"), App.t("hw.decD"), s.hw, (on) => { s.hw = on; App.persist(); Settings.render(); }),
          U.srow(App.t("set.autoClean"), U.segc([[0, App.t("clean.never")], ...[7, 30, 90].map((n) => [n, App.t("clean.days", { n })])],
            s.autoCleanDays || 0, (d) => { s.autoCleanDays = d; App.persist(); Settings.render(); }))))));
  }

  /* Ogólne: language, animations, starting with Windows, theme, notifications, version. */
  async function renderGeneralTab(pane) {
    const s = App.P.settings;
    await App.RecUI.ensureLoaded();
    const cur = App.i18n.current();
    const langRow = U.srow(App.t("lang.label"), h("div", { class: "pathrow" },
      h("select", { class: "field", onChange: (e) => App.i18n.setLang(e.target.value) },
        App.i18n.ORDER.map((c) => h("option", { value: c, text: App.i18n.META[c].name, selected: c === cur }))),
      App.i18n.META[cur].ai ? h("span", { class: "muted", style: { fontSize: "12px", whiteSpace: "nowrap" }, title: App.t("lang.aiTip"), text: App.t("lang.ai") }) : null));
    const animRow = U.trow(App.t("app.anim"), App.t("app.animD"), s.anim, (on) => { s.anim = on; App.persist(); App.applyTheme(); Settings.render(); });
    const blocks = App.RecUI.generalBlocks([langRow, animRow]);
    pane.append(h("div", { class: "setcol" },
      blocks.main,
      h("div", null, U.cap(App.t("theme.cap")), h("div", { class: "themes2" }, THEMES.map(([k, bg]) =>
        h("button", { class: "th2" + ((s.theme === "oled" ? "oled" : "ink") === k ? " on" : ""),
          onClick: () => { s.theme = k; App.persist(); App.applyTheme(); App.RecUI.syncLang().catch(() => {}); Settings.render(); } },
          h("div", { class: "pv2", style: { background: bg } }, h("i", { style: { width: "60%", background: "#1a1d16" } }), h("i", { style: { width: "30%", background: "#c6f432" } })),
          h("div", { class: "nm", text: App.t("theme." + k) }))))),
      h("div", null, U.cap(App.t("cap.notify")), blocks.notify),
      // sidebar modules: the same switch as the right click on the sidebar
      h("div", { title: App.t("nav.capTip") }, U.cap(App.t("nav.cap")),
        h("div", { class: "card2 tgl" }, MODULES.map((v) => U.trow(navName(v), null, !App.navHidden().includes(v), (on) => App.setNavHidden(v, !on)))))));
  }

  /* O programie: version and updates, where the project lives, license. */
  async function renderAboutTab(pane) {
    const s = App.P.settings;
    const REPO = "https://github.com/Wheatlay/Buffer";
    const link = (key, path) => h("button", { class: "linkbtn", text: App.t(key) + " ↗",
      onClick: () => api.invoke("open_issue_page", { url: REPO + path }).catch((e) => App.toast(String(e), "err")) });
    pane.append(h("div", { class: "setcol" },
      h("div", null, U.cap(App.t("cap.version")),
        h("div", { class: "card2" },
          U.srow("Buffer", h("span", { class: "muted", text: App.t("about.tag") })),
          U.srow(App.t("ver.label"), h("div", { class: "pathrow" },
            h("span", { class: "mono", id: "appVersion", text: "…" }),
            h("button", { class: "linkbtn", id: "updBtn", text: App.Update.hasUpdate() ? App.t("ver.install") : App.t("ver.check"),
              onClick: () => (App.Update.hasUpdate() ? App.Update.install() : App.Update.check(true)) }))),
          U.trow(App.t("ver.auto"), App.t("ver.autoD"), s.autoUpdate !== false, (on) => { s.autoUpdate = on; App.persist(); Settings.render(); }))),
      h("div", null, U.cap(App.t("about.links")),
        h("div", { class: "card2" },
          U.srow(App.t("about.github"), link("about.open", "")),
          U.srow(App.t("about.releases"), link("about.open", "/releases")),
          U.srow(App.t("about.issues"), link("about.open", "/issues/new")))),
      h("div", null, U.cap(App.t("about.legal")),
        h("div", { class: "card2" },
          U.srow(App.t("about.license"), link("about.open", "/blob/main/LICENSE")),
          U.srow(App.t("about.notices"), link("about.open", "/blob/main/THIRD-PARTY-NOTICES.md"))))));
  }

  /* Zaawansowane: what is normally left alone: tools, the recorder's own status, the cache. */
  async function renderAdvancedTab(pane) {
    const [tools, rgb, fans] = await Promise.all([api.invoke("tools_status").catch(() => null), api.invoke("rgb_status").catch(() => null), api.invoke("fans_status").catch(() => null)]);
    // the folder walk is the slowest part and only fills one line: show the page first, write the size in when it is known
    const sizeLine = h("span", { text: App.t("cache.used", { size: "…" }) });
    api.invoke("cache_size").catch(() => 0).then((b) => { sizeLine.textContent = App.t("cache.used", { size: App.fmtMB(b) }); });
    pane.append(h("div", { class: "setcol" },
      h("div", null, U.cap(App.t("adv.diag")),
        h("div", { class: "card2 pad stackrows" },
          h("div", { class: "between" }, h("span", { text: tools?.ffmpeg_found ? `ffmpeg ${tools.ffmpeg_version}` : "ffmpeg" }),
            h("span", { class: "mono " + (tools?.ffmpeg_found ? "lime" : "err"), style: { fontSize: "12px" },
              text: tools?.ffmpeg_found ? (tools.ffmpeg_bundled ? App.t("ffmpeg.bundled") : App.t("ffmpeg.system")) : App.t("ffmpeg.missing") })),
          h("div", { class: "between" }, h("span", { text: App.t("adv.recorder") }),
            h("span", { class: "mono muted", id: "recDiag", style: { fontSize: "12px" } })))),
      h("div", null, U.cap(App.t("cap.cache")),
        h("div", { class: "card2 pad stackrows" },
          h("div", { class: "between" }, sizeLine,
            h("button", { class: "btn", text: App.t("cache.clear"), onClick: async () => {
              await api.invoke("clear_cache").catch(() => {});
              App.toast(App.t("cache.cleared"), "ok");
              sizeLine.textContent = App.t("cache.used", { size: App.fmtMB(await api.invoke("cache_size").catch(() => 0)) });
            } })))),
      // RGB is all or nothing: the way out is here (turn off = devices get their own lighting back; remove = also uninstall)
      rgb && rgb.installed ? h("div", null, U.cap("RGB"),
        h("div", { class: "card2 pad stackrows" },
          h("div", { class: "between" }, h("span", { text: App.t("rgb.state." + (rgb.enabled ? "on" : "off")) }),
            h("span", { class: "pathrow" },
              rgb.enabled ? h("button", { class: "btn", text: App.t("rgb.turnOff"), onClick: async (e) => {
                e.currentTarget.disabled = true;
                await api.invoke("rgb_stop").catch(() => {});
                App.toast(App.t("rgb.stopped"), "ok"); App.Settings.render(true);
              } }) : null,
              U.sure(App.t("rgb.remove"), App.t("rgb.removeTip"), async () => {
                try { await api.invoke("rgb_uninstall"); App.toast(App.t("rgb.removed"), "ok"); } catch (e) { const err = App.installError(e); if (err) App.toast(App.t("rgb.err", { err }), "err"); }
                App.Settings.render(true);
              }))))) : null,
      // Fans: the same way out (turn off = every fan goes back to the BIOS; remove = also uninstall the helper)
      fans && fans.installed ? h("div", null, U.cap(App.t("nav.fans")),
        h("div", { class: "card2 pad stackrows" },
          h("div", { class: "between" }, h("span", { text: App.t("fans.state." + (fans.enabled ? "on" : "off")) }),
            h("span", { class: "pathrow" },
              fans.enabled ? h("button", { class: "btn", text: App.t("fans.off"), title: App.t("fans.offTip"), onClick: async (e) => {
                e.currentTarget.disabled = true;
                await api.invoke("fans_disable").catch(() => {});
                App.toast(App.t("fans.offDone"), "ok"); App.Settings.render(true);
              } }) : null,
              U.sure(App.t("fans.removeHelper"), App.t("fans.removeTip"), async () => {
                try { await api.invoke("fans_uninstall"); App.toast(App.t("fans.removed"), "ok"); } catch (e) { const err = App.installError(e); if (err) App.toast(App.t("fans.err", { err }), "err"); }
                App.Settings.render(true);
              }))))) : null));
  }

  // fill the version label once the settings page is on screen
  const showVersion = () => App.Update.version().then((ver) => { const el = App.$("#appVersion"); if (el && el.textContent !== (ver || "?")) el.textContent = ver || "?"; });   // only on change: writing the text is itself a DOM mutation, so an unconditional write loops forever
  new MutationObserver(() => { if (App.S.view === "set") showVersion(); }).observe(document.body, { childList: true, subtree: true });

  /* The recording tab and the first-run wizard live in recording.js. */
  

  App.renderNav = () => { buildTopbar(); renderRail(); };
})();
