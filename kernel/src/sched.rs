//! A minimal preemptive round-robin scheduler for kernel- and user-mode
//! tasks.
//!
//! The trick that makes this simple: a suspended task's entire CPU state is
//! just a [`TrapFrame`] sitting on top of its own kernel stack, identical in
//! shape to what [`super::arch::x86_64::idt`]'s common interrupt stub builds
//! for a real interrupt. Switching tasks is nothing more than telling that
//! stub to `iretq` from a different stack than the one it was called on --
//! indistinguishable, from the CPU's perspective, from returning from an
//! ordinary interrupt.
//!
//! Usermode tasks (see [`spawn_user`]) add two wrinkles kernel tasks don't
//! have: their own [`AddressSpace`](crate::memory::paging::AddressSpace),
//! switched to in CR3 on every context switch, and a *private* kernel
//! stack the CPU switches to automatically via TSS.RSP0 whenever a trap
//! interrupts them in ring 3 (kernel tasks never take a privilege change on
//! trap, so RSP0 is simply irrelevant while one of those is running).

use crate::arch::x86_64::gdt::{self, KERNEL_CODE_SELECTOR, KERNEL_DATA_SELECTOR};
use crate::arch::x86_64::idt::TrapFrame;
use crate::memory::paging::{self, AddressSpace};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

const STACK_SIZE: usize = 32 * 1024;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// Number of timer ticks (currently 100/sec) since the scheduler started.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

struct Task {
    id: u64,
    /// Keeps the kernel-side stack allocation alive. Empty for the wrapped
    /// idle task, whose stack is whatever the kernel booted on.
    _stack: Vec<u8>,
    /// Address of this task's saved [`TrapFrame`], i.e. where it should
    /// resume from. Updated every time it's preempted.
    sp: u64,
    /// Top of this task's private kernel stack -- becomes TSS.RSP0 while
    /// it's running. Unused (and unconsulted) for kernel-mode tasks.
    kernel_stack_top: u64,
    /// Physical address of this task's PML4.
    cr3: u64,
    /// Keeps the process's page tables (and the frames they map) alive for
    /// as long as the task exists. `None` for kernel tasks, which share the
    /// boot address space instead of owning one.
    _address_space: Option<AddressSpace>,
}

struct Scheduler {
    tasks: VecDeque<Task>,
    next_id: u64,
    idle_wrapped: bool,
}

impl Scheduler {
    const fn new() -> Self {
        Self {
            tasks: VecDeque::new(),
            next_id: 1,
            idle_wrapped: false,
        }
    }
}

static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler::new());
static CURRENT_CR3: AtomicU64 = AtomicU64::new(0);

/// Spawns a new kernel task with its own stack. `entry` must never return.
pub fn spawn(entry: extern "C" fn() -> !) {
    let mut stack = alloc::vec![0u8; STACK_SIZE];
    let stack_top = stack.as_mut_ptr() as u64 + STACK_SIZE as u64;
    let frame_addr = (stack_top - size_of::<TrapFrame>() as u64) & !0xF;

    // SAFETY: frame_addr points inside the stack we just allocated and sized
    // for exactly one TrapFrame plus alignment slack.
    unsafe {
        let frame = frame_addr as *mut TrapFrame;
        core::ptr::write_bytes(frame as *mut u8, 0, size_of::<TrapFrame>());
        (*frame).rip = entry as usize as u64;
        (*frame).cs = u64::from(KERNEL_CODE_SELECTOR);
        (*frame).rflags = 0x202; // reserved bit 1 + IF, so the task runs with interrupts on
                                 // iretq always pops rsp/ss too, even for a same-privilege return; once
                                 // the frame itself is consumed, let the task have the whole stack.
        (*frame).rsp = frame_addr;
        (*frame).ss = u64::from(KERNEL_DATA_SELECTOR);
    }

    let mut sched = SCHED.lock();
    let id = sched.next_id;
    sched.next_id += 1;
    sched.tasks.push_back(Task {
        id,
        _stack: stack,
        sp: frame_addr,
        kernel_stack_top: stack_top,
        cr3: paging::current_cr3(),
        _address_space: None,
    });
}

