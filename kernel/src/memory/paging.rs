//! Manipulation of the active x86_64 4-level page tables.
//!
//! We don't build a fresh address space in this milestone -- we extend the
//! page tables Limine already set up and switched to (CR3 still points at
//! them). Every intermediate table we allocate comes from the physical
//! memory manager and is reached through the HHDM, exactly like any other
//! physical frame.

use core::arch::asm;

pub const FLAG_PRESENT: u64 = 1 << 0;
pub const FLAG_WRITABLE: u64 = 1 << 1;
pub const FLAG_NO_EXECUTE: u64 = 1 << 63;

const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const ENTRIES_PER_TABLE: usize = 512;

fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value & ADDR_MASK
}

fn invalidate(virt: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
}

fn table_at(phys: u64) -> *mut u64 {
    crate::memory::phys_to_virt(phys) as *mut u64
}

fn page_indices(virt: u64) -> [usize; 4] {
    [
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    ]
}

/// Returns the physical address of the next-level table referenced by
/// `table[index]`, allocating and zeroing a fresh one if the entry isn't
/// present yet.
unsafe fn next_table(table: *mut u64, index: usize) -> u64 {
    let entry = unsafe { core::ptr::read(table.add(index)) };
    if entry & FLAG_PRESENT != 0 {
        entry & ADDR_MASK
    } else {
        let frame =
            crate::memory::pmm::alloc_frame().expect("out of physical memory for page tables");
        let frame_virt = table_at(frame);
        unsafe { core::ptr::write_bytes(frame_virt, 0, ENTRIES_PER_TABLE * 8) };
        unsafe { core::ptr::write(table.add(index), frame | FLAG_PRESENT | FLAG_WRITABLE) };
        frame
    }
}

/// Maps a single 4 KiB page. `virt` and `phys` must already be page-aligned.
pub fn map(virt: u64, phys: u64, flags: u64) {
    let [i4, i3, i2, i1] = page_indices(virt);
    unsafe {
        let pml4 = table_at(read_cr3());
        let pdpt = table_at(next_table(pml4, i4));
        let pd = table_at(next_table(pdpt, i3));
        let pt = table_at(next_table(pd, i2));
        core::ptr::write(pt.add(i1), (phys & ADDR_MASK) | flags | FLAG_PRESENT);
    }
    invalidate(virt);
}

/// Removes a single 4 KiB mapping, if present. Does not free any physical
/// frame backing it -- the caller owns that.
pub fn unmap(virt: u64) {
    let [i4, i3, i2, i1] = page_indices(virt);
    unsafe {
        let pml4 = table_at(read_cr3());
        let pml4e = core::ptr::read(pml4.add(i4));
        if pml4e & FLAG_PRESENT == 0 {
            return;
        }
        let pdpt = table_at(pml4e & ADDR_MASK);
        let pdpte = core::ptr::read(pdpt.add(i3));
        if pdpte & FLAG_PRESENT == 0 {
            return;
        }
        let pd = table_at(pdpte & ADDR_MASK);
        let pde = core::ptr::read(pd.add(i2));
        if pde & FLAG_PRESENT == 0 {
            return;
        }
        let pt = table_at(pde & ADDR_MASK);
        core::ptr::write(pt.add(i1), 0);
    }
    invalidate(virt);
}

/// Walks the active page tables to find the physical address `virt` maps to.
pub fn translate(virt: u64) -> Option<u64> {
    let [i4, i3, i2, i1] = page_indices(virt);
    unsafe {
        let pml4 = table_at(read_cr3());
        let pml4e = core::ptr::read(pml4.add(i4));
        if pml4e & FLAG_PRESENT == 0 {
            return None;
        }
        let pdpt = table_at(pml4e & ADDR_MASK);
        let pdpte = core::ptr::read(pdpt.add(i3));
        if pdpte & FLAG_PRESENT == 0 {
            return None;
        }
        let pd = table_at(pdpte & ADDR_MASK);
        let pde = core::ptr::read(pd.add(i2));
        if pde & FLAG_PRESENT == 0 {
            return None;
        }
        let pt = table_at(pde & ADDR_MASK);
        let pte = core::ptr::read(pt.add(i1));
        if pte & FLAG_PRESENT == 0 {
            return None;
        }
        Some((pte & ADDR_MASK) + (virt & 0xFFF))
    }
}
