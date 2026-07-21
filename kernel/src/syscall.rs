//! The syscall gate (`int 0x80`, see `arch::x86_64::idt::SYSCALL_VECTOR`):
//! moon OS's own minimal ABI, not Linux-compatible. Convention: syscall
//! number in `rax`, up to two arguments in `rdi`/`rsi`, return value
//! written back into `rax`.
//!
//! Userspace and the kernel run under the *same* CR3 at the moment a
//! syscall traps in (address spaces differ per process, but this is that
//! process's own), so a pointer a task passes in is safe to dereference
//! directly -- there's no copy_from_user/copy_to_user validation yet. A
//! known gap for later hardening (a malicious or buggy pointer can still
//! fault the kernel), acceptable for now since there's exactly one
//! usermode process to trust.

use crate::arch::x86_64::idt::TrapFrame;

const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;

/// Longest single write accepted in one syscall -- just a sanity bound so a
/// garbage length argument can't make the kernel walk off into unmapped
/// memory.
const MAX_WRITE_LEN: usize = 4096;

pub fn handle(frame: *mut TrapFrame) -> *mut TrapFrame {
    let syscall_num = unsafe { (*frame).rax };
    match syscall_num {
        SYS_EXIT => {
            let code = unsafe { (*frame).rdi };
            crate::serial_println!("syscall: user task exited with code {}", code);
            let next = crate::sched::exit_current();
            if next.is_null() {
                frame
            } else {
                next
            }
        }
        SYS_WRITE => {
            let ptr = unsafe { (*frame).rdi } as *const u8;
            let len = (unsafe { (*frame).rsi } as usize).min(MAX_WRITE_LEN);
            let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
            match core::str::from_utf8(slice) {
                Ok(s) => crate::serial_println!("[user] {}", s),
                Err(_) => crate::serial_println!("[user] <{} bytes, not valid UTF-8>", len),
            }
            unsafe { (*frame).rax = len as u64 };
            frame
        }
        other => {
            crate::serial_println!("syscall: unknown syscall number {}", other);
            unsafe { (*frame).rax = u64::MAX };
            frame
        }
    }
}
