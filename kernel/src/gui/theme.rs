//! The one genuinely OS-wide visual setting: an accent color used for
//! glow borders, focus highlights, and other neon touches across the
//! window chrome, dock, top bar, desktop widgets, notifications, and File
//! Manager. Changed from Settings (a click cycles to the next preset) and
//! read by every module that used to hardcode its own copy of the same
//! cyan constant -- so changing it here is a real, OS-wide effect, not a
//! setting that only updates one screen.

use spin::Mutex;

pub const PRESETS: [((u8, u8, u8), &str); 5] = [
    ((0x30, 0xE0, 0xFF), "Cyan"),
    ((0xE0, 0x40, 0xC0), "Magenta"),
    ((0x50, 0xE8, 0x80), "Green"),
    ((0xE8, 0x90, 0x30), "Orange"),
    ((0x90, 0x60, 0xE8), "Purple"),
];

static ACCENT_INDEX: Mutex<usize> = Mutex::new(0);

pub fn accent() -> (u8, u8, u8) {
    PRESETS[*ACCENT_INDEX.lock()].0
}

pub fn accent_name() -> &'static str {
    PRESETS[*ACCENT_INDEX.lock()].1
}

pub fn cycle_accent() {
    let mut idx = ACCENT_INDEX.lock();
    *idx = (*idx + 1) % PRESETS.len();
}
