//! CMOS Real-Time Clock (Motorola MC146818-compatible), read-only.
//!
//! Ports 0x70 (index)/0x71 (data), the same legacy interface every PC since
//! the original IBM AT has had. No periodic interrupt (IRQ8) setup here --
//! the GUI just polls a fresh reading each redraw, which is cheap and avoids
//! juggling another IRQ source and its own PIC unmasking.

use crate::arch::x86_64::port::{inb, outb};

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

#[derive(Clone, Copy, Debug)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

fn read_reg(reg: u8) -> u8 {
    unsafe {
        outb(CMOS_ADDR, reg);
        inb(CMOS_DATA)
    }
}

fn update_in_progress() -> bool {
    read_reg(REG_STATUS_A) & 0x80 != 0
}

fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

#[derive(PartialEq)]
struct RawFields {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
}

fn read_raw_once() -> RawFields {
    RawFields {
        second: read_reg(REG_SECONDS),
        minute: read_reg(REG_MINUTES),
        hour: read_reg(REG_HOURS),
        day: read_reg(REG_DAY),
        month: read_reg(REG_MONTH),
        year: read_reg(REG_YEAR),
    }
}

/// Reads the current date/time, guarding against the classic RTC race: a
/// read that lands mid-tick can see a torn update, so this spins until two
/// consecutive (outside the "update in progress" window) reads agree.
pub fn read() -> DateTime {
    while update_in_progress() {}
    let mut fields = read_raw_once();
    loop {
        while update_in_progress() {}
        let next = read_raw_once();
        if next == fields {
            break;
        }
        fields = next;
    }

    let status_b = read_reg(REG_STATUS_B);
    let is_bcd = status_b & 0x04 == 0;
    let is_12h = status_b & 0x02 == 0;

    let mut hour = fields.hour;
    let pm = is_12h && (hour & 0x80) != 0;
    hour &= 0x7F;

    let (second, minute, mut hour, day, month, year) = if is_bcd {
        (
            bcd_to_bin(fields.second),
            bcd_to_bin(fields.minute),
            bcd_to_bin(hour),
            bcd_to_bin(fields.day),
            bcd_to_bin(fields.month),
            bcd_to_bin(fields.year),
        )
    } else {
        (
            fields.second,
            fields.minute,
            hour,
            fields.day,
            fields.month,
            fields.year,
        )
    };

    if pm && hour < 12 {
        hour += 12;
    }

    DateTime {
        year: 2000 + u16::from(year),
        month,
        day,
        hour,
        minute,
        second,
    }
}
