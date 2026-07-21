pub mod gdt;
pub mod idt;
pub mod pic;
pub mod pit;
pub mod port;
pub mod serial;

/// Brings up the CPU: serial console, GDT/TSS, IDT, and the (still masked)
/// PIC. Must run before any code that can fault, since until `idt::init()`
/// runs, any exception triple-faults. Interrupts stay disabled until the
/// caller explicitly calls `enable_interrupts`, once the scheduler has
/// something to run.
pub fn init() {
    serial::init();
    gdt::init();
    idt::init();
    pic::init();
    pic::mask_all();
}

pub fn enable_interrupts() {
    unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
}

pub fn disable_interrupts() {
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}
