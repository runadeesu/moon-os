//! Audio subsystem state: wraps the AC97 driver the same way `net` wraps
//! the RTL8139 -- a single `Mutex`-guarded optional device, safe to call
//! into even when no audio hardware exists (every call just becomes a
//! no-op and callers get an honest "no audio device" status).

use crate::drivers::ac97::Ac97;
use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;

static DEVICE: Mutex<Option<Ac97>> = Mutex::new(None);
/// Last volume the user actually set, 0-100. Read back by Settings/taskbar
/// so the slider reflects real state instead of resetting on redraw.
static VOLUME: AtomicU8 = AtomicU8::new(100);

pub fn init() {
    match crate::drivers::ac97::init() {
        Some(dev) => {
            dev.set_volume(VOLUME.load(Ordering::Relaxed));
            *DEVICE.lock() = Some(dev);
        }
        None => crate::serial_println!("ac97: no audio controller found, audio disabled"),
    }
}

pub fn is_up() -> bool {
    DEVICE.lock().is_some()
}

pub fn volume() -> u8 {
    VOLUME.load(Ordering::Relaxed)
}

/// Sets master volume (0-100) and writes it to the real mixer register if
/// a codec is present. Always updates the stored value, even with no
/// hardware, so Settings/taskbar UI stays consistent -- but `is_up()` is
/// what callers should check before claiming the change had any audible
/// effect.
pub fn set_volume(percent: u8) {
    let percent = percent.min(100);
    VOLUME.store(percent, Ordering::Relaxed);
    if let Some(dev) = DEVICE.lock().as_ref() {
        dev.set_volume(percent);
    }
}

/// Plays the system beep (e.g. for a UI click or notification) if a codec
/// is present; a silent no-op otherwise. `main` triggers a mute check first
/// -- volume 0 skips the DMA round-trip entirely rather than looping through
/// hardware just to end up silent.
pub fn beep() {
    if VOLUME.load(Ordering::Relaxed) == 0 {
        return;
    }
    if let Some(dev) = DEVICE.lock().as_mut() {
        dev.beep(880, 8000);
    }
}

/// Short taskbar tray label: real hardware state, not a decorative icon.
pub fn status_summary() -> &'static str {
    if !is_up() {
        "no audio device"
    } else if VOLUME.load(Ordering::Relaxed) == 0 {
        "muted"
    } else {
        "audio ready"
    }
}