/// Spawns a new ring-3 task: `entry_va` and `user_stack_top` are virtual
/// addresses already mapped (executable, and writable+present respectively)
/// in `address_space`, typically by [`crate::elf::load`]. Ownership of
/// `address_space` moves to the scheduler, which keeps it alive for as long
/// as the task exists.
pub fn spawn_user(entry_va: u64, user_stack_top: u64, address_space: AddressSpace) {
    let mut kernel_stack = alloc::vec![0u8; STACK_SIZE];
    let kernel_stack_top = kernel_stack.as_mut_ptr() as u64 + STACK_SIZE as u64;
    let frame_addr = (kernel_stack_top - size_of::<TrapFrame>() as u64) & !0xF;

    // SAFETY: frame_addr points inside the kernel stack we just allocated
    // and sized for exactly one TrapFrame plus alignment slack.
    unsafe {
        let frame = frame_addr as *mut TrapFrame;
        core::ptr::write_bytes(frame as *mut u8, 0, size_of::<TrapFrame>());
        (*frame).rip = entry_va;
        (*frame).cs = u64::from(gdt::USER_CODE_SELECTOR);
        (*frame).rflags = 0x202;
        (*frame).rsp = user_stack_top;
        (*frame).ss = u64::from(gdt::USER_DATA_SELECTOR);
    }

    let mut sched = SCHED.lock();
    let id = sched.next_id;
    sched.next_id += 1;
    sched.tasks.push_back(Task {
        id,
        _stack: kernel_stack,
        sp: frame_addr,
        kernel_stack_top,
        cr3: address_space.cr3(),
        _address_space: Some(address_space),
    });
}

pub fn task_count() -> usize {
    SCHED.lock().tasks.len()
}

/// Called from the timer IRQ handler. Returns the [`TrapFrame`] to resume
/// into: either the same one that was interrupted (nothing to switch to
/// yet), or the next task's in round-robin order.
pub fn on_timer_tick(current_frame: *mut TrapFrame) -> *mut TrapFrame {
    TICKS.fetch_add(1, Ordering::Relaxed);
    let mut sched = SCHED.lock();

    if sched.tasks.is_empty() {
        return current_frame;
    }

    if !sched.idle_wrapped {
        // First tick ever: whatever was running (kmain's halt loop) becomes
        // the idle task. It goes to the *front*, because it's the task this
        // very interrupt caught running -- the normal rotation below then
        // correctly saves its frame and moves it to the back.
        sched.tasks.push_front(Task {
            id: 0,
            _stack: Vec::new(),
            sp: current_frame as u64,
            kernel_stack_top: 0,
            cr3: paging::current_cr3(),
            _address_space: None,
        });
        sched.idle_wrapped = true;
    } else if let Some(running) = sched.tasks.front_mut() {
        running.sp = current_frame as u64;
    }

    if let Some(running) = sched.tasks.pop_front() {
        sched.tasks.push_back(running);
    }

    let Some(next) = sched.tasks.front() else {
        return current_frame;
    };

    if CURRENT_CR3.swap(next.cr3, Ordering::Relaxed) != next.cr3 {
        paging::switch_to(next.cr3);
    }
    gdt::set_kernel_stack(next.kernel_stack_top);

    next.sp as *mut TrapFrame
}

/// Called from a `SYS_EXIT` syscall: drops the current (front-of-queue)
/// task -- freeing its kernel stack and address space -- and returns the
/// frame of whichever task should run next. Never returns to the caller in
/// the normal sense: the common stub `iretq`s straight into that frame.
pub fn exit_current() -> *mut TrapFrame {
    let mut sched = SCHED.lock();

    // The idle task (id 0) represents kmain's own halt loop, not a real
    // process -- there's nothing meaningful to "exit" it into, so treat the
    // call as a no-op rather than dropping the only thing left to run.
    if sched.tasks.front().map(|t| t.id) == Some(0) {
        return sched.tasks.front().map(|t| t.sp as *mut TrapFrame).unwrap();
    }

    sched.tasks.pop_front();

    let Some(next) = sched.tasks.front() else {
        // Nothing left at all (shouldn't happen -- idle is never removed);
        // fall back to whatever's still mapped rather than dereference a
        // dangling frame.
        return core::ptr::null_mut();
    };

    if CURRENT_CR3.swap(next.cr3, Ordering::Relaxed) != next.cr3 {
        paging::switch_to(next.cr3);
    }
    gdt::set_kernel_stack(next.kernel_stack_top);

    next.sp as *mut TrapFrame
}
