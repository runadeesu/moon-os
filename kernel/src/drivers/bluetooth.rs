//! Bluetooth adapter presence check: a real PCI scan for a device in class
//! 0x0D ("Wireless Controller"), subclass 0x11 ("Bluetooth Controller") --
//! not a fake "connected" indicator. moon OS has no Bluetooth protocol stack
//! (HCI/L2CAP/pairing) at all, so even if an adapter were found here there
//! would be nothing further to do with it yet; this only answers the
//! honest, narrower question the taskbar/Settings need: is there real
//! Bluetooth hardware on this machine at all. On stock QEMU there never is,
//! so the taskbar icon stays honestly greyed out rather than showing a
//! connected state with nothing behind it.

use super::pci;

const CLASS_WIRELESS: u8 = 0x0D;
const SUBCLASS_BLUETOOTH: u8 = 0x11;

pub fn adapter_present() -> bool {
    pci::enumerate()
        .into_iter()
        .any(|d| d.class == CLASS_WIRELESS && d.subclass == SUBCLASS_BLUETOOTH)
}
