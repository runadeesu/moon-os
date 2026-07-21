//! Manipulation of x86_64 4-level page tables.
//!
//! `map`/`unmap`/`translate` extend whichever page tables are *currently*
//! active (CR3) -- that was fine while everything ran under one shared
//! address space (Limine's own tables, extended in place). Now that
//! usermode processes get their own (`AddressSpace`), those three stay as
//! thin wrappers around the `_in` variants, which take an explicit PML4
//! physical address instead of always reading CR3.

use core::arch::asm;

pub const FLAG_PRESENT: u64 = 1 << 0;
pub const FLAG_WRITABLE: u64 = 1 << 1;
pub const FLAG_USER: u64 = 1 << 2;
pub const FLAG_NO_EXECUTE: u64 = 1 << 63;

const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
const ENTRIES_PER_TABLE: usize = 512;

/// Index of the first PML4 entry covering the higher half (0xffff800000000000
/// and up): the HHDM and the kernel itself both live above this. Every fresh
/// `AddressSpace` copies entries in `HIGHER_HALF_START..512` from the boot
/// address space so kernel code/data/HHDM stay reachable no matter which
/// process's CR3 is loaded when a trap lands.
const HIGHER_HALF_START: usize = 256;

pub fn current_cr3() -> u64 {
    read_cr3()
}

fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value & ADDR_MASK
}

/// Loads `pml4_phys` into CR3, if it isn't already active. A full TLB flush
/// (implicit in any CR3 write) is only worth paying when the address space
/// is actually changing.
pub fn switch_to(pml4_phys: u64) {
    if read_cr3() != pml4_phys {
        unsafe { asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags)) };
    }
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
/// present yet. `extra_flags` (e.g. `FLAG_USER`) is applied to newly
/// created intermediate entries -- present/writable is always granted at
/// this level; the leaf mapping's own flags are what actually restrict
/// access.
unsafe fn next_table(table: *mut u64, index: usize, extra_flags: u64) -> u64 {
    let entry = unsafe { core::ptr::read(table.add(index)) };
    if entry & FLAG_PRESENT != 0 {
        entry & ADDR_MASK
    } else {
        let frame =
            crate::memory::pmm::alloc_frame().expect("out of physical memory for page tables");
        let frame_virt = table_at(frame);
        unsafe { core::ptr::write_bytes(frame_virt, 0, ENTRIES_PER_TABLE * 8) };
        unsafe {
            core::ptr::write(
                table.add(index),
                frame | FLAG_PRESENT | FLAG_WRITABLE | extra_flags,
            )
        };
        frame
    }
}

/// Maps a single 4 KiB page in the address space rooted at `pml4_phys`.
/// `virt` and `phys` must already be page-aligned.
pub fn map_in(pml4_phys: u64, virt: u64, phys: u64, flags: u64) {
    let [i4, i3, i2, i1] = page_indices(virt);
    let extra = flags & FLAG_USER;
    unsafe {
        let pml4 = table_at(pml4_phys);
        let pdpt = table_at(next_table(pml4, i4, extra));
        let pd = table_at(next_table(pdpt, i3, extra));
        let pt = table_at(next_table(pd, i2, extra));
        core::ptr::write(pt.add(i1), (phys & ADDR_MASK) | flags | FLAG_PRESENT);
    }
    if pml4_phys == read_cr3() {
        invalidate(virt);
    }
}

/// Removes a single 4 KiB mapping, if present, from the address space
/// rooted at `pml4_phys`. Does not free any physical frame backing it --
/// the caller owns that.
pub fn unmap_in(pml4_phys: u64, virt: u64) {
    let [i4, i3, i2, i1] = page_indices(virt);
    unsafe {
        let pml4 = table_at(pml4_phys);
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
    if pml4_phys == read_cr3() {
        invalidate(virt);
    }
}

/// Walks the page tables rooted at `pml4_phys` to find the physical address
/// `virt` maps to.
pub fn translate_in(pml4_phys: u64, virt: u64) -> Option<u64> {
    let [i4, i3, i2, i1] = page_indices(virt);
    unsafe {
        let pml4 = table_at(pml4_phys);
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

/// Maps a single 4 KiB page in the *currently active* address space.
pub fn map(virt: u64, phys: u64, flags: u64) {
    map_in(read_cr3(), virt, phys, flags);
}

/// Removes a single 4 KiB mapping from the *currently active* address space.
pub fn unmap(virt: u64) {
    unmap_in(read_cr3(), virt);
}

/// Walks the *currently active* page tables to find the physical address
/// `virt` maps to.
pub fn translate(virt: u64) -> Option<u64> {
    translate_in(read_cr3(), virt)
}

/// An independent top-level page table -- a process's own address space.
/// Every fresh one starts with the boot address space's higher half (the
/// kernel and the HHDM) already present, so kernel code keeps running and
/// the HHDM keeps resolving physical frames no matter whose CR3 is loaded
/// when a trap or interrupt lands; the lower half starts empty for the ELF
/// loader to populate.
pub struct AddressSpace {
    pml4_phys: u64,
}

impl AddressSpace {
    pub fn new() -> Self {
        let pml4_phys =
            crate::memory::pmm::alloc_frame().expect("out of physical memory for a new PML4");
        let pml4 = table_at(pml4_phys);
        unsafe { core::ptr::write_bytes(pml4, 0, ENTRIES_PER_TABLE * 8) };

        let boot_pml4 = table_at(read_cr3());
        for i in HIGHER_HALF_START..ENTRIES_PER_TABLE {
            unsafe {
                let entry = core::ptr::read(boot_pml4.add(i));
                core::ptr::write(pml4.add(i), entry);
            }
        }

        Self { pml4_phys }
    }

    pub fn cr3(&self) -> u64 {
        self.pml4_phys
    }

    /// Maps a page into this address space specifically, regardless of
    /// which one is currently active.
    pub fn map(&self, virt: u64, phys: u64, flags: u64) {
        map_in(self.pml4_phys, virt, phys, flags);
    }
}

impl Default for AddressSpace {
    fn default() -> Self {
        Self::new()
    }
}
