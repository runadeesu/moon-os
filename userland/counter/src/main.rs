//! moon OS's second userland test program: a real loop (not just one
//! syscall then exit, like `userland/init`) that prints a few ticks before
//! exiting. Exists to prove more than one *installed package* can be
//! loaded and run -- see `kernel/src/pkg.rs` and the `pkg` terminal
//! command -- not just that the one hardcoded init process works.
//!
//! Same minimal, no_std/no_main/no-libc shape as `userland/init`; see that
//! crate's comments for the syscall ABI.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

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

fn write(msg: &[u8]) {
    unsafe {
        syscall2(SYS_WRITE, msg.as_ptr() as u64, msg.len() as u64);
    }
}

const TICK_COUNT: u8 = 5;

#[no_mangle]
extern "C" fn _start() -> ! {
    // "tick N\n", built one byte at a time -- no allocator, no core::fmt
    // machinery pulled in, just enough to prove this isn't a single
    // hardcoded string like `init`'s.
    let mut line = *b"tick N\n";
    for i in 0..TICK_COUNT {
        line[5] = b'0' + i;
        write(&line);
    }
    write(b"counter app done\n");

    unsafe {
        syscall2(SYS_EXIT, 0, 0);
    }
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
