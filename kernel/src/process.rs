//! Glue between the ELF loader, a fresh address space, and the scheduler:
//! the handful of steps every new ring-3 process needs (map the binary in,
//! give it a stack, hand it to `sched::spawn_user`), factored out of
//! `main.rs`'s original one-off `spawn_init_process` so `pkg::run` and the
//! File Manager/Moon Store can launch processes the same way.

use crate::memory::paging::{self, AddressSpace};

/// Virtual address (in the new process's own address space -- every
/// process gets the same one, since each has its own page tables) of the
/// top of its user-mode stack.
const USER_STACK_TOP: u64 = 0x0000_0000_7000_0000;
const USER_STACK_PAGES: u64 = 4;

/// Loads `elf_bytes` into a fresh [`AddressSpace`], maps it a user stack,
/// and spawns it as a new ring-3 task. Returns the entry point on success,
/// mostly useful for logging.
pub fn spawn_from_elf(elf_bytes: &[u8]) -> Result<u64, &'static str> {
    let space = AddressSpace::new();

    let loaded = crate::elf::load(&space, elf_bytes)?;

    let stack_base = USER_STACK_TOP - USER_STACK_PAGES * 4096;
    for i in 0..USER_STACK_PAGES {
        let frame = crate::memory::pmm::alloc_frame().ok_or("out of memory for user stack")?;
        space.map(
            stack_base + i * 4096,
            frame,
            paging::FLAG_PRESENT | paging::FLAG_WRITABLE | paging::FLAG_USER,
        );
    }

    crate::sched::spawn_user(loaded.entry, USER_STACK_TOP, space);
    Ok(loaded.entry)
}
