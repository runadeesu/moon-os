//! PS/2 (auxiliary port) mouse driver: standard 3-byte packet protocol, no
//! scroll wheel (IntelliMouse extensions can come later if needed).

use super::ps2;
use core::sync::atomic::{AtomicU8, Ordering};

static PACKET_INDEX: AtomicU8 = AtomicU8::new(0);
static mut PACKET: [u8; 3] = [0; 3];

/// Enables the controller's auxiliary port and asks the mouse to start
/// streaming movement packets. Returns `false` (without panicking) if any
/// step doesn't ACK -- e.g. no mouse present -- so boot can continue either way.
pub fn enable() -> bool {
    if !ps2::write_command(0xA8) {
        return false; // enable auxiliary device
    }

    if !ps2::write_command(0x20) {
        return false; // "read controller configuration byte"
    }
    let Some(mut config) = ps2::read_data() else {
        return false;
    };
    config |= 0x02; // enable IRQ12
    config &= !0x20; // enable the aux port's clock
    if !ps2::write_command(0x60) || !ps2::write_data(config) {
        return false; // "write controller configuration byte"
    }

    send_mouse_command(0xF6) && send_mouse_command(0xF4) // defaults, then enable reporting
}

fn send_mouse_command(cmd: u8) -> bool {
    // 0xD4 tells the controller the next data byte is for the aux port.
    if !ps2::write_command(0xD4) || !ps2::write_data(cmd) {
        return false;
    }
    matches!(ps2::read_data(), Some(0xFA))
}

/// Bit 3 of a packet's first byte is always set on a real PS/2 mouse --
/// the one fixed marker we have to find (or keep) alignment with the
/// 3-byte packet stream.
const SYNC_BIT: u8 = 0x08;

/// Called from the IRQ12 handler with one byte of a 3-byte packet.
///
/// Framing is checked on every byte 0 candidate, not just after a full
/// packet is assembled: if a byte is ever dropped or duplicated (plausible
/// under bursty input -- rapid programmatic mouse events, a missed IRQ),
/// grouping blindly into fixed 3-byte windows would lock onto the wrong
/// offset *permanently*, since nothing after that ever re-checks alignment
/// mid-stream. Rejecting an invalid byte 0 without advancing keeps
/// re-trying at the next byte until sync is found again, so a one-off
/// framing glitch degrades input for at most a couple of bytes instead of
/// wedging the mouse dead for the rest of the session.
pub fn handle_irq() {
    let Some(byte) = ps2::read_data() else {
        return;
    };

    let idx = PACKET_INDEX.load(Ordering::Relaxed);
    if idx == 0 && byte & SYNC_BIT == 0 {
        return; // not a valid packet start; drop and try the next byte
    }
    unsafe { PACKET[idx as usize] = byte };

    if idx < 2 {
        PACKET_INDEX.store(idx + 1, Ordering::Relaxed);
        return;
    }
    PACKET_INDEX.store(0, Ordering::Relaxed);
    process_packet();
}

fn process_packet() {
    let packet = unsafe { PACKET };
    let flags = packet[0];

    // Bits 6/7 mark X/Y overflow -- a real mouse only sets these on a
    // movement far too large for one packet to encode, which never
    // legitimately happens under emulation. In practice this fires on a
    // mis-framed packet (a byte 0 candidate that happened to have the sync
    // bit set by coincidence); dropping it here is cheap insurance on top
    // of the byte-0 resync in `handle_irq`.
    if flags & 0xC0 != 0 {
        return;
    }

    let mut dx = i32::from(packet[1]);
    let mut dy = i32::from(packet[2]);
    if flags & 0x10 != 0 {
        dx -= 256;
    }
    if flags & 0x20 != 0 {
        dy -= 256;
    }
    let left = flags & 0x01 != 0;
    let right = flags & 0x02 != 0;
    let middle = flags & 0x04 != 0;

    if dx != 0 || dy != 0 || left || right || middle {
        crate::gui::on_mouse(dx, dy, left, right, middle);
    }
}
