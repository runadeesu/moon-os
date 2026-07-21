//! Global Descriptor Table and Task State Segment setup.
//!
//! The TSS currently exists only to give the double-fault handler a private
//! Interrupt Stack Table (IST) entry, so a stack overflow doesn't triple
//! fault while the CPU tries to push the exception frame onto an already
//! blown stack.

use core::arch::asm;
use core::mem::size_of;

pub const KERNEL_CODE_SELECTOR: u16 = 1 << 3;
pub const KERNEL_DATA_SELECTOR: u16 = 2 << 3;
const TSS_SELECTOR: u16 = 3 << 3;

const DOUBLE_FAULT_IST_INDEX: usize = 0;
const IST_STACK_SIZE: usize = 4096 * 5;

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp: [0; 3],
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: size_of::<Tss>() as u16,
        }
    }
}

static mut DOUBLE_FAULT_STACK: [u8; IST_STACK_SIZE] = [0; IST_STACK_SIZE];
static mut TSS: Tss = Tss::new();

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

static mut GDT: [u64; 5] = [
    0x0000000000000000, // null
    0x00AF9A000000FFFF, // kernel code (64-bit, present, ring 0, exec/read)
    0x00CF92000000FFFF, // kernel data (present, ring 0, read/write)
    0,                  // TSS low half (filled in at init)
    0,                  // TSS high half (filled in at init)
];

fn tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    let low = (limit as u64 & 0xFFFF)
        | ((base & 0xFFFFFF) << 16)
        | (0x89u64 << 40) // present, DPL0, type = 64-bit TSS (available)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = (base >> 32) & 0xFFFF_FFFF;
    (low, high)
}

pub fn init() {
    unsafe {
        let stack_top = core::ptr::addr_of!(DOUBLE_FAULT_STACK) as u64 + IST_STACK_SIZE as u64;
        TSS.ist[DOUBLE_FAULT_IST_INDEX] = stack_top;

        let tss_base = core::ptr::addr_of!(TSS) as u64;
        let tss_limit = (size_of::<Tss>() - 1) as u32;
        let (low, high) = tss_descriptor(tss_base, tss_limit);
        GDT[3] = low;
        GDT[4] = high;

        let pointer = DescriptorTablePointer {
            limit: (size_of::<[u64; 5]>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        reload_segments();

        asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

pub fn double_fault_ist_index() -> u16 {
    DOUBLE_FAULT_IST_INDEX as u16
}

unsafe fn reload_segments() {
    unsafe {
        asm!(
            "push {sel}",
            "lea {tmp}, [55f + rip]",
            "push {tmp}",
            "retfq",
            "55:",
            sel = in(reg) u64::from(KERNEL_CODE_SELECTOR),
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {sel:x}",
            "mov es, {sel:x}",
            "mov fs, {sel:x}",
            "mov gs, {sel:x}",
            "mov ss, {sel:x}",
            sel = in(reg) KERNEL_DATA_SELECTOR,
            options(nostack, preserves_flags),
        );
    }
}
