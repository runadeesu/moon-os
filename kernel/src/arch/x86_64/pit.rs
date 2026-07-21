//! Programmable Interval Timer (channel 0), used to drive the scheduler tick.

use super::port::outb;

const CHANNEL0: u16 = 0x40;
const COMMAND: u16 = 0x43;
const BASE_FREQUENCY: u32 = 1_193_182;

/// Programs channel 0 for a periodic (mode 3, square wave) interrupt on IRQ0
/// at approximately `frequency_hz`.
pub fn init(frequency_hz: u32) {
    let divisor = (BASE_FREQUENCY / frequency_hz).clamp(1, u16::MAX as u32) as u16;
    unsafe {
        outb(COMMAND, 0x36); // channel 0, lobyte/hibyte, mode 3, binary
        outb(CHANNEL0, (divisor & 0xFF) as u8);
        outb(CHANNEL0, (divisor >> 8) as u8);
    }
}
