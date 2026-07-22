//! Reboot/shutdown -- both real hardware operations, not stubs, but neither
//! is a general-purpose implementation: `reboot` uses the universally
//! supported (real hardware and every emulator) "pulse the keyboard
//! controller's reset line" trick, and `shutdown` uses the fixed ACPI PM1a
//! control port QEMU's `q35`/`i440fx` machines happen to expose at 0x604 --
//! that address is a QEMU-specific convenience, not something derived from
//! parsing the real ACPI FADT (which would be needed to shut down real
//! hardware). Good enough for a kernel that only targets QEMU so far.

use crate::arch::x86_64::port::{inb, outb, outw};

/// Real battery state, as far as this kernel is able to determine it --
/// which, honestly, is not very far. Reading an actual battery percentage
/// requires walking the ACPI FADT/DSDT and evaluating an AML "Control
/// Method Battery Device" (or talking to a Smart Battery over SMBus); this
/// kernel has no ACPI table parser or AML interpreter at all yet (writing
/// one is a project on the scale of the whole existing kernel, not a small
/// addition -- see ROADMAP.md). Since that's true regardless of what
/// machine this runs on, and QEMU's default `q35`/`i440fx` boards don't
/// attach a virtual battery device to begin with, the only honest answer
/// available right now is "no battery, running on AC power" -- never a
/// fabricated percentage.
pub enum BatteryStatus {
    /// No ACPI battery device is reachable (true for every machine this
    /// kernel can currently query, including QEMU).
    AcPowerNoBattery,
}

pub fn battery_status() -> BatteryStatus {
    BatteryStatus::AcPowerNoBattery
}

/// Pulses the PS/2 controller's reset line (command 0xFE) -- the same
/// technique real-mode bootloaders and most hobby OSes use, since it works
/// on every PC-compatible without needing ACPI tables parsed first.
pub fn reboot() -> ! {
    unsafe {
        // Drain the input buffer first: issuing 0xFE while the controller
        // still has a stale byte pending is a common reason this trick
        // silently fails to reset the machine.
        while inb(0x64) & 0x02 != 0 {}
        outb(0x64, 0xFE);
    }
    // If the controller didn't reset the CPU (shouldn't happen on QEMU),
    // halt rather than fall back into undefined kernel state.
    loop {
        unsafe { core::arch::asm!("cli", "hlt") };
    }
}

/// QEMU-specific ACPI shutdown: `q35`/`i440fx` both wire PM1a_CNT to port
/// 0x604 by default, and writing SLP_EN (bit 13) with SLP_TYP=0 there is a
/// widely used trick (see OSDev's "Shutdown" page) for triggering a real
/// ACPI S5 shutdown without implementing FADT/DSDT parsing. Only works
/// under QEMU -- real hardware needs the general ACPI path this skips.
pub fn shutdown() -> ! {
    unsafe {
        outw(0x604, 0x2000);
    }
    loop {
        unsafe { core::arch::asm!("cli", "hlt") };
    }
}
