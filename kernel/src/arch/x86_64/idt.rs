//! Interrupt Descriptor Table: CPU exceptions (vectors 0-31) and the legacy
//! PIC's hardware IRQs (remapped to vectors 32-47).
//!
//! Rust's `x86-interrupt` calling convention is nightly-only, so each vector
//! gets a small `#[naked]` trampoline (stable since Rust 1.88) that pushes a
//! normalized frame and jumps to one shared handler, which saves the
//! general-purpose registers and calls into safe Rust. That handler returns
//! the stack pointer to resume from, which is ordinarily the same frame it
//! was given -- except on a timer tick, where the scheduler may hand back a
//! *different* task's saved frame, which is how context switches happen.

use super::gdt::KERNEL_CODE_SELECTOR;
use super::pic;
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
        self.set_with_dpl(handler, ist, 0);
    }

    /// Like `set`, but with an explicit gate DPL. The syscall vector needs
    /// DPL=3 -- otherwise `int 0x80` from ring 3 takes a #GP instead of
    /// entering the gate, since the CPU checks the *gate's* DPL against CPL
    /// for a software `int`, unlike hardware interrupts which ignore it.
    fn set_with_dpl(&mut self, handler: usize, ist: u8, dpl: u8) {
        self.offset_low = (handler & 0xFFFF) as u16;
        self.selector = KERNEL_CODE_SELECTOR;
        self.ist = ist;
        self.type_attr = 0x8E | (dpl << 5); // present, 64-bit interrupt gate
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

const VECTOR_COUNT: usize = 48;

static mut IDT: [IdtEntry; 256] = [IdtEntry::MISSING; 256];

/// Registers saved by the common stub, in the order it pushes them, followed
/// by the normalized (vector, error_code) pair and the frame the CPU itself
/// pushes on exception entry.
///
/// Even returning to the same privilege level, x86-64's `iretq` always
/// consumes all five trailing fields here (RIP, CS, RFLAGS, RSP, SS) -- unlike
/// legacy 32-bit `iret`, it does not conditionally skip RSP/SS. A hand-built
/// frame that only fills in the first three gets whatever garbage follows
/// misread as the new stack pointer and segment, which faults as a bogus
/// #GP the moment `iretq` runs.
///
/// A suspended task's entire state is one of these sitting at the top of its
/// own kernel stack -- resuming it is just pointing `rsp` back at it and
/// running the second half of the common stub, indistinguishable from
/// returning from a real interrupt.
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
    pub rsp: u64,
    pub ss: u64,
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

extern "C" fn interrupt_dispatch(frame: *mut TrapFrame) -> *mut TrapFrame {
    let vector = unsafe { (*frame).vector };

    if vector < 32 {
        handle_exception(frame);
        return frame;
    }

    if vector == SYSCALL_VECTOR as u64 {
        return crate::syscall::handle(frame);
    }

    let irq = (vector - u64::from(pic::IRQ_BASE)) as u8;
    let next = match irq {
        0 => {
            let next = crate::sched::on_timer_tick(frame);
            crate::gui::on_tick(crate::sched::ticks());
            next
        }
        1 => {
            crate::drivers::keyboard::handle_irq();
            frame
        }
        12 => {
            crate::drivers::mouse::handle_irq();
            frame
        }
        _ => frame,
    };
    pic::send_eoi(irq);
    next
}

fn handle_exception(frame: *mut TrapFrame) {
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
        "mov rsp, rax", // dispatch returns the frame to resume (maybe a new task)
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
        dispatch = sym interrupt_dispatch,
    )
}

macro_rules! interrupt_stub {
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

interrupt_stub!(stub_00, 0, false);
interrupt_stub!(stub_01, 1, false);
interrupt_stub!(stub_02, 2, false);
interrupt_stub!(stub_03, 3, false);
interrupt_stub!(stub_04, 4, false);
interrupt_stub!(stub_05, 5, false);
interrupt_stub!(stub_06, 6, false);
interrupt_stub!(stub_07, 7, false);
interrupt_stub!(stub_08, 8, true);
interrupt_stub!(stub_09, 9, false);
interrupt_stub!(stub_10, 10, true);
interrupt_stub!(stub_11, 11, true);
interrupt_stub!(stub_12, 12, true);
interrupt_stub!(stub_13, 13, true);
interrupt_stub!(stub_14, 14, true);
interrupt_stub!(stub_15, 15, false);
interrupt_stub!(stub_16, 16, false);
interrupt_stub!(stub_17, 17, true);
interrupt_stub!(stub_18, 18, false);
interrupt_stub!(stub_19, 19, false);
interrupt_stub!(stub_20, 20, false);
interrupt_stub!(stub_21, 21, true);
interrupt_stub!(stub_22, 22, false);
interrupt_stub!(stub_23, 23, false);
interrupt_stub!(stub_24, 24, false);
interrupt_stub!(stub_25, 25, false);
interrupt_stub!(stub_26, 26, false);
interrupt_stub!(stub_27, 27, false);
interrupt_stub!(stub_28, 28, false);
interrupt_stub!(stub_29, 29, true);
interrupt_stub!(stub_30, 30, true);
interrupt_stub!(stub_31, 31, false);
// IRQ0-15 (remapped to vectors 32-47), no CPU-pushed error code on any of them.
interrupt_stub!(stub_32, 32, false);
interrupt_stub!(stub_33, 33, false);
interrupt_stub!(stub_34, 34, false);
interrupt_stub!(stub_35, 35, false);
interrupt_stub!(stub_36, 36, false);
interrupt_stub!(stub_37, 37, false);
interrupt_stub!(stub_38, 38, false);
interrupt_stub!(stub_39, 39, false);
interrupt_stub!(stub_40, 40, false);
interrupt_stub!(stub_41, 41, false);
interrupt_stub!(stub_42, 42, false);
interrupt_stub!(stub_43, 43, false);
interrupt_stub!(stub_44, 44, false);
interrupt_stub!(stub_45, 45, false);
interrupt_stub!(stub_46, 46, false);
interrupt_stub!(stub_47, 47, false);

/// `int 0x80`: the syscall gate. A software interrupt rather than
/// `SYSCALL`/`SYSRET` -- simpler to wire into the same common-stub/TrapFrame
/// machinery everything else already uses, at the cost of the couple-dozen
/// extra cycles a full interrupt gate costs over the dedicated instruction.
pub const SYSCALL_VECTOR: usize = 0x80;
interrupt_stub!(stub_syscall, 0x80, false);

const DOUBLE_FAULT_VECTOR: usize = 8;

pub fn init() {
    let stubs: [extern "C" fn(); VECTOR_COUNT] = [
        stub_00, stub_01, stub_02, stub_03, stub_04, stub_05, stub_06, stub_07, stub_08, stub_09,
        stub_10, stub_11, stub_12, stub_13, stub_14, stub_15, stub_16, stub_17, stub_18, stub_19,
        stub_20, stub_21, stub_22, stub_23, stub_24, stub_25, stub_26, stub_27, stub_28, stub_29,
        stub_30, stub_31, stub_32, stub_33, stub_34, stub_35, stub_36, stub_37, stub_38, stub_39,
        stub_40, stub_41, stub_42, stub_43, stub_44, stub_45, stub_46, stub_47,
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

        IDT[SYSCALL_VECTOR].set_with_dpl(stub_syscall as *const () as usize, 0, 3);

        let pointer = DescriptorTablePointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}
