//! Mouse sensitivity: a real percentage scale applied to every PS/2
//! packet's raw dx/dy before it moves the cursor.
//!
//! Under QEMU/VirtualBox (no Guest Additions, no absolute-pointer/USB-HID
//! driver in this kernel -- just the legacy relative PS/2 protocol), the
//! virtual mouse commonly reports deltas that move the cursor much further
//! per physical movement than real PS/2 hardware would, especially with a
//! high-DPI host mouse or trackpad: a small hand movement turns into the
//! cursor sliding all the way across the screen. Lower-than-100% is the
//! honest fix for that -- a real damping factor applied to real hardware
//! deltas, adjustable from Settings, not a cosmetic slider with nothing
//! behind it.

use core::sync::atomic::{AtomicU8, Ordering};

pub const PRESETS: [(u8, &str); 3] = [(35, "Slow"), (60, "Normal"), (100, "Fast")];
static INDEX: AtomicU8 = AtomicU8::new(1); // "Normal" (60%) by default.

pub fn percent() -> u8 {
    PRESETS[INDEX.load(Ordering::Relaxed) as usize % PRESETS.len()].0
}

pub fn name() -> &'static str {
    PRESETS[INDEX.load(Ordering::Relaxed) as usize % PRESETS.len()].1
}

pub fn cycle() {
    INDEX
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |i| {
            Some((i + 1) % PRESETS.len() as u8)
        })
        .ok();
}

/// Scales one axis of raw PS/2 delta. Preserves a minimum +/-1 step when
/// the input is nonzero -- so slow, precise movement stays responsive even
/// at a low percentage, while fast flicks (the actual "skiing" complaint)
/// get proportionally damped instead of rounding to a dead zone.
pub fn scale(delta: i32) -> i32 {
    if delta == 0 {
        return 0;
    }
    let scaled = delta * i32::from(percent()) / 100;
    if scaled == 0 {
        delta.signum()
    } else {
        scaled
    }
}
