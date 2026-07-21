#![no_std]
#![no_main]

mod arch;
mod font;
mod framebuffer;
mod limine;

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

    if let Some(hhdm) = HHDM_REQUEST.response() {
        crate::serial_println!("HHDM offset: {:#x}", hhdm.offset);
    }

    if let Some(memmap) = MEMMAP_REQUEST.response() {
        let mut usable_bytes: u64 = 0;
        for entry_ptr in memmap.entries() {
            let entry = unsafe { &**entry_ptr };
            if entry.entry_type() == limine::MemmapEntryType::Usable {
                usable_bytes += entry.length;
            }
        }
        crate::serial_println!(
            "memory map: {} entries, {} MiB usable",
            memmap.entry_count,
            usable_bytes / (1024 * 1024)
        );
    }

    match FRAMEBUFFER_REQUEST
        .response()
        .and_then(|r| r.framebuffers().first().copied())
    {
        Some(fb_ptr) => {
            let fb = unsafe { &*fb_ptr };
            crate::serial_println!("framebuffer: {}x{} @ {} bpp", fb.width, fb.height, fb.bpp);
            unsafe { framebuffer::init(fb) };
            crate::fb_println!("moon OS");
            crate::fb_println!("kernel M0 milestone: booted via Limine into 64-bit long mode");
            crate::fb_println!("GDT/TSS, IDT, and this framebuffer console are alive.");
        }
        None => crate::serial_println!("no framebuffer available"),
    }

    crate::serial_println!("kernel init complete, halting.");
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
