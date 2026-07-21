//! Bitmap physical frame allocator.
//!
//! Only `Usable` memory map entries are ever handed out. `BootloaderReclaimable`
//! regions are deliberately left alone for now: we are still running on
//! Limine's own page tables (we haven't built and switched to our own yet),
//! and some of that reclaimable memory backs those very page tables. Once a
//! later milestone builds and switches to a kernel-owned address space, those
//! regions can be folded into the allocator safely.

use crate::limine::{MemmapEntryType, MemmapResponse};
use spin::Mutex;

const PAGE_SIZE: u64 = 4096;

struct BitmapAllocator {
    bitmap: &'static mut [u8],
    frame_count: u64,
    free_frames: u64,
    next_hint: u64,
}

impl BitmapAllocator {
    fn mark(&mut self, frame: u64, used: bool) {
        let byte = (frame / 8) as usize;
        let bit = (frame % 8) as u8;
        let was_used = self.bitmap[byte] & (1 << bit) != 0;
        if used {
            self.bitmap[byte] |= 1 << bit;
        } else {
            self.bitmap[byte] &= !(1 << bit);
        }
        match (was_used, used) {
            (false, true) => self.free_frames -= 1,
            (true, false) => self.free_frames += 1,
            _ => {}
        }
    }

    fn is_free(&self, frame: u64) -> bool {
        let byte = (frame / 8) as usize;
        let bit = (frame % 8) as u8;
        self.bitmap[byte] & (1 << bit) == 0
    }

    fn mark_range_free(&mut self, base: u64, length: u64) {
        let start = base / PAGE_SIZE;
        let end = (base + length) / PAGE_SIZE;
        for frame in start..end {
            self.mark(frame, false);
        }
    }

    fn mark_range_used(&mut self, base: u64, length: u64) {
        let start = base / PAGE_SIZE;
        let end = (base + length).div_ceil(PAGE_SIZE);
        for frame in start..end {
            self.mark(frame, true);
        }
    }

    fn alloc(&mut self) -> Option<u64> {
        for offset in 0..self.frame_count {
            let frame = (self.next_hint + offset) % self.frame_count;
            if self.is_free(frame) {
                self.mark(frame, true);
                self.next_hint = frame + 1;
                return Some(frame * PAGE_SIZE);
            }
        }
        None
    }

    /// Finds `count` consecutive free frames (needed for DMA rings like the
    /// RTL8139's RX buffer, which the hardware treats as one contiguous
    /// physical region). Plain linear scan -- fine since this only runs a
    /// handful of times, at driver init.
    fn alloc_contiguous(&mut self, count: u64) -> Option<u64> {
        if count == 0 {
            return None;
        }
        let mut run_start = None;
        let mut run_len = 0u64;
        for frame in 0..self.frame_count {
            if self.is_free(frame) {
                if run_start.is_none() {
                    run_start = Some(frame);
                }
                run_len += 1;
                if run_len == count {
                    let start = run_start.unwrap();
                    for f in start..start + count {
                        self.mark(f, true);
                    }
                    return Some(start * PAGE_SIZE);
                }
            } else {
                run_start = None;
                run_len = 0;
            }
        }
        None
    }

    fn free(&mut self, phys: u64) {
        self.mark(phys / PAGE_SIZE, false);
    }
}

static PMM: Mutex<Option<BitmapAllocator>> = Mutex::new(None);

pub struct MemoryStats {
    pub total_frames: u64,
    pub free_frames: u64,
}

/// Builds the frame bitmap from Limine's memory map. Must run after
/// `memory::set_hhdm_offset`.
pub fn init(memmap: &MemmapResponse) {
    let mut max_addr = 0u64;
    for entry_ptr in memmap.entries() {
        let entry = unsafe { &**entry_ptr };
        if entry.entry_type() == MemmapEntryType::Usable {
            max_addr = max_addr.max(entry.base + entry.length);
        }
    }

    let frame_count = max_addr / PAGE_SIZE;
    let bitmap_bytes = frame_count.div_ceil(8);

    let mut bitmap_phys = None;
    for entry_ptr in memmap.entries() {
        let entry = unsafe { &**entry_ptr };
        if entry.entry_type() == MemmapEntryType::Usable && entry.length >= bitmap_bytes {
            bitmap_phys = Some(entry.base);
            break;
        }
    }
    let bitmap_phys = bitmap_phys.expect("no usable region large enough for the frame bitmap");

    let bitmap_ptr = crate::memory::phys_to_virt(bitmap_phys);
    let bitmap = unsafe {
        core::ptr::write_bytes(bitmap_ptr, 0xFF, bitmap_bytes as usize);
        core::slice::from_raw_parts_mut(bitmap_ptr, bitmap_bytes as usize)
    };

    let mut allocator = BitmapAllocator {
        bitmap,
        frame_count,
        free_frames: 0,
        next_hint: 0,
    };

    for entry_ptr in memmap.entries() {
        let entry = unsafe { &**entry_ptr };
        if entry.entry_type() == MemmapEntryType::Usable {
            allocator.mark_range_free(entry.base, entry.length);
        }
    }
    allocator.mark_range_used(bitmap_phys, bitmap_bytes);

    *PMM.lock() = Some(allocator);
}

pub fn alloc_frame() -> Option<u64> {
    PMM.lock().as_mut()?.alloc()
}

/// Allocates `count` physically contiguous 4K frames, returning the base
/// address. See [`BitmapAllocator::alloc_contiguous`].
pub fn alloc_contiguous(count: u64) -> Option<u64> {
    PMM.lock().as_mut()?.alloc_contiguous(count)
}

pub fn free_frame(phys: u64) {
    if let Some(allocator) = PMM.lock().as_mut() {
        allocator.free(phys);
    }
}

pub fn stats() -> MemoryStats {
    let guard = PMM.lock();
    let allocator = guard.as_ref().expect("pmm::init was not called");
    MemoryStats {
        total_frames: allocator.frame_count,
        free_frames: allocator.free_frames,
    }
}
