//! Glue between a binary loader (ELF or PE), a fresh address space, and the
//! scheduler: the handful of steps every new ring-3 process needs (map the
//! binary in, give it a stack, hand it to `sched::spawn_user`), factored
//! out of `main.rs`'s original one-off `spawn_init_process` so `pkg::run`,
//! the File Manager/Moon Store, and the PE/Win32 test path can all launch
//! processes the same way.

use crate::memory::paging::{self, AddressSpace};

/// Virtual address (in the new process's own address space -- every
/// process gets the same one, since each has its own page tables) of the
/// top of its user-mode stack.
const USER_STACK_TOP: u64 = 0x0000_0000_7000_0000;
const USER_STACK_PAGES: u64 = 4;

/// Maps a user stack into `space` and spawns `entry` as a new ring-3 task.
/// Shared tail end of both `spawn_from_elf` and `spawn_from_pe` once the
/// binary itself is loaded and its entry point known.
fn finish_spawn(space: AddressSpace, entry: u64) -> Result<u64, &'static str> {
    let stack_base = USER_STACK_TOP - USER_STACK_PAGES * 4096;
    for i in 0..USER_STACK_PAGES {
        let frame = crate::memory::pmm::alloc_frame().ok_or("out of memory for user stack")?;
        space.map(
            stack_base + i * 4096,
            frame,
            paging::FLAG_PRESENT | paging::FLAG_WRITABLE | paging::FLAG_USER,
        );
    }

    crate::sched::spawn_user(entry, USER_STACK_TOP, space);
    Ok(entry)
}

/// Loads `elf_bytes` (a static ET_EXEC ELF64 binary) into a fresh
/// [`AddressSpace`] and spawns it as a new ring-3 task. Returns the entry
/// point on success, mostly useful for logging.
pub fn spawn_from_elf(elf_bytes: &[u8]) -> Result<u64, &'static str> {
    let space = AddressSpace::new();
    let loaded = crate::elf::load(&space, elf_bytes)?;
    finish_spawn(space, loaded.entry)
}

/// Loads `pe_bytes` (a PE32+ Windows executable, see `kernel/src/pe.rs`)
/// into a fresh [`AddressSpace`] and spawns it as a new ring-3 task.
/// Returns the entry point on success, mostly useful for logging.
pub fn spawn_from_pe(pe_bytes: &[u8]) -> Result<u64, &'static str> {
    let space = AddressSpace::new();
    let loaded = crate::pe::load(&space, pe_bytes)?;
    finish_spawn(space, loaded.entry)
}
