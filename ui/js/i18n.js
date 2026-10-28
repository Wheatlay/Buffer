"use strict";
/* Translations. Text lives in js/locales/<code>.js (en is the master and the fallback for missing keys).
   Always call App.t() when a label is drawn: the language is only known after the saved state has loaded,
   so labels must never be translated once at script load. */

App.locales = {};
App.i18n = (function () {
  const ORDER = ["en", "pl", "de", "es", "fr", "uk", "tr", "ja", "zh"];
  // what the language list needs without loading every locale file (keep in step with js/locales/<code>.js)
  const META = {
    en: { name: "English", tag: "en-US", ai: true }, pl: { name: "Polski", tag: "pl-PL", ai: false },
    de: { name: "Deutsch", tag: "de-DE", ai: true }, es: { name: "Español", tag: "es-ES", ai: true },
    fr: { name: "Français", tag: "fr-FR", ai: true }, uk: { name: "Українська", tag: "uk-UA", ai: true },
    tr: { name: "Türkçe", tag: "tr-TR", ai: true }, ja: { name: "日本語", tag: "ja-JP", ai: true },
    zh: { name: "简体中文", tag: "zh-CN", ai: true },
  };
  // only English ships in index.html; the UI language (and a suggested one) load in init
  const load = (code) => (code === "en" || !META[code] ? Promise.resolve() : App.loadScripts([`js/locales/${code}.js`]).catch(() => {}));
  let lang = "en";
  let auto = false;          // the language was taken from Windows on first run (the wizard then offers English)
  const rules = {};
  const rule = (code) => (rules[code] ||= new Intl.PluralRules(App.locales[code].tag));

  function format(code, key, vars) {
    let src = code, v = App.locales[code]?.t[key];
    if (v === undefined) { src = "en"; v = App.locales.en.t[key]; }
    if (v === undefined) return key;
    if (typeof v === "object") v = v[rule(src).select(vars?.n ?? 0)] ?? v.other;
    return vars ? v.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? vars[k] : m)) : v;
  }

  App.t = (key, vars) => format(lang, key, vars);
  /** A string in a specific language (used to word the "switch language" suggestion in that language). */
  App.tIn = (code, key, vars) => format(code, key, vars);
  /** BCP 47 tag of the UI language, for Intl and date formatting. */
  App.tag = () => App.locales[lang].tag;

  /* Bundled fonts cover Latin only; Cyrillic and CJK fall back to Windows' own UI fonts under the same family names. */
  const SYSTEM = {
    cyr: ["Segoe UI"],
    ja: ["Yu Gothic UI", "Meiryo UI", "Microsoft YaHei UI"],
    zh: ["Microsoft YaHei UI", "Microsoft YaHei", "SimSun"],
  };
  function installFonts(code) {
    const faces = [];
    const add = (names, range) => {
      const src = names.map((n) => `local("${n}")`).join(", ");
      for (const fam of ["Instrument Sans", "JetBrains Mono"]) {
        faces.push(`@font-face { font-family: "${fam}"; font-weight: 100 900; src: ${src}; unicode-range: ${range}; }`);
      }
    };
    add(SYSTEM.cyr, "U+0400-052F, U+2DE0-2DFF, U+A640-A69F");
    if (code === "ja" || code === "zh") add(SYSTEM[code], "U+2E80-A4CF, U+AC00-D7AF, U+F900-FAFF, U+FE30-FE4F, U+FF00-FFEF, U+3000-303F");
    let st = document.getElementById("i18n-fonts");
    if (!st) { st = document.createElement("style"); st.id = "i18n-fonts"; document.head.append(st); }
    st.textContent = faces.join("\n");
  }

  /** Picks the UI language and loads its texts (call once after the saved state is loaded). */
  async function init() {
    const s = App.P.settings;
    // first run (the wizard not finished, no language ever picked or dismissed): start in the OS language when we have it,
    // so a Polish Windows opens in Polish. A choice made with setLang (or the hint's ✕) sets langHint and is kept.
    if (!s.onboarded && !s.langHint) {
      const d = detect() || "en";
      if (d !== s.lang) { s.lang = d; App.persist(); }
      auto = d !== "en";
    }
    const want = s.lang;
    await load(want);
    lang = App.locales[want] ? want : "en";
    const sug = suggestion();
    if (sug) await load(sug);
    document.documentElement.lang = App.locales[lang].tag;   // also makes CSS uppercase right for Turkish
    installFonts(lang);
  }

  /** The OS language, when it is one we ship and not English; null otherwise. Only ever a suggestion. */
  function detect() {
    for (const raw of navigator.languages || [navigator.language]) {
      const tag = String(raw).toLowerCase();
      const base = tag.split("-")[0];
      if (base === "en") return null;
      if (base === "zh" && /tw|hk|mo|hant/.test(tag)) continue;   // we only have Simplified Chinese
      if (META[base]) return base;
    }
    return null;
  }

  /** The language to suggest, or null (already handled, already using it, or nothing to suggest). */
  const suggestion = () => {
    if (App.P.settings.langHint) return null;
    if (auto) return lang !== "en" ? "en" : null;   // started in the Windows language: English stays one click away
    const d = detect();
    return d && d !== lang ? d : null;
  };

  const dismissHint = () => { App.P.settings.langHint = true; App.persist(); };

  /** Switching reloads the window so every label is rebuilt in the new language; state is saved first. */
  async function setLang(code) {
    App.P.settings.lang = code;
    App.P.settings.langHint = true;
    await App.api.invoke("save_state", { state: App.P }).catch(() => {});
    // the recorder (tray menu, popups) follows the app language; best effort, it may not be installed
    if (App.P.settings.onboarded) {
      try { await App.api.invoke("set_recorder_config", { config: { ...(await App.api.invoke("recorder_config")), lang: code } }); await App.api.invoke("recorder_cmd", { cmd: "reload" }); } catch { /* recorder not available */ }
    }
    location.reload();
  }

  /** One-line "switch to <language>" link worded in that language, for the first-run wizard. */
  function hintLink(onDone) {
    const code = suggestion();
    if (!code) return null;
    const h = App.h;
    return h("div", { class: "langhint inline" },
      h("button", { class: "linkbtn lime", text: App.tIn(code, "lang.switchTo"), onClick: () => setLang(code) }),
      META[code].ai ? h("span", { class: "muted", title: App.tIn(code, "lang.aiTip"), text: App.tIn(code, "lang.ai") }) : null,
      // started in a machine-translated Windows language: say so, as the link did before
      auto && META[lang].ai ? h("span", { class: "muted", title: App.t("lang.aiTip"), text: App.t("lang.ai") }) : null,
      h("button", { class: "linkbtn", text: "✕", onClick: (e) => { dismissHint(); e.currentTarget.parentElement.remove(); onDone?.(); } }));
  }

  /** Top-bar chip suggesting the system language (null when there is nothing to suggest). Click switches; ✕ dismisses for good. */
  function topbarChip() {
    const code = auto ? null : suggestion();         // the wizard's link offers English; no chip on top of it
    if (!code) return null;
    const h = App.h;
    const L = META[code];
    return h("div", { class: "langchip", title: L.ai ? App.tIn(code, "lang.aiTip") : "" },
      h("button", { class: "go", text: "🌐 " + App.tIn(code, "lang.switchTo"), onClick: () => setLang(code) }),
      h("button", { class: "x", text: "✕", "aria-label": App.t("common.dismiss"), onClick: (e) => { dismissHint(); e.currentTarget.parentElement.remove(); } }));
  }

  return { ORDER, META, init, detect, suggestion, setLang, hintLink, topbarChip, current: () => lang };
})();
