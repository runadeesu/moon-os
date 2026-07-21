//! PS/2 keyboard driver: decodes scancode set 1 (the set every PS/2
//! controller emulates, including QEMU's) into ASCII and echoes typed
//! characters. No shift/caps-lock state yet -- just enough to prove input
//! is flowing end to end; a real input-event queue comes with the GUI work.

use super::ps2;

const DATA_PORT_RELEASE_BIT: u8 = 0x80;

const SCANCODE_ASCII: [u8; 128] = {
    let mut table = [0u8; 128];
    table[0x02] = b'1';
    table[0x03] = b'2';
    table[0x04] = b'3';
    table[0x05] = b'4';
    table[0x06] = b'5';
    table[0x07] = b'6';
    table[0x08] = b'7';
    table[0x09] = b'8';
    table[0x0A] = b'9';
    table[0x0B] = b'0';
    table[0x0C] = b'-';
    table[0x0D] = b'=';
    table[0x0E] = 0x08; // backspace
    table[0x0F] = b'\t';
    table[0x10] = b'q';
    table[0x11] = b'w';
    table[0x12] = b'e';
    table[0x13] = b'r';
    table[0x14] = b't';
    table[0x15] = b'y';
    table[0x16] = b'u';
    table[0x17] = b'i';
    table[0x18] = b'o';
    table[0x19] = b'p';
    table[0x1A] = b'[';
    table[0x1B] = b']';
    table[0x1C] = b'\n';
    table[0x1E] = b'a';
    table[0x1F] = b's';
    table[0x20] = b'd';
    table[0x21] = b'f';
    table[0x22] = b'g';
    table[0x23] = b'h';
    table[0x24] = b'j';
    table[0x25] = b'k';
    table[0x26] = b'l';
    table[0x27] = b';';
    table[0x28] = b'\'';
    table[0x29] = b'`';
    table[0x2B] = b'\\';
    table[0x2C] = b'z';
    table[0x2D] = b'x';
    table[0x2E] = b'c';
    table[0x2F] = b'v';
    table[0x30] = b'b';
    table[0x31] = b'n';
    table[0x32] = b'm';
    table[0x33] = b',';
    table[0x34] = b'.';
    table[0x35] = b'/';
    table[0x39] = b' ';
    table
};

/// Called from the IRQ1 handler with the controller's output buffer known
/// to hold a byte.
pub fn handle_irq() {
    let Some(scancode) = ps2::read_data() else {
        return;
    };

    if scancode & DATA_PORT_RELEASE_BIT != 0 {
        return; // key release, no state to track yet
    }

    let ascii = SCANCODE_ASCII.get(scancode as usize).copied().unwrap_or(0);
    if ascii != 0 {
        crate::serial_print!("{}", ascii as char);
        crate::fb_print!("{}", ascii as char);
    }
}
