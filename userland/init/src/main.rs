//! moon OS's first userland program: proof that the ring-3/syscall/ELF
//! loader plumbing in the kernel actually works, not a real init system yet.
//! Statically linked, no libc, no allocator -- just enough to make two
//! syscalls and exit.
//!
//! The ABI here is moon OS's own (see `kernel/src/syscall.rs`), not Linux's:
//! syscall number in `rax`, up to two arguments in `rdi`/`rsi`, delivered
//! via `int 0x80` rather than the `syscall` instruction (simpler to route
//! through the same interrupt-gate machinery every other trap already
//! uses).

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

/// Issues `int 0x80` with `num` in rax and up to two arguments in rdi/rsi,
/// returning whatever the kernel wrote back into rax.
#[inline(always)]
unsafe fn syscall2(num: u64, arg0: u64, arg1: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
        );
    }
    ret
}

const MESSAGE: &[u8] = b"Hello from moon OS userland (ring 3)!\n";

#[no_mangle]
extern "C" fn _start() -> ! {
    unsafe {
        syscall2(SYS_WRITE, MESSAGE.as_ptr() as u64, MESSAGE.len() as u64);
        syscall2(SYS_EXIT, 0, 0);
    }
    // SYS_EXIT drops this task and the scheduler switches away without
    // ever returning here. If that somehow didn't happen, spin instead of
    // running off into unmapped memory -- there's no `hlt` available in
    // ring 3, it's a privileged instruction. `spin_loop` at least hints the
    // CPU to a lower-power busy-wait state (PAUSE) instead of hammering it.
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
