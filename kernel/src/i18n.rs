//! Minimal UI internationalization: a handful of translatable labels used
//! by the desktop chrome (the Settings window and the system-monitor
//! desktop widget), switchable live by clicking the language row in
//! Settings (see `gui/widgets/settings.rs`).
//!
//! Japanese labels are spelled entirely in hiragana. There's no kanji or
//! katakana glyph table yet -- only `font_hiragana.rs`'s public-domain
//! hiragana set -- so words that would conventionally be katakana loanwords
//! ("system", "task") or kanji compounds ("memory", "info") are respelled
//! phonetically in hiragana, the same trick Japanese children's books use
//! before kanji is learned. "Terminal" and the "moon OS"/"NET" chrome are
//! left as-is in every language, same as a real desktop keeps brand names
//! and protocol acronyms untranslated.

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    English,
    Japanese,
    Spanish,
    French,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::English, Lang::Japanese, Lang::Spanish, Lang::French];

    pub fn name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Japanese => "Japanese",
            Lang::Spanish => "Spanish",
            Lang::French => "French",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|l| *l == self).unwrap_or(0)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Lang {
    Lang::ALL[(CURRENT.load(Ordering::Relaxed) as usize) % Lang::ALL.len()]
}

/// Advances to the next language in `Lang::ALL`, wrapping around -- called
/// when the Settings window's language row is clicked.
pub fn cycle() {
    let next = (current().index() + 1) % Lang::ALL.len();
    CURRENT.store(next as u8, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
pub enum Key {
    SettingsTitle,
    SystemInfoHeader,
    MemoryLabel,
    TasksLabel,
    UptimeLabel,
    SystemMonitorHeader,
    LangHint,
}

/// Hiragana table index `i` (0..96, see `font_hiragana.rs`) is addressed as
/// byte `0x80 + i`; bytes below 0x80 are plain ASCII (`glyph_bits` in
/// `framebuffer.rs` is the other half of this encoding).
const fn h(i: u8) -> u8 {
    0x80 + i
}

// settei (setei) -- "settings" / 設定
const JA_SETTINGS_TITLE: [u8; 4] = [h(27), h(35), h(38), h(4)];
// shisutemu jouhou -- "system info" / システム情報
const JA_SYSTEM_INFO: [u8; 10] = [
    h(23),
    h(25),
    h(38),
    h(64),
    b' ',
    h(24),
    h(71),
    h(6),
    h(59),
    h(6),
];
// kioku -- "memory" / 記憶
const JA_MEMORY: [u8; 3] = [h(13), h(10), h(15)];
// tasuku -- "task(s)" / タスク
const JA_TASKS: [u8; 3] = [h(31), h(25), h(15)];
// jikan -- "time" / 時間
const JA_UPTIME: [u8; 3] = [h(24), h(11), h(83)];
// shisutemu kanshi -- "system monitor" / システム監視
const JA_SYSTEM_MONITOR: [u8; 8] = [h(23), h(25), h(38), h(64), b' ', h(11), h(83), h(23)];
// gengo wo kaeru -- "change language" / 言語を変える
const JA_LANG_HINT: [u8; 7] = [h(18), h(83), h(20), h(82), h(11), h(8), h(75)];

/// Looks up the label for `key` in the currently selected language, encoded
/// as raw bytes (not `&str` -- the hiragana byte range isn't valid UTF-8),
/// meant for `framebuffer::draw_glyphs_at`.
pub fn tr(key: Key) -> &'static [u8] {
    match (current(), key) {
        (Lang::English, Key::SettingsTitle) => b"Settings",
        (Lang::English, Key::SystemInfoHeader) => b"system info",
        (Lang::English, Key::MemoryLabel) => b"memory",
        (Lang::English, Key::TasksLabel) => b"tasks",
        (Lang::English, Key::UptimeLabel) => b"uptime",
        (Lang::English, Key::SystemMonitorHeader) => b"system monitor",
        (Lang::English, Key::LangHint) => b"click to change language",

        (Lang::Spanish, Key::SettingsTitle) => b"Ajustes",
        (Lang::Spanish, Key::SystemInfoHeader) => b"info del sistema",
        (Lang::Spanish, Key::MemoryLabel) => b"memoria",
        (Lang::Spanish, Key::TasksLabel) => b"tareas",
        (Lang::Spanish, Key::UptimeLabel) => b"tiempo activo",
        (Lang::Spanish, Key::SystemMonitorHeader) => b"monitor del sistema",
        (Lang::Spanish, Key::LangHint) => b"clic para cambiar idioma",

        (Lang::French, Key::SettingsTitle) => b"Parametres",
        (Lang::French, Key::SystemInfoHeader) => b"infos systeme",
        (Lang::French, Key::MemoryLabel) => b"memoire",
        (Lang::French, Key::TasksLabel) => b"taches",
        (Lang::French, Key::UptimeLabel) => b"temps actif",
        (Lang::French, Key::SystemMonitorHeader) => b"moniteur systeme",
        (Lang::French, Key::LangHint) => b"cliquer pour changer",

        (Lang::Japanese, Key::SettingsTitle) => &JA_SETTINGS_TITLE,
        (Lang::Japanese, Key::SystemInfoHeader) => &JA_SYSTEM_INFO,
        (Lang::Japanese, Key::MemoryLabel) => &JA_MEMORY,
        (Lang::Japanese, Key::TasksLabel) => &JA_TASKS,
        (Lang::Japanese, Key::UptimeLabel) => &JA_UPTIME,
        (Lang::Japanese, Key::SystemMonitorHeader) => &JA_SYSTEM_MONITOR,
        (Lang::Japanese, Key::LangHint) => &JA_LANG_HINT,
    }
}
