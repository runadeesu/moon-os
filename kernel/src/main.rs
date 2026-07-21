#![no_std]
#![no_main]
// Early-bring-up kernel: memory.rs already exposes the full pmm/paging API
// (free_frame, unmap, translate, ...) that later milestones (heap growth,
// process teardown, page-fault handling) will call.
#![allow(dead_code)]

extern crate alloc;

mod arch;
mod font;
mod framebuffer;
mod limine;
mod memory;

use alloc::vec::Vec;
use core::panic::PanicInfo;

#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: limine::RequestsStartMarker = limine::RequestsStartMarker::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: limine::RequestsEndMarker = limine::RequestsEndMarker::new();

#[used]
#[link_section = ".requests"]
static BASE_REVISION: limine::BaseRevision = limine::BaseRevision::new(3);

#[used]
#[link_section = ".requests"]
static BOOTLOADER_INFO_REQUEST: limine::BootloaderInfoRequest =
    limine::BootloaderInfoRequest::new();

#[used]
#[link_section = ".requests"]
static HHDM_REQUEST: limine::HhdmRequest = limine::HhdmRequest::new();

#[used]
#[link_section = ".requests"]
static FRAMEBUFFER_REQUEST: limine::FramebufferRequest = limine::FramebufferRequest::new();

#[used]
#[link_section = ".requests"]
static MEMMAP_REQUEST: limine::MemmapRequest = limine::MemmapRequest::new();

#[no_mangle]
extern "C" fn kmain() -> ! {
    arch::x86_64::init();

    crate::serial_println!("moon OS kernel booting...");

    if !BASE_REVISION.is_supported() {
        crate::serial_println!(
            "FATAL: bootloader does not support the requested Limine base revision"
        );
        halt();
    }

    if let Some(info) = BOOTLOADER_INFO_REQUEST.response() {
        crate::serial_println!("bootloader: {} {}", info.name(), info.version());
    }

    let hhdm = HHDM_REQUEST
        .response()
        .unwrap_or_else(|| fatal("no HHDM response from bootloader"));
    crate::serial_println!("HHDM offset: {:#x}", hhdm.offset);
    memory::set_hhdm_offset(hhdm.offset);

    let memmap = MEMMAP_REQUEST
        .response()
        .unwrap_or_else(|| fatal("no memory map response from bootloader"));
    memory::pmm::init(memmap);
    let stats = memory::pmm::stats();
    crate::serial_println!(
        "pmm: {} MiB total, {} MiB free ({} 4K frames)",
        (stats.total_frames * 4096) / (1024 * 1024),
        (stats.free_frames * 4096) / (1024 * 1024),
        stats.total_frames
    );

    memory::heap::init();
    crate::serial_println!("heap: mapped and handed to the global allocator");

    let mut v: Vec<u32> = Vec::new();
    for i in 0..16 {
        v.push(i * i);
    }
    let sum: u32 = v.iter().sum();
    crate::serial_println!(
        "heap self-test: Vec<u32> of {} squares, sum={}",
        v.len(),
        sum
    );
    drop(v);

    match FRAMEBUFFER_REQUEST
        .response()
        .and_then(|r| r.framebuffers().first().copied())
    {
        Some(fb_ptr) => {
            let fb = unsafe { &*fb_ptr };
            crate::serial_println!("framebuffer: {}x{} @ {} bpp", fb.width, fb.height, fb.bpp);
            unsafe { framebuffer::init(fb) };
            crate::fb_println!("moon OS");
            crate::fb_println!("kernel M1 milestone: PMM + paging + heap allocator are alive");
            crate::fb_println!("heap self-test: Vec<u32> of {} squares, sum={}", 16, sum);
        }
        None => crate::serial_println!("no framebuffer available"),
    }

    crate::serial_println!("kernel init complete, halting.");
    halt();
}

fn fatal(message: &str) -> ! {
    crate::serial_println!("FATAL: {}", message);
    halt();
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::serial_println!("KERNEL PANIC: {}", info);
    halt();
}
