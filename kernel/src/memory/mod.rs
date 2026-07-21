//! Physical/virtual memory management: frame allocation, page table
//! manipulation, and the kernel heap.

pub mod heap;
pub mod paging;
pub mod pmm;

use core::sync::atomic::{AtomicU64, Ordering};

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Must be called once, before any other function in this module, with the
/// offset Limine reported for the Higher Half Direct Map.
pub fn set_hhdm_offset(offset: u64) {
    HHDM_OFFSET.store(offset, Ordering::Relaxed);
}

pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

/// Translates a physical address to the corresponding HHDM virtual address.
/// Only valid for physical memory Limine actually mapped into the HHDM
/// (usable, bootloader-reclaimable, executable/modules, framebuffer -- see
/// base revision 3 semantics in the Limine protocol spec).
pub fn phys_to_virt(phys: u64) -> *mut u8 {
    (phys + hhdm_offset()) as *mut u8
}
