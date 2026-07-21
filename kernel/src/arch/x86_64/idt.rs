//! Interrupt Descriptor Table and CPU exception handling.
//!
//! Rust's `x86-interrupt` calling convention is nightly-only, so each vector
//! gets a small `#[naked]` trampoline (stable since Rust 1.88) that pushes a
//! normalized frame and jumps to one shared handler, which saves the
//! general-purpose registers and calls into safe Rust.

use super::gdt::KERNEL_CODE_SELECTOR;
use core::arch::naked_asm;
use core::mem::size_of;

#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn set(&mut self, handler: usize, ist: u8) {
        self.offset_low = (handler & 0xFFFF) as u16;
        self.selector = KERNEL_CODE_SELECTOR;
        self.ist = ist;
        self.type_attr = 0x8E; // present, ring 0, 64-bit interrupt gate
        self.offset_mid = ((handler >> 16) & 0xFFFF) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.reserved = 0;
    }
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::MISSING; 256];

/// Register saved by the common stub, in the order it pushes them, followed
/// by the normalized (vector, error_code) pair and the frame the CPU itself
/// pushes on exception entry (no privilege change, so no user rsp/ss here).
#[repr(C)]
pub struct TrapFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
}

const EXCEPTION_NAMES: [&str; 32] = [
    "Divide Error",
    "Debug",
    "Non-Maskable Interrupt",
    "Breakpoint",
    "Overflow",
    "BOUND Range Exceeded",
    "Invalid Opcode",
    "Device Not Available",
    "Double Fault",
    "Coprocessor Segment Overrun",
    "Invalid TSS",
    "Segment Not Present",
    "Stack-Segment Fault",
    "General Protection Fault",
    "Page Fault",
    "Reserved",
    "x87 Floating-Point Exception",
    "Alignment Check",
    "Machine Check",
    "SIMD Floating-Point Exception",
    "Virtualization Exception",
    "Control Protection Exception",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Reserved",
    "Hypervisor Injection Exception",
    "VMM Communication Exception",
    "Security Exception",
    "Reserved",
];

extern "C" fn exception_dispatch(frame: *mut TrapFrame) {
    let frame = unsafe { &*frame };
    let vector = frame.vector as usize;
    let name = EXCEPTION_NAMES.get(vector).copied().unwrap_or("Unknown");

    if vector == 3 {
        // Breakpoint: report and resume, don't halt the machine.
        crate::serial_println!("[int3] breakpoint at rip={:#x}", { frame.rip });
        return;
    }

    let cr2 = if vector == 14 { read_cr2() } else { 0 };

    crate::serial_println!(
        "\n!! CPU EXCEPTION: {} (vector {}) !!\n  error_code={:#x} rip={:#x} cs={:#x} rflags={:#x}",
        name,
        vector,
        { frame.error_code },
        { frame.rip },
        { frame.cs },
        { frame.rflags },
    );
    if vector == 14 {
        crate::serial_println!("  faulting address (cr2)={:#x}", cr2);
    }
    crate::serial_println!("  system halted.");

    loop {
        unsafe { core::arch::asm!("cli", "hlt") };
    }
}

fn read_cr2() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

#[unsafe(naked)]
extern "C" fn common_stub() {
    naked_asm!(
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rbp",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "add rsp, 16", // drop vector + error_code
        "iretq",
        dispatch = sym exception_dispatch,
    )
}

macro_rules! exception_stub {
    ($name:ident, $vector:literal, false) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            naked_asm!(
                "push 0",
                "push {v}",
                "jmp {common}",
                v = const $vector,
                common = sym common_stub,
            )
        }
    };
    ($name:ident, $vector:literal, true) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            naked_asm!(
                "push {v}",
                "jmp {common}",
                v = const $vector,
                common = sym common_stub,
            )
        }
    };
}

exception_stub!(stub_00, 0, false);
exception_stub!(stub_01, 1, false);
exception_stub!(stub_02, 2, false);
exception_stub!(stub_03, 3, false);
exception_stub!(stub_04, 4, false);
exception_stub!(stub_05, 5, false);
exception_stub!(stub_06, 6, false);
exception_stub!(stub_07, 7, false);
exception_stub!(stub_08, 8, true);
exception_stub!(stub_09, 9, false);
exception_stub!(stub_10, 10, true);
exception_stub!(stub_11, 11, true);
exception_stub!(stub_12, 12, true);
exception_stub!(stub_13, 13, true);
exception_stub!(stub_14, 14, true);
exception_stub!(stub_15, 15, false);
exception_stub!(stub_16, 16, false);
exception_stub!(stub_17, 17, true);
exception_stub!(stub_18, 18, false);
exception_stub!(stub_19, 19, false);
exception_stub!(stub_20, 20, false);
exception_stub!(stub_21, 21, true);
exception_stub!(stub_22, 22, false);
exception_stub!(stub_23, 23, false);
exception_stub!(stub_24, 24, false);
exception_stub!(stub_25, 25, false);
exception_stub!(stub_26, 26, false);
exception_stub!(stub_27, 27, false);
exception_stub!(stub_28, 28, false);
exception_stub!(stub_29, 29, true);
exception_stub!(stub_30, 30, true);
exception_stub!(stub_31, 31, false);

const DOUBLE_FAULT_VECTOR: usize = 8;

pub fn init() {
    let stubs: [extern "C" fn(); 32] = [
        stub_00, stub_01, stub_02, stub_03, stub_04, stub_05, stub_06, stub_07, stub_08, stub_09,
        stub_10, stub_11, stub_12, stub_13, stub_14, stub_15, stub_16, stub_17, stub_18, stub_19,
        stub_20, stub_21, stub_22, stub_23, stub_24, stub_25, stub_26, stub_27, stub_28, stub_29,
        stub_30, stub_31,
    ];

    unsafe {
        for (vector, stub) in stubs.iter().enumerate() {
            let ist = if vector == DOUBLE_FAULT_VECTOR {
                super::gdt::double_fault_ist_index() as u8 + 1
            } else {
                0
            };
            IDT[vector].set(*stub as usize, ist);
        }

        let pointer = DescriptorTablePointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}
