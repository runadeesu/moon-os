//! Kernel heap: a fixed virtual range backed by frames from the PMM, served
//! by a first-fit free-list allocator registered as `#[global_allocator]`.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::{align_of, size_of};
use core::ptr::NonNull;
use spin::Mutex;

use super::paging::{self, FLAG_PRESENT, FLAG_WRITABLE};
use super::pmm;

/// Chosen well away from both the kernel image (0xffffffff80000000) and the
/// HHDM (Limine typically puts that around 0xffff800000000000): plenty of
/// higher-half address space is unused and free for us to claim.
const HEAP_START: u64 = 0xffff_ffff_9000_0000;
/// 4 MiB was plenty before the GUI grew a framebuffer backbuffer (see
/// `framebuffer::Console::back`) -- at 1280x800x32bpp that's already ~3.9
/// MiB on its own, permanently. 32 MiB leaves generous headroom for that
/// plus RAMFS files, process images, and everything still to come, well
/// within the 256 MiB QEMU is given (`tools/run.sh`).
const HEAP_SIZE: u64 = 32 * 1024 * 1024;
const PAGE_SIZE: u64 = 4096;

struct FreeNode {
    size: usize,
    next: Option<NonNull<FreeNode>>,
}

struct FreeListAllocator {
    head: FreeNode,
}

// Only ever touched through the `Mutex` in `LockedHeap`.
unsafe impl Send for FreeListAllocator {}

impl FreeListAllocator {
    const fn new() -> Self {
        Self {
            head: FreeNode {
                size: 0,
                next: None,
            },
        }
    }

    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        if size < size_of::<FreeNode>() {
            return;
        }
        debug_assert_eq!(addr % align_of::<FreeNode>(), 0);
        let node = FreeNode {
            size,
            next: self.head.next,
        };
        let node_ptr = addr as *mut FreeNode;
        unsafe { node_ptr.write(node) };
        self.head.next = NonNull::new(node_ptr);
    }

    fn region_fits(addr: usize, size: usize, region_size: usize, align: usize) -> Option<usize> {
        let alloc_start = align_up(addr, align);
        let alloc_end = alloc_start.checked_add(size)?;
        if alloc_end > addr + region_size {
            return None;
        }
        let excess = (addr + region_size) - alloc_end;
        if excess != 0 && excess < size_of::<FreeNode>() {
            return None;
        }
        Some(alloc_start)
    }

    unsafe fn alloc(&mut self, size: usize, align: usize) -> *mut u8 {
        let mut prev = &mut self.head as *mut FreeNode;
        loop {
            let current = unsafe { (*prev).next };
            let Some(mut current_ptr) = current else {
                return core::ptr::null_mut();
            };
            let current_ref = unsafe { current_ptr.as_mut() };
            let region_addr = current_ptr.as_ptr() as usize;

            if let Some(alloc_start) = Self::region_fits(region_addr, size, current_ref.size, align)
            {
                let region_end = region_addr + current_ref.size;
                let next = current_ref.next;
                unsafe { (*prev).next = next };

                let alloc_end = alloc_start + size;
                let excess = region_end - alloc_end;
                if excess > 0 {
                    unsafe { self.add_free_region(alloc_end, excess) };
                }
                return alloc_start as *mut u8;
            }

            prev = current_ptr.as_ptr();
        }
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, size: usize) {
        unsafe { self.add_free_region(ptr as usize, size) };
    }
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

fn size_align(layout: Layout) -> (usize, usize) {
    let layout = layout
        .align_to(align_of::<FreeNode>())
        .expect("invalid layout alignment")
        .pad_to_align();
    (layout.size().max(size_of::<FreeNode>()), layout.align())
}

pub struct LockedHeap(Mutex<FreeListAllocator>);

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (size, align) = size_align(layout);
        unsafe { self.0.lock().alloc(size, align) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let (size, _) = size_align(layout);
        unsafe { self.0.lock().dealloc(ptr, size) };
    }
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap(Mutex::new(FreeListAllocator::new()));

/// Maps `HEAP_SIZE` bytes of fresh physical memory into the fixed kernel
/// heap range and hands it to the global allocator. Must run after `pmm::init`.
pub fn init() {
    let page_count = HEAP_SIZE / PAGE_SIZE;
    for i in 0..page_count {
        let virt = HEAP_START + i * PAGE_SIZE;
        let phys = pmm::alloc_frame().expect("out of physical memory while mapping kernel heap");
        paging::map(virt, phys, FLAG_PRESENT | FLAG_WRITABLE);
    }
    unsafe {
        ALLOCATOR
            .0
            .lock()
            .add_free_region(HEAP_START as usize, HEAP_SIZE as usize);
    }
}
