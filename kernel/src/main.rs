#![no_std]
#![no_main]
// Early-bring-up kernel: memory.rs already exposes the full pmm/paging API
// (free_frame, unmap, translate, ...) that later milestones (heap growth,
// process teardown, page-fault handling) will call.
#![allow(dead_code)]

extern crate alloc;

mod apk;
mod arch;
mod drivers;
mod elf;
mod font;
mod font_hiragana;
mod framebuffer;
mod fs;
mod gui;
mod i18n;
mod limine;
mod memory;
mod net;
mod pe;
mod pkg;
mod power;
mod process;
mod sched;
mod syscall;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

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
            gui::init();
        }
        None => crate::serial_println!("no framebuffer available"),
    }

    {
        let mut root = fs::root().lock();
        root.write("/hello.txt", b"Hello from moon OS RAMFS!\n");
        let contents = root.read("/hello.txt").unwrap();
        crate::serial_println!(
            "ramfs: /hello.txt = {:?}",
            core::str::from_utf8(contents).unwrap_or("<binary>")
        );
        let files: Vec<&str> = root.list().collect();
        crate::serial_println!("ramfs: files = {:?}", files);
    }

    let ahci_ports = drivers::ahci::init();
    for port in &ahci_ports {
        match port.kind {
            drivers::ahci::PortKind::Sata => {
                let mut buf = [0u8; 512];
                if drivers::ahci::identify(port, &mut buf) {
                    crate::serial_println!(
                        "ahci: port {} SATA drive model=\"{}\"",
                        port.index,
                        ata_model_string(&buf)
                    );
                }
            }
            drivers::ahci::PortKind::Atapi => {
                let mut sector = [0u8; 2048];
                if drivers::ahci::atapi_read_sector(port, 16, &mut sector) {
                    let signature_ok = &sector[1..6] == b"CD001";
                    crate::serial_println!(
                        "ahci: port {} ATAPI read of LBA16 succeeded, ISO9660 PVD signature: {}",
                        port.index,
                        if signature_ok {
                            "CD001 (verified!)"
                        } else {
                            "mismatch"
                        }
                    );
                } else {
                    crate::serial_println!("ahci: port {} ATAPI read failed", port.index);
                }
            }
        }
    }

    net::init();
    net::run_demo();

    sched::spawn(task_a);
    sched::spawn(task_b);

    install_bundled_packages();
    match pkg::run("/apps/init.mapp") {
        Ok(name) => crate::serial_println!("pkg: running {}", name),
        Err(err) => crate::serial_println!("pkg: failed to run init.mapp: {}", err),
    }

    match process::spawn_from_pe(PE_TEST_EXE) {
        Ok(entry) => crate::serial_println!("pe: Win32-ish test binary loaded, entry={:#x}", entry),
        Err(err) => crate::serial_println!("pe: failed to load test binary: {}", err),
    }

    inspect_apk_test_fixture();

    crate::serial_println!("scheduler: {} task(s) spawned", sched::task_count());

    arch::x86_64::pit::init(100);
    arch::x86_64::pic::clear_mask(0); // timer
    arch::x86_64::pic::clear_mask(1); // keyboard
    crate::serial_println!("keyboard: IRQ1 unmasked");

    if drivers::mouse::enable() {
        arch::x86_64::pic::clear_mask(2); // cascade, needed for any PIC2 (8-15) IRQ
        arch::x86_64::pic::clear_mask(12); // mouse
        crate::serial_println!("mouse: enabled, IRQ12 unmasked");
    } else {
        crate::serial_println!("mouse: not detected, skipping");
    }

    arch::x86_64::enable_interrupts();
    crate::serial_println!("interrupts enabled, 100 Hz timer running, entering idle loop");

    halt();
}

/// The userland test binaries (`userland/init`, `userland/counter` --
/// separate standalone crates; see their build.rs's and kernel/build.rs for
/// how these paths get here), embedded directly into the kernel image.
/// There's no on-disk package repository yet -- these get wrapped into
/// `.mapp` packages and written into RAMFS at boot (see `install_packages`)
/// so the package manager, File Manager, and Moon Store all have something
/// real to list/install/run through the same format real packages would
/// use later.
static INIT_ELF: &[u8] = include_bytes!(env!("USERLAND_INIT_ELF"));
static COUNTER_ELF: &[u8] = include_bytes!(env!("USERLAND_COUNTER_ELF"));

