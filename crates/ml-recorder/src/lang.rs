//! Text shown by the recorder itself (tray menu, tooltip, popups, errors). The UI language comes from
//! `recorder.json` (`lang`, written by the app); unknown codes fall back to English.
//! Column order of every row: en, pl, de, es, fr, uk, tr, ja, zh. Placeholders look like `{n}`.

use std::sync::RwLock;

const CODES: [&str; 9] = ["en", "pl", "de", "es", "fr", "uk", "tr", "ja", "zh"];

static LANG: RwLock<usize> = RwLock::new(0);

/// Sets the language for all later lookups (call after every config load/reload).
pub fn set(code: &str) {
    let i = CODES.iter().position(|c| *c == code).unwrap_or(0);
    *LANG.write().unwrap() = i;
}

/// Looks `key` up in the current language; the key itself is returned when it is unknown.
pub fn t(key: &str) -> String {
    let i = *LANG.read().unwrap();
    TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v[i].to_string())
        .unwrap_or_else(|| key.to_string())
}

/// Like [`t`], then replaces `{name}` placeholders with the given values.
pub fn f(key: &str, vars: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut s = t(key);
    for (name, v) in vars {
        s = s.replace(&format!("{{{name}}}"), &v.to_string());
    }
    s
}

#[rustfmt::skip]
static TABLE: &[(&str, [&str; 9])] = &[
    ("tip.saved", ["Buffer — clip saved", "Buffer — klip zapisany", "Buffer — Clip gespeichert", "Buffer — clip guardado", "Buffer — clip enregistré", "Buffer — кліп збережено", "Buffer — klip kaydedildi", "Buffer — クリップを保存しました", "Buffer — 片段已保存"]),
    ("tip.save_failed", ["Buffer — couldn't save the clip", "Buffer — nie udało się zapisać klipu", "Buffer — Clip konnte nicht gespeichert werden", "Buffer — no se pudo guardar el clip", "Buffer — échec de l'enregistrement du clip", "Buffer — не вдалося зберегти кліп", "Buffer — klip kaydedilemedi", "Buffer — クリップを保存できませんでした", "Buffer — 无法保存片段"]),
    ("tip.postroll", ["Buffer — saving clip in {n} s (press again = now)", "Buffer — zapis klipu za {n} s (skrót ponownie = od razu)", "Buffer — Clip wird in {n} s gespeichert (nochmal drücken = sofort)", "Buffer — guardando el clip en {n} s (pulsa otra vez = ya)", "Buffer — clip enregistré dans {n} s (réappuyer = tout de suite)", "Buffer — збереження кліпу за {n} с (натисніть ще раз = одразу)", "Buffer — klip {n} sn içinde kaydedilecek (tekrar bas = hemen)", "Buffer — {n} 秒後にクリップを保存（もう一度押すと即保存）", "Buffer — {n} 秒后保存片段（再按一次立即保存）"]),
    ("tip.finalizing", ["Buffer — finishing recording…", "Buffer — kończę zapis nagrania…", "Buffer — Aufnahme wird abgeschlossen…", "Buffer — terminando la grabación…", "Buffer — fin de l'enregistrement…", "Buffer — завершую запис…", "Buffer — kayıt tamamlanıyor…", "Buffer — 録画を終了しています…", "Buffer — 正在结束录制…"]),
    ("tip.longrec", ["Buffer — manual recording · {t}", "Buffer — nagrywanie ręczne · {t}", "Buffer — manuelle Aufnahme · {t}", "Buffer — grabación manual · {t}", "Buffer — enregistrement manuel · {t}", "Buffer — ручний запис · {t}", "Buffer — elle kayıt · {t}", "Buffer — 手動録画中 · {t}", "Buffer — 手动录制中 · {t}"]),
    ("tip.idle", ["Buffer — waiting for a game (records games only)", "Buffer — czeka na grę (nagrywa tylko gry)", "Buffer — wartet auf ein Spiel (nur Spiele)", "Buffer — esperando un juego (solo juegos)", "Buffer — attend un jeu (jeux seulement)", "Buffer — чекає на гру (лише ігри)", "Buffer — oyun bekliyor (yalnızca oyunlar)", "Buffer — ゲーム待機中（ゲームのみ録画）", "Buffer — 等待游戏（仅录制游戏）"]),
    ("tip.starting", ["Buffer — starting recording…", "Buffer — uruchamianie nagrywania…", "Buffer — Aufnahme startet…", "Buffer — iniciando la grabación…", "Buffer — démarrage de l'enregistrement…", "Buffer — запуск запису…", "Buffer — kayıt başlıyor…", "Buffer — 録画を開始しています…", "Buffer — 正在启动录制…"]),
    ("tip.stopped", ["Buffer — not recording: {msg}", "Buffer — nie nagrywa: {msg}", "Buffer — keine Aufnahme: {msg}", "Buffer — no graba: {msg}", "Buffer — n'enregistre pas : {msg}", "Buffer — не записує: {msg}", "Buffer — kaydetmiyor: {msg}", "Buffer — 録画していません: {msg}", "Buffer — 未录制：{msg}"]),
    ("word.stopped", ["stopped", "zatrzymane", "gestoppt", "detenida", "arrêté", "зупинено", "durduruldu", "停止中", "已停止"]),
    ("tip.problem", ["Buffer — check: not working: {list}", "Buffer — uwaga, nie działa: {list}", "Buffer — Achtung, funktioniert nicht: {list}", "Buffer — atención, no funciona: {list}", "Buffer — attention, ne marche pas : {list}", "Buffer — увага, не працює: {list}", "Buffer — dikkat, çalışmıyor: {list}", "Buffer — 注意: 動作していません: {list}", "Buffer — 注意：无法工作：{list}"]),
    ("tip.recording", ["Buffer — recording · buffer {cur}/{max} s{game}", "Buffer — nagrywa · bufor {cur}/{max} s{game}", "Buffer — nimmt auf · Puffer {cur}/{max} s{game}", "Buffer — grabando · búfer {cur}/{max} s{game}", "Buffer — enregistre · tampon {cur}/{max} s{game}", "Buffer — записує · буфер {cur}/{max} с{game}", "Buffer — kaydediyor · arabellek {cur}/{max} sn{game}", "Buffer — 録画中 · バッファ {cur}/{max} 秒{game}", "Buffer — 录制中 · 缓冲 {cur}/{max} 秒{game}"]),
    ("src.Game", ["game audio", "dźwięk gry", "Spielton", "audio del juego", "son du jeu", "звук гри", "oyun sesi", "ゲーム音声", "游戏音频"]),
    ("src.Chat", ["voice chat", "czat głosowy", "Sprachchat", "chat de voz", "chat vocal", "голосовий чат", "sesli sohbet", "ボイスチャット", "语音聊天"]),
    ("src.Mic", ["microphone", "mikrofon", "Mikrofon", "micrófono", "microphone", "мікрофон", "mikrofon", "マイク", "麦克风"]),
    ("pop.postroll", ["Saving clip in {n} s", "Zapisuję klip za {n} s", "Clip wird in {n} s gespeichert", "Guardando el clip en {n} s", "Clip enregistré dans {n} s", "Збереження кліпу за {n} с", "Klip {n} sn içinde kaydedilecek", "{n} 秒後にクリップを保存", "{n} 秒后保存片段"]),
    ("pop.postroll_sub", ["Press the shortcut again to save now", "Naciśnij skrót ponownie, aby zapisać od razu", "Kürzel nochmal drücken, um sofort zu speichern", "Pulsa el atajo otra vez para guardar ya", "Réappuyez sur le raccourci pour enregistrer tout de suite", "Натисніть скорочення ще раз, щоб зберегти одразу", "Hemen kaydetmek için kısayola tekrar bas", "もう一度ショートカットを押すと即保存します", "再按一次快捷键即可立即保存"]),
    ("pop.save_failed", ["Couldn't save the clip", "Nie udało się zapisać klipu", "Clip konnte nicht gespeichert werden", "No se pudo guardar el clip", "Échec de l'enregistrement du clip", "Не вдалося зберегти кліп", "Klip kaydedilemedi", "クリップを保存できませんでした", "无法保存片段"]),
    ("pop.update", ["Buffer {v} is available", "Dostępny Buffer {v}", "Buffer {v} ist verfügbar", "Buffer {v} disponible", "Buffer {v} est disponible", "Доступний Buffer {v}", "Buffer {v} mevcut", "Buffer {v} が利用可能です", "Buffer {v} 已推出"]),
    ("pop.update_sub", ["Open Buffer to update", "Otwórz Buffer, aby zaktualizować", "Buffer öffnen zum Aktualisieren", "Abre Buffer para actualizar", "Ouvrez Buffer pour mettre à jour", "Відкрийте Buffer, щоб оновити", "Güncellemek için Buffer'ı aç", "Buffer を開いて更新", "打开 Buffer 进行更新"]),
    ("pop.saved", ["Clip saved", "Zapisano klip", "Clip gespeichert", "Clip guardado", "Clip enregistré", "Кліп збережено", "Klip kaydedildi", "クリップを保存しました", "片段已保存"]),
    ("pop.active", ["Buffer active", "Buffer aktywny", "Buffer aktiv", "Buffer activo", "Buffer actif", "Buffer активний", "Buffer etkin", "Buffer 動作中", "Buffer 已启用"]),
    ("pop.active_sub", ["{hk} — save the last {len} · {game}", "{hk} — zapisz ostatnie {len} · {game}", "{hk} — letzte {len} speichern · {game}", "{hk} — guardar los últimos {len} · {game}", "{hk} — enregistrer les {len} dernières · {game}", "{hk} — зберегти останні {len} · {game}", "{hk} — son {len} kaydedilir · {game}", "{hk} — 直近 {len} を保存 · {game}", "{hk} — 保存最近 {len} · {game}"]),
    ("pop.down", ["{game}: recording isn't working", "{game}: nagrywanie nie działa", "{game}: Aufnahme funktioniert nicht", "{game}: la grabación no funciona", "{game} : l'enregistrement ne marche pas", "{game}: запис не працює", "{game}: kayıt çalışmıyor", "{game}: 録画が動作していません", "{game}：录制无法工作"]),
    ("pop.down_sub", ["Open Buffer → Settings → Recording", "Otwórz Buffer → Ustawienia → Nagrywanie", "Buffer öffnen → Einstellungen → Aufnahme", "Abre Buffer → Ajustes → Grabación", "Ouvrez Buffer → Réglages → Enregistrement", "Відкрийте Buffer → Параметри → Запис", "Buffer → Ayarlar → Kayıt bölümünü açın", "Buffer → 設定 → 録画 を開いてください", "打开 Buffer → 设置 → 录制"]),
    ("pop.warn", ["Buffer active, but check", "Buffer aktywny, ale uwaga", "Buffer aktiv, aber Achtung", "Buffer activo, pero revisa", "Buffer actif, mais attention", "Buffer активний, але є проблема", "Buffer etkin, ama dikkat", "Buffer は動作中ですが注意", "Buffer 已启用，但需注意"]),
    ("pop.warn_sub", ["Not working: {list} · {hk} saves a clip", "Nie działa: {list} · {hk} zapisuje klip", "Funktioniert nicht: {list} · {hk} speichert einen Clip", "No funciona: {list} · {hk} guarda un clip", "Ne marche pas : {list} · {hk} enregistre un clip", "Не працює: {list} · {hk} зберігає кліп", "Çalışmıyor: {list} · {hk} klip kaydeder", "動作していません: {list} · {hk} でクリップ保存", "无法工作：{list} · {hk} 保存片段"]),
    ("pop.warn_nokey", ["Not working: {list}", "Nie działa: {list}", "Funktioniert nicht: {list}", "No funciona: {list}", "Ne marche pas : {list}", "Не працює: {list}", "Çalışmıyor: {list}", "動作していません: {list}", "无法工作：{list}"]),
    ("pop.health_down", ["Recording stopped working", "Nagrywanie przestało działać", "Aufnahme funktioniert nicht mehr", "La grabación dejó de funcionar", "L'enregistrement ne fonctionne plus", "Запис перестав працювати", "Kayıt çalışmayı durdurdu", "録画が動作しなくなりました", "录制已停止工作"]),
    ("pop.health_warn", ["A source stopped working", "Źródło przestało działać", "Eine Quelle funktioniert nicht mehr", "Una fuente dejó de funcionar", "Une source ne fonctionne plus", "Джерело перестало працювати", "Bir kaynak çalışmayı durdurdu", "ソースが動作しなくなりました", "某个音源已停止工作"]),
    ("pop.bad_clip", ["The saved clip looks broken", "Zapisany klip wygląda na uszkodzony", "Der gespeicherte Clip scheint defekt", "El clip guardado parece dañado", "Le clip enregistré semble corrompu", "Збережений кліп виглядає пошкодженим", "Kaydedilen klip bozuk görünüyor", "保存したクリップが壊れているようです", "已保存的片段似乎已损坏"]),
    ("menu.save", ["Save the last minutes", "Zapisz ostatnie minuty", "Letzte Minuten speichern", "Guardar los últimos minutos", "Enregistrer les dernières minutes", "Зберегти останні хвилини", "Son dakikaları kaydet", "直近の数分を保存", "保存最近几分钟"]),
    ("menu.record", ["Record from now", "Nagrywaj od teraz", "Ab jetzt aufnehmen", "Grabar desde ahora", "Enregistrer maintenant", "Записувати з цього моменту", "Şimdiden kaydet", "今から録画", "从现在开始录制"]),
    ("menu.record_stop", ["Stop recording ({t})", "Zatrzymaj nagrywanie ({t})", "Aufnahme beenden ({t})", "Detener grabación ({t})", "Arrêter l'enregistrement ({t})", "Зупинити запис ({t})", "Kaydı durdur ({t})", "録画を停止 ({t})", "停止录制 ({t})"]),
    ("menu.open", ["Open editor", "Otwórz edytor", "Editor öffnen", "Abrir editor", "Ouvrir l'éditeur", "Відкрити редактор", "Düzenleyiciyi aç", "エディターを開く", "打开编辑器"]),
    ("pop.output", ["Output switched", "Zmieniono wyjście", "Ausgabe gewechselt", "Salida cambiada", "Sortie changée", "Вихід змінено", "Çıkış değişti", "出力を切り替えました", "已切换输出"]),
    ("mix.open", ["Open SoundSwitcher", "Otwórz SoundSwitcher", "SoundSwitcher öffnen", "Abrir SoundSwitcher", "Ouvrir SoundSwitcher", "Відкрити SoundSwitcher", "SoundSwitcher'ı aç", "SoundSwitcher を開く", "打开 SoundSwitcher"]),
    ("pop.mic", ["Microphone switched", "Zmieniono mikrofon", "Mikrofon gewechselt", "Micrófono cambiado", "Micro changé", "Мікрофон змінено", "Mikrofon değişti", "マイクを切り替えました", "已切换麦克风"]),
    ("mix.system", ["System sounds", "Dźwięki systemowe", "Systemsounds", "Sonidos del sistema", "Sons système", "Системні звуки", "Sistem sesleri", "システム音", "系统声音"]),
    ("mix.drop",["Drop here", "Upuść tutaj", "Hier ablegen", "Suelta aquí", "Déposer ici", "Відпустіть тут", "Buraya bırak", "ここにドロップ", "放到这里"]),
    ("mix.dropDefault", ["Back to default", "Wróć do domyślnego", "Zurück zum Standard", "Volver al predeterminado", "Retour au défaut", "Повернути до типового", "Varsayılana dön", "既定に戻す", "回到默认"]),
    ("mix.pinned", ["Pinned", "Przypięta", "Angeheftet", "Fijada", "Épinglée", "Закріплено", "Sabit", "固定", "已固定"]),
    ("mix.idle", ["not playing", "nie gra", "spielt nicht", "no suena", "ne joue pas", "не грає", "çalmıyor", "再生していません", "未播放"]),
    ("mix.unpin", ["Unpin", "Odepnij", "Lösen", "Soltar", "Détacher", "Відкріпити", "Kaldır", "解除", "取消固定"]),
    ("tip.sound", ["{dev} · {n}%", "{dev} · {n}%", "{dev} · {n}%", "{dev} · {n}%", "{dev} · {n}%", "{dev} · {n}%", "{dev} · %{n}", "{dev} · {n}%", "{dev} · {n}%"]),
    ("tip.sound_muted", ["{dev} · muted", "{dev} · wyciszone", "{dev} · stumm", "{dev} · silenciado", "{dev} · muet", "{dev} · без звуку", "{dev} · sessiz", "{dev} · ミュート", "{dev} · 已静音"]),
    ("menu.output", ["Output", "Wyjście", "Ausgabe", "Salida", "Sortie", "Вихід", "Çıkış", "出力", "输出"]),
    ("menu.eq", ["EQ", "EQ", "EQ", "EQ", "EQ", "EQ", "EQ", "EQ", "均衡器"]),
    ("menu.eq_on", ["EQ on", "EQ włączony", "EQ an", "EQ activado", "EQ activé", "EQ увімкнено", "EQ açık", "EQ オン", "开启均衡器"]),
    ("menu.eq_preset", ["Preset", "Preset", "Preset", "Preajuste", "Préréglage", "Пресет", "Ön ayar", "プリセット", "预设"]),
    ("menu.listen", ["Hear my mic", "Odsłuch mikrofonu", "Mikrofon mithören", "Escuchar mi micrófono", "M'entendre au micro", "Чути свій мікрофон", "Mikrofonumu duy", "マイクをモニター", "监听麦克风"]),
    ("preset.flat", ["Flat", "Płasko", "Neutral", "Plano", "Neutre", "Рівно", "Düz", "フラット", "平直"]),
    ("preset.gaming", ["Footsteps", "Kroki", "Schritte", "Pasos", "Pas", "Кроки", "Ayak sesleri", "足音", "脚步"]),
    ("preset.voice", ["Voice", "Głos", "Stimme", "Voz", "Voix", "Голос", "Ses", "ボイス", "人声"]),
    ("preset.music", ["Music", "Muzyka", "Musik", "Música", "Musique", "Музика", "Müzik", "音楽", "音乐"]),
    ("menu.quit", ["Quit Buffer", "Zamknij Buffer", "Buffer beenden", "Cerrar Buffer", "Quitter Buffer", "Закрити Buffer", "Buffer'dan çık", "Buffer を終了", "退出 Buffer"]),
    ("lr.started", ["Manual recording started", "Nagrywanie ręczne rozpoczęte", "Manuelle Aufnahme gestartet", "Grabación manual iniciada", "Enregistrement manuel démarré", "Ручний запис розпочато", "Elle kayıt başladı", "手動録画を開始しました", "已开始手动录制"]),
    ("lr.started_sub", ["{hk} stops", "{hk} kończy", "{hk} beendet", "{hk} detiene", "{hk} arrête", "{hk} зупиняє", "{hk} durdurur", "{hk} で停止", "{hk} 停止"]),
    ("lr.start_failed", ["Couldn't start recording", "Nie udało się rozpocząć nagrywania", "Aufnahme konnte nicht gestartet werden", "No se pudo iniciar la grabación", "Impossible de démarrer l'enregistrement", "Не вдалося розпочати запис", "Kayıt başlatılamadı", "録画を開始できませんでした", "无法开始录制"]),
    ("lr.saved", ["Recording saved", "Zapisano nagranie", "Aufnahme gespeichert", "Grabación guardada", "Enregistrement sauvegardé", "Запис збережено", "Kayıt kaydedildi", "録画を保存しました", "录制已保存"]),
    ("lr.stopped", ["Stopped: {r}", "Zatrzymano: {r}", "Gestoppt: {r}", "Detenida: {r}", "Arrêté : {r}", "Зупинено: {r}", "Durduruldu: {r}", "停止: {r}", "已停止：{r}"]),
    ("lr.save_failed", ["Couldn't save the recording", "Nie udało się zapisać nagrania", "Aufnahme konnte nicht gespeichert werden", "No se pudo guardar la grabación", "Échec de la sauvegarde de l'enregistrement", "Не вдалося зберегти запис", "Kayıt kaydedilemedi", "録画を保存できませんでした", "无法保存录制"]),
    ("lr.remind", ["Manual recording is still running", "Nagrywanie ręczne nadal trwa", "Die manuelle Aufnahme läuft noch", "La grabación manual sigue en curso", "L'enregistrement manuel est toujours en cours", "Ручний запис досі триває", "Elle kayıt hâlâ sürüyor", "手動録画はまだ続いています", "手动录制仍在进行"]),
    ("lr.remind_sub", ["{h} h so far · {hk} stops", "Trwa już {h} h · {hk} kończy", "Bisher {h} Std. · {hk} beendet", "Lleva {h} h · {hk} detiene", "Depuis {h} h · {hk} arrête", "Триває вже {h} год · {hk} зупиняє", "{h} saattir sürüyor · {hk} durdurur", "{h} 時間経過 · {hk} で停止", "已进行 {h} 小时 · {hk} 停止"]),
    ("pop.nothing", ["Nothing to save yet", "Nie ma jeszcze czego zapisać", "Noch nichts zu speichern", "Aún no hay nada que guardar", "Rien à enregistrer pour l'instant", "Поки нічого зберігати", "Henüz kaydedilecek bir şey yok", "まだ保存するものがありません", "暂无可保存的内容"]),
    ("pop.nothing_idle", ["Recording starts when a game does", "Nagrywanie rusza razem z grą", "Die Aufnahme startet mit einem Spiel", "La grabación empieza con un juego", "L'enregistrement démarre avec un jeu", "Запис почнеться разом із грою", "Kayıt bir oyunla birlikte başlar", "ゲームの起動と同時に録画が始まります", "游戏启动后开始录制"]),
    ("pop.nothing_stopped", ["Recording is stopped", "Nagrywanie jest zatrzymane", "Die Aufnahme ist gestoppt", "La grabación está detenida", "L'enregistrement est arrêté", "Запис зупинено", "Kayıt durdurulmuş", "録画は停止中です", "录制已停止"]),
    ("err.empty", ["nothing has been recorded yet", "nic jeszcze nie nagrano", "es wurde noch nichts aufgenommen", "todavía no se ha grabado nada", "rien n'a encore été enregistré", "ще нічого не записано", "henüz bir şey kaydedilmedi", "まだ何も録画されていません", "尚未录制任何内容"]),
    ("err.read", ["the buffer wasn't ready, try again in a moment", "bufor nie był gotowy, spróbuj za chwilę", "der Puffer war nicht bereit, gleich nochmal versuchen", "el búfer no estaba listo, inténtalo de nuevo en un momento", "le tampon n'était pas prêt, réessayez dans un instant", "буфер не був готовий, спробуйте за мить", "arabellek hazır değildi, birazdan tekrar deneyin", "バッファの準備ができていません。少し待ってからもう一度お試しください", "缓冲尚未就绪，请稍后再试"]),
    ("err.busy", ["recording is already running", "nagrywanie już trwa", "Aufnahme läuft bereits", "ya hay una grabación en curso", "un enregistrement est déjà en cours", "запис уже триває", "kayıt zaten sürüyor", "すでに録画中です", "录制已在进行"]),
    ("err.space", ["not enough disk space", "za mało miejsca na dysku", "zu wenig Speicherplatz", "poco espacio en disco", "espace disque insuffisant", "замало місця на диску", "yetersiz disk alanı", "ディスク容量が不足しています", "磁盘空间不足"]),
    ("err.disk_full", ["the disk is almost full", "dysk jest prawie pełny", "Datenträger fast voll", "el disco está casi lleno", "le disque est presque plein", "диск майже заповнений", "disk neredeyse dolu", "ディスクがほぼ満杯です", "磁盘几乎已满"]),
    ("err.long_limit", ["the 12-hour limit was reached", "osiągnięto limit 12 godzin", "das 12-Stunden-Limit wurde erreicht", "se alcanzó el límite de 12 horas", "la limite de 12 heures est atteinte", "досягнуто ліміту 12 годин", "12 saatlik sınıra ulaşıldı", "12 時間の上限に達しました", "已达到 12 小时上限"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_match_english_in_every_language() {
        let ph = |s: &str| {
            let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split('}').next().map(String::from)).collect();
            v.sort();
            v
        };
        for (key, row) in TABLE {
            for (i, s) in row.iter().enumerate() {
                assert!(!s.is_empty(), "{key} empty for {}", CODES[i]);
                assert_eq!(ph(row[0]), ph(s), "{key} placeholders differ for {}", CODES[i]);
            }
        }
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        set("xx");
        assert_eq!(t("menu.quit"), "Quit Buffer");
        set("pl");
        assert_eq!(f("pop.postroll", &[("n", &5)]), "Zapisuję klip za 5 s");
        set("en");
    }
}
