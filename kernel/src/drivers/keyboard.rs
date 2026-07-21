//! PS/2 keyboard driver: decodes scancode set 1 (the set every PS/2
//! controller emulates, including QEMU's) into ASCII and echoes typed
//! characters. No shift/caps-lock state yet -- just enough to prove input
//! is flowing end to end; a real input-event queue comes with the GUI work.

use super::ps2;
use core::sync::atomic::{AtomicBool, Ordering};

const DATA_PORT_RELEASE_BIT: u8 = 0x80;

const SCANCODE_ALT: u8 = 0x38;
const SCANCODE_LSHIFT: u8 = 0x2A;
const SCANCODE_RSHIFT: u8 = 0x36;
const SCANCODE_TAB: u8 = 0x0F;
const SCANCODE_UP: u8 = 0x48;
const SCANCODE_DOWN: u8 = 0x50;
const SCANCODE_LEFT: u8 = 0x4B;
const SCANCODE_RIGHT: u8 = 0x4D;
const EXTENDED_PREFIX: u8 = 0xE0;

static ALT_DOWN: AtomicBool = AtomicBool::new(false);
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static PENDING_EXTENDED: AtomicBool = AtomicBool::new(false);

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

/// Same layout as `SCANCODE_ASCII`, shifted: uppercase letters and the
/// shifted symbol on each punctuation key (US QWERTY). Needed for anything
/// beyond lowercase-and-symbols input -- capitalized text, `_`/`"`/`>` etc.,
/// which every text field in the GUI (terminal, search boxes, file rename)
/// otherwise has no way to produce.
const SCANCODE_ASCII_SHIFTED: [u8; 128] = {
    let mut table = [0u8; 128];
    table[0x02] = b'!';
    table[0x03] = b'@';
    table[0x04] = b'#';
    table[0x05] = b'$';
    table[0x06] = b'%';
    table[0x07] = b'^';
    table[0x08] = b'&';
    table[0x09] = b'*';
    table[0x0A] = b'(';
    table[0x0B] = b')';
    table[0x0C] = b'_';
    table[0x0D] = b'+';
    table[0x0E] = 0x08; // backspace
    table[0x0F] = b'\t';
    table[0x10] = b'Q';
    table[0x11] = b'W';
    table[0x12] = b'E';
    table[0x13] = b'R';
    table[0x14] = b'T';
    table[0x15] = b'Y';
    table[0x16] = b'U';
    table[0x17] = b'I';
    table[0x18] = b'O';
    table[0x19] = b'P';
    table[0x1A] = b'{';
    table[0x1B] = b'}';
    table[0x1C] = b'\n';
    table[0x1E] = b'A';
    table[0x1F] = b'S';
    table[0x20] = b'D';
    table[0x21] = b'F';
    table[0x22] = b'G';
    table[0x23] = b'H';
    table[0x24] = b'J';
    table[0x25] = b'K';
    table[0x26] = b'L';
    table[0x27] = b':';
    table[0x28] = b'"';
    table[0x29] = b'~';
    table[0x2B] = b'|';
    table[0x2C] = b'Z';
    table[0x2D] = b'X';
    table[0x2E] = b'C';
    table[0x2F] = b'V';
    table[0x30] = b'B';
    table[0x31] = b'N';
    table[0x32] = b'M';
    table[0x33] = b'<';
    table[0x34] = b'>';
    table[0x35] = b'?';
    table[0x39] = b' ';
    table
};

/// Called from the IRQ1 handler with the controller's output buffer known
/// to hold a byte.
pub fn handle_irq() {
    let Some(scancode) = ps2::read_data() else {
        return;
    };

    if scancode == EXTENDED_PREFIX {
        PENDING_EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    let extended = PENDING_EXTENDED.swap(false, Ordering::Relaxed);

    let released = scancode & DATA_PORT_RELEASE_BIT != 0;
    let code = scancode & !DATA_PORT_RELEASE_BIT;

    if code == SCANCODE_ALT {
        ALT_DOWN.store(!released, Ordering::Relaxed);
        return;
    }

    if code == SCANCODE_LSHIFT || code == SCANCODE_RSHIFT {
        SHIFT_DOWN.store(!released, Ordering::Relaxed);
        return;
    }

    if released {
        return; // key release, no other state to track yet
    }

    if extended {
        let key = match code {
            SCANCODE_UP => Some(crate::gui::SpecialKey::Up),
            SCANCODE_DOWN => Some(crate::gui::SpecialKey::Down),
            SCANCODE_LEFT => Some(crate::gui::SpecialKey::Left),
            SCANCODE_RIGHT => Some(crate::gui::SpecialKey::Right),
            _ => None,
        };
        if let Some(key) = key {
            crate::gui::on_special_key(key);
        }
        return;
    }

    if code == SCANCODE_TAB && ALT_DOWN.load(Ordering::Relaxed) {
        crate::gui::on_special_key(crate::gui::SpecialKey::AltTab);
        return;
    }

    let table = if SHIFT_DOWN.load(Ordering::Relaxed) {
        &SCANCODE_ASCII_SHIFTED
    } else {
        &SCANCODE_ASCII
    };
    let ascii = table.get(code as usize).copied().unwrap_or(0);
    if ascii != 0 {
        crate::serial_print!("{}", ascii as char);
        crate::gui::on_key(ascii);
    }
}
