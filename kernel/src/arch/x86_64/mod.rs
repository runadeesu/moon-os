pub mod gdt;
pub mod idt;
pub mod port;
pub mod serial;

/// Brings up the CPU: serial console, GDT/TSS, IDT. Must run before any code
/// that can fault, since until `idt::init()` runs, any exception triple-faults.
pub fn init() {
    serial::init();
    gdt::init();
    idt::init();
}
