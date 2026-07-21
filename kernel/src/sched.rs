//! A minimal preemptive round-robin scheduler for kernel-mode tasks.
//!
//! There's no address-space switch here (every task shares the kernel's page
//! tables) and no `yield` -- the timer IRQ is the only thing that ever
//! switches tasks. The trick that makes this simple: a suspended task's
//! entire CPU state is just a [`TrapFrame`] sitting on top of its own kernel
//! stack, identical in shape to what [`super::arch::x86_64::idt`]'s common
//! interrupt stub builds for a real interrupt. Switching tasks is nothing
//! more than telling that stub to `iretq` from a different stack than the
//! one it was called on -- indistinguishable, from the CPU's perspective,
//! from returning from an ordinary interrupt.

use crate::arch::x86_64::gdt::{KERNEL_CODE_SELECTOR, KERNEL_DATA_SELECTOR};
use crate::arch::x86_64::idt::TrapFrame;
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
    /// Keeps the stack allocation alive. Empty for the wrapped idle task,
    /// whose stack is whatever the kernel booted on.
    _stack: Vec<u8>,
    /// Address of this task's saved [`TrapFrame`], i.e. where it should
    /// resume from. Updated every time it's preempted.
    sp: u64,
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
        });
        sched.idle_wrapped = true;
    } else if let Some(running) = sched.tasks.front_mut() {
        running.sp = current_frame as u64;
    }

    if let Some(running) = sched.tasks.pop_front() {
        sched.tasks.push_back(running);
    }

    sched
        .tasks
        .front()
        .map(|t| t.sp as *mut TrapFrame)
        .unwrap_or(current_frame)
}