/// A hand-built PE32+ (Windows) test binary (`tools/pe_test/`, assembled
/// with NASM since no Windows cross-toolchain is available here) that
/// exercises `kernel/src/pe.rs`'s loader and `KERNEL32.DLL` import subset
/// end to end.
static PE_TEST_EXE: &[u8] = include_bytes!(env!("PE_TEST_EXE"));

/// A hand-built test APK -- a minimal ZIP archive with an
/// `AndroidManifest.xml` entry (`tools/apk_test/`) -- exercising
/// `kernel/src/apk.rs`'s ZIP container reader and AXML chunk-header parser.
/// See that module's doc comment for exactly how far "APK support" goes
/// right now (not very far: no DEFLATE, no manifest tree decoding, and
/// nowhere near an actual Dalvik/ART runtime).
static APK_TEST_FILE: &[u8] = include_bytes!(env!("APK_TEST_FILE"));

/// Lists the bundled test APK's ZIP contents, pulls out
/// `AndroidManifest.xml`, and validates its AXML chunk header -- proof the
/// container/format parsing in `apk.rs` works, not a claim that moon OS can
/// run Android apps.
fn inspect_apk_test_fixture() {
    let entries = match apk::list_entries(APK_TEST_FILE) {
        Ok(entries) => entries,
        Err(err) => {
            crate::serial_println!("apk: failed to read test APK: {}", err);
            return;
        }
    };
    crate::serial_println!("apk: test APK has {} entr(y/ies)", entries.len());
    for entry in &entries {
        crate::serial_println!(
            "apk:   {} ({} -> {} bytes, method={})",
            entry.name,
            entry.compressed_size,
            entry.uncompressed_size,
            entry.method
        );
    }

    match apk::find_manifest(APK_TEST_FILE) {
        Ok(manifest_bytes) => match apk::parse_axml_header(&manifest_bytes) {
            Ok(header) => crate::serial_println!(
                "apk: AndroidManifest.xml is valid AXML (chunk_type={:#x}, header_size={}, chunk_size={})",
                header.chunk_type,
                header.header_size,
                header.chunk_size
            ),
            Err(err) => crate::serial_println!("apk: AndroidManifest.xml is not valid AXML: {}", err),
        },
        Err(err) => crate::serial_println!("apk: failed to extract AndroidManifest.xml: {}", err),
    }
}

/// Wraps the bundled test binaries as `.mapp` packages and writes them into
/// RAMFS, so everything downstream (pkg::installed/run, the File Manager,
/// Moon Store) operates on real files through the real package format
/// rather than special-cased embedded bytes.
fn install_bundled_packages() {
    let mut root = fs::root().lock();
    root.write("/apps/init.mapp", &pkg::build("init", "0.1.0", INIT_ELF));
    root.write(
        "/apps/counter.mapp",
        &pkg::build("counter", "0.1.0", COUNTER_ELF),
    );
    crate::serial_println!("pkg: installed 2 bundled package(s) into /apps");
}

static TASK_A_ITERS: AtomicU64 = AtomicU64::new(0);
static TASK_B_ITERS: AtomicU64 = AtomicU64::new(0);

extern "C" fn task_a() -> ! {
    loop {
        let n = TASK_A_ITERS.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(100_000_000) {
            crate::serial_println!("[task A] iteration {}", n);
        }
    }
}

extern "C" fn task_b() -> ! {
    loop {
        let n = TASK_B_ITERS.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(100_000_000) {
            crate::serial_println!("[task B] iteration {}", n);
        }
    }
}

/// ATA IDENTIFY's model string (words 27-46) is ASCII but byte-swapped
/// within each 16-bit word, and space-padded to 40 bytes.
fn ata_model_string(identify: &[u8; 512]) -> String {
    let mut s = String::with_capacity(40);
    for pair in identify[54..94].chunks_exact(2) {
        s.push(pair[1] as char);
        s.push(pair[0] as char);
    }
    s.trim().to_string()
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
