//! Low-level i8042 PS/2 controller access, shared by the keyboard and mouse
//! drivers. We rely on firmware having already run the controller's own
//! self-test at boot (true of BIOS and every OVMF/QEMU setup); this just
//! talks to the two ports it exposes.

use crate::arch::x86_64::port::{inb, outb};

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const COMMAND_PORT: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

/// Generous but bounded spin count so a missing/misbehaving controller
/// (unlikely, but not worth risking on real hardware) can't hang boot forever.
const SPIN_LIMIT: u32 = 100_000;

fn wait_for_write() -> bool {
    for _ in 0..SPIN_LIMIT {
        if unsafe { inb(STATUS_PORT) } & STATUS_INPUT_FULL == 0 {
            return true;
        }
    }
    false
}

fn wait_for_read() -> bool {
    for _ in 0..SPIN_LIMIT {
        if unsafe { inb(STATUS_PORT) } & STATUS_OUTPUT_FULL != 0 {
            return true;
        }
    }
    false
}

pub fn read_data() -> Option<u8> {
    wait_for_read().then(|| unsafe { inb(DATA_PORT) })
}

pub fn write_data(byte: u8) -> bool {
    if !wait_for_write() {
        return false;
    }
    unsafe { outb(DATA_PORT, byte) };
    true
}

pub fn write_command(byte: u8) -> bool {
    if !wait_for_write() {
        return false;
    }
    unsafe { outb(COMMAND_PORT, byte) };
    true
}
