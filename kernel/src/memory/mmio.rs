//! Maps device MMIO regions (PCI BARs and the like) into virtual memory.
//!
//! The HHDM only covers memory map regions typed Usable, BootloaderReclaimable,
//! ExecutableAndModules, or Framebuffer (see the Limine protocol's base
//! revision 3 semantics) -- MMIO BARs live in `Reserved` physical ranges, so
//! they need their own mapping, and one we mark cache-disabled, since this is
//! device memory, not RAM.

use super::paging::{self, FLAG_PRESENT, FLAG_WRITABLE};
use core::sync::atomic::{AtomicU64, Ordering};

const MMIO_VIRT_BASE: u64 = 0xffff_ffff_a000_0000;
const CACHE_DISABLE: u64 = 1 << 4; // PCD
const PAGE_SIZE: u64 = 4096;

static NEXT_VIRT: AtomicU64 = AtomicU64::new(MMIO_VIRT_BASE);

/// Maps `size` bytes of physical MMIO space starting at `phys` (need not be
/// page-aligned) into a fresh, never-reused virtual range, returning a
/// pointer to `phys`'s own byte offset within it.
pub fn map(phys: u64, size: u64) -> *mut u8 {
    let phys_page_start = phys & !(PAGE_SIZE - 1);
    let offset_in_page = phys - phys_page_start;
    let page_count = (offset_in_page + size).div_ceil(PAGE_SIZE);

    let virt_start = NEXT_VIRT.fetch_add(page_count * PAGE_SIZE, Ordering::Relaxed);
    for i in 0..page_count {
        paging::map(
            virt_start + i * PAGE_SIZE,
            phys_page_start + i * PAGE_SIZE,
            FLAG_PRESENT | FLAG_WRITABLE | CACHE_DISABLE,
        );
    }

    (virt_start + offset_in_page) as *mut u8
}
