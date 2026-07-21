//! Legacy 8259 Programmable Interrupt Controller.
//!
//! We remap it so its 16 IRQ lines land on vectors 32-47, out of the way of
//! the CPU exceptions at 0-31. APIC support (needed for SMP later) can
//! replace this in a future milestone; the 8259 is more than enough for a
//! single timer and a keyboard.

use super::port::{inb, outb};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

const ICW1_INIT: u8 = 0x11; // edge-triggered, cascade, expect ICW4
const ICW4_8086: u8 = 0x01; // 8086/88 mode

const PIC_EOI: u8 = 0x20;

pub const IRQ_BASE: u8 = 32;

pub fn init() {
    unsafe {
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        outb(PIC1_COMMAND, ICW1_INIT);
        outb(PIC2_COMMAND, ICW1_INIT);
        outb(PIC1_DATA, IRQ_BASE); // ICW2: vector offset for PIC1
        outb(PIC2_DATA, IRQ_BASE + 8); // ICW2: vector offset for PIC2
        outb(PIC1_DATA, 0b0000_0100); // ICW3: PIC2 is on IRQ2 of PIC1
        outb(PIC2_DATA, 0b0000_0010); // ICW3: PIC2's cascade identity
        outb(PIC1_DATA, ICW4_8086);
        outb(PIC2_DATA, ICW4_8086);

        outb(PIC1_DATA, mask1);
        outb(PIC2_DATA, mask2);
    }
}

pub fn mask_all() {
    unsafe {
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
}

pub fn set_mask(irq: u8) {
    unsafe {
        let (port, bit) = if irq < 8 {
            (PIC1_DATA, irq)
        } else {
            (PIC2_DATA, irq - 8)
        };
        let value = inb(port) | (1 << bit);
        outb(port, value);
    }
}

pub fn clear_mask(irq: u8) {
    unsafe {
        let (port, bit) = if irq < 8 {
            (PIC1_DATA, irq)
        } else {
            (PIC2_DATA, irq - 8)
        };
        let value = inb(port) & !(1 << bit);
        outb(port, value);
    }
}

pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, PIC_EOI);
        }
        outb(PIC1_COMMAND, PIC_EOI);
    }
}
