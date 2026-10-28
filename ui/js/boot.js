"use strict";
/* Start-up: load persisted state, theme, library, recorder polling, global keys. */

(async function boot() {
  // independent start-up questions go out together; the window stays hidden until the first screen is drawn
  const [, clipsHome, isDev, first] = await Promise.all([
    App.loadPersisted(),
    App.api.invoke("default_clips_dir").catch(() => ""),
    App.api.invoke("is_dev_build").catch(() => false),
    App.api.invoke("startup_view").catch(() => null),
  ]);
  // %USERPROFILE%\Videos\Buffer: where a fresh install saves clips, and the export folder's fallback
  App.clipsHome = clipsHome;
  // First launch on this machine: library = the default clips folder.
  if (App.fresh) App.P.libDirs = [App.clipsHome].filter(Boolean);
  App.isDev = isDev;
  if (App.isDev) document.title = "Buffer (dev)";
  await App.i18n.init();
  App.applyTheme();
  App.renderNav();

  document.addEventListener("keydown", (ev) => {
    const t = ev.target;
    const typing = t instanceof HTMLInputElement && !["range", "checkbox"].includes(t.type);
    if ((ev.ctrlKey || ev.metaKey) && (ev.key === "f" || ev.key === "F" || ev.key === "k" || ev.key === "K")) {
      ev.preventDefault();
      App.focusSearch();
      return;
    }
    if (typing || t instanceof HTMLSelectElement || t instanceof HTMLTextAreaElement) return;
    if (ev.key === "Escape" && App.S.view === "lib" && (App.S.sel.size || App.S.active)) { App.Lib.clearSel(); return; }
    if (App.S.view === "ed" && App.Ed.key(ev)) ev.preventDefault();
  });

  // Mouse "Back" (button 4): closes the export popover, else leaves the editor / settings for the library.
  const onBack = (ev) => {
    if (ev.button !== 3 && ev.button !== 4) return;
    ev.preventDefault();
    if (ev.type === "mouseup" || ev.type === "auxclick") (ev.button === 3 ? App.back : App.forward)();
  };
  for (const t of ["mousedown", "mouseup", "auxclick"]) window.addEventListener(t, onBack, true);
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "BrowserBack" || (ev.altKey && ev.key === "ArrowLeft")) { ev.preventDefault(); App.back(); }
    if (ev.key === "BrowserForward" || (ev.altKey && ev.key === "ArrowRight")) { ev.preventDefault(); App.forward(); }
  }, true);

  // Pick up clips the recorder saved while the window was in the background.
  window.addEventListener("focus", () => { if (App.S.view === "lib") App.Lib.load(); });

  App.Rec.start();
  App.Update.start();
  // launched for a page (the tray flyout's "open" button runs `buffer.exe --view=snd`)
  const VIEWS_OK = ["lib", "eq", "snd", "rgb", "fans", "set"];
  const view = VIEWS_OK.includes(first) ? first : "lib";
  if (view === "rgb" || view === "fans") await App.loadHw();
  App.go(view);
  App.api.listen("open-view", (e) => { if (VIEWS_OK.includes(e.payload)) App.go(e.payload); });
  // the window starts hidden; show it with the page drawn (the host shows it after 4 s anyway). The clip list fills in
  // right after: walking big clip folders must not keep the window away.
  requestAnimationFrame(() => App.api.invoke("show_main_window").catch(() => {}));
  await App.Lib.load();
  // RGB and Fans are not needed for the first screen: fetch them once the app is idle, so their first open is instant
  requestIdleCallback(() => App.loadHw().catch(() => {}), { timeout: 5000 });
  App.Exports.autoClean();            // only does anything when "Delete old exports" is set

  // First run: walk the user through the settings that matter.
  if (!App.P.settings.onboarded) setTimeout(() => App.RecUI.wizard(), 400);

  // If the user asked for recording at login but the recorder is not up (crashed, killed), start it.
  if (App.P.settings.onboarded) App.RecUI.syncLang().catch(() => {});   // existing installs: recorder follows the app language
  try {
    await App.Rec.refresh();
    if (!App.S.recorder && (await App.api.invoke("recorder_autostart", { enable: null }))) {
      await App.api.invoke("recorder_launch");
      setTimeout(App.Rec.refresh, 2000);
    }
  } catch { /* recorder binary not present (development layout) */ }
})().catch((e) => {
  console.error(e);
  document.body.prepend(Object.assign(document.createElement("pre"), { textContent: App.t("boot.failed", { err: e && e.stack || e }), style: "color:#ff6b6b;padding:20px;white-space:pre-wrap" }));
});
