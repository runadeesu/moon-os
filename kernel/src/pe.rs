//! A minimal PE32+ (64-bit Windows executable) loader -- the first slice of
//! the "EXE compatibility layer" from ROADMAP.md's M8. Parses the DOS/PE/
//! COFF/Optional headers and section table (works on any well-formed
//! x86_64 PE32+ image, not just our own hand-built test binary), maps
//! sections into a fresh [`AddressSpace`], and resolves a small subset of
//! `KERNEL32.DLL` imports (`ExitProcess`, `WriteConsoleA`) by synthesizing
//! tiny machine-code thunks in the target process's own address space that
//! translate the Win64 calling convention into moon OS's `int 0x80` ABI.
//!
//! Deliberately not attempted: relocations/ASLR (our test image is built
//! non-relocatable, fixed `ImageBase`), delayed/ordinal imports, the
//! resource/exception/TLS directories, and anything beyond this tiny
//! import subset -- a real Win32 API surface is enormous and grows here
//! incrementally, not all at once. See ROADMAP.md for the honest tally of
//! what this is and isn't yet.

use crate::memory::paging::{self, AddressSpace};
use alloc::string::String;
use core::arch::naked_asm;

const IMAGE_DOS_SIGNATURE: u16 = 0x5A4D; // "MZ"
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550; // "PE\0\0"
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_NT_OPTIONAL_HDR64_MAGIC: u16 = 0x20B;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;
const IMAGE_ORDINAL_FLAG64: u64 = 1 << 63;

const PAGE_SIZE: u64 = 4096;

/// Virtual address (inside the loaded process's own address space) of the
/// page holding synthesized import thunks -- picked well clear of both a
/// typical `ImageBase` and the user stack `process.rs` sets up.
const THUNK_PAGE: u64 = 0x0000_0000_7200_0000;
const THUNK_SLOT_SIZE: u64 = 64;
/// Deliberately generous rather than tracking each template's exact
/// assembled length: every slot copies this many bytes from the template,
/// and anything past its `ret` is dead code that's copied but never
/// reached.
const THUNK_COPY_LEN: usize = 64;

pub struct Loaded {
    pub entry: u64,
}

fn read_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn read_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn read_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    raw_ptr: u32,
    characteristics: u32,
}

/// Parses `data` as a PE32+ executable and maps it into `space`. Returns
/// the entry point virtual address (`ImageBase + AddressOfEntryPoint`) on
/// success.
pub fn load(space: &AddressSpace, data: &[u8]) -> Result<Loaded, &'static str> {
    if data.len() < 0x40 || read_u16(data, 0) != IMAGE_DOS_SIGNATURE {
        return Err("not a PE file (bad DOS signature)");
    }
    let pe_offset = read_u32(data, 0x3C) as usize;
    if pe_offset + 24 > data.len() || read_u32(data, pe_offset) != IMAGE_NT_SIGNATURE {
        return Err("not a PE file (bad NT signature)");
    }

    let coff_offset = pe_offset + 4;
    if read_u16(data, coff_offset) != IMAGE_FILE_MACHINE_AMD64 {
        return Err("not an x86_64 PE image");
    }
    let number_of_sections = read_u16(data, coff_offset + 2) as usize;
    let size_of_optional_header = read_u16(data, coff_offset + 16) as usize;

    let opt_offset = coff_offset + 20;
    if opt_offset + 2 > data.len() || read_u16(data, opt_offset) != IMAGE_NT_OPTIONAL_HDR64_MAGIC {
        return Err("not a PE32+ (64-bit) image");
    }
    let entry_rva = read_u32(data, opt_offset + 16) as u64;
    let image_base = read_u64(data, opt_offset + 24);
    let number_of_rva_and_sizes = read_u32(data, opt_offset + 108) as usize;

    let data_directory = opt_offset + 112;
    let (import_dir_rva, import_dir_size) = if number_of_rva_and_sizes > 1 {
        (
            read_u32(data, data_directory + 8),
            read_u32(data, data_directory + 12),
        )
    } else {
        (0, 0)
    };

    let section_table_offset = opt_offset + size_of_optional_header;
    let mut sections = alloc::vec::Vec::with_capacity(number_of_sections);
    for i in 0..number_of_sections {
        let off = section_table_offset + i * 40;
        if off + 40 > data.len() {
            return Err("section header out of bounds");
        }
        sections.push(Section {
            virtual_address: read_u32(data, off + 12),
            virtual_size: read_u32(data, off + 8),
            raw_size: read_u32(data, off + 16),
            raw_ptr: read_u32(data, off + 20),
            characteristics: read_u32(data, off + 36),
        });
    }

    for section in &sections {
        load_section(space, data, image_base, section)?;
    }

    if import_dir_size > 0 {
        resolve_imports(space, image_base, import_dir_rva)?;
    }

    Ok(Loaded {
        entry: image_base + entry_rva,
    })
}

fn load_section(
    space: &AddressSpace,
    data: &[u8],
    image_base: u64,
    section: &Section,
) -> Result<(), &'static str> {
    let mem_size = if section.virtual_size == 0 {
        section.raw_size
    } else {
        section.virtual_size
    } as u64;
    if mem_size == 0 {
        return Ok(());
    }

    let flags = paging::FLAG_PRESENT
        | paging::FLAG_USER
        | if section.characteristics & IMAGE_SCN_MEM_WRITE != 0 {
            paging::FLAG_WRITABLE
        } else {
            0
        };

    let base_va = image_base + section.virtual_address as u64;
    let start_page = base_va & !(PAGE_SIZE - 1);
    let end_page = (base_va + mem_size).div_ceil(PAGE_SIZE) * PAGE_SIZE;

    let mut page = start_page;
    while page < end_page {
        let frame = crate::memory::pmm::alloc_frame().ok_or("out of memory loading PE section")?;
        let frame_virt = crate::memory::phys_to_virt(frame);
        unsafe { core::ptr::write_bytes(frame_virt, 0, PAGE_SIZE as usize) };
        space.map(page, frame, flags);
        page += PAGE_SIZE;
    }

    let raw_size = section.raw_size as u64;
    if raw_size == 0 {
        return Ok(());
    }
    let raw_ptr = section.raw_ptr as u64;
    if (raw_ptr + raw_size) as usize > data.len() {
        return Err("PE section data out of bounds");
    }

    let mut copied = 0u64;
    while copied < raw_size {
        let va = base_va + copied;
        let page_base = va & !(PAGE_SIZE - 1);
        let page_off = va - page_base;
        let chunk = (PAGE_SIZE - page_off).min(raw_size - copied);

        let phys = paging::translate_in(space.cr3(), page_base)
            .ok_or("PE section page vanished mid-copy")?;
        let dst = crate::memory::phys_to_virt(phys + page_off);
        let src_off = (raw_ptr + copied) as usize;
        let src = &data[src_off..src_off + chunk as usize];
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, chunk as usize) };

        copied += chunk;
    }

    Ok(())
}

fn read_va_u8(space: &AddressSpace, va: u64) -> Option<u8> {
    let page = va & !(PAGE_SIZE - 1);
    let off = va & (PAGE_SIZE - 1);
    let phys = paging::translate_in(space.cr3(), page)?;
    Some(unsafe { core::ptr::read(crate::memory::phys_to_virt(phys + off)) })
}

fn write_va_u8(space: &AddressSpace, va: u64, value: u8) -> Option<()> {
    let page = va & !(PAGE_SIZE - 1);
    let off = va & (PAGE_SIZE - 1);
    let phys = paging::translate_in(space.cr3(), page)?;
    unsafe { core::ptr::write(crate::memory::phys_to_virt(phys + off), value) };
    Some(())
}

fn read_va_u32(space: &AddressSpace, va: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = read_va_u8(space, va + i as u64)?;
    }
    Some(u32::from_le_bytes(b))
}

fn read_va_u64(space: &AddressSpace, va: u64) -> Option<u64> {
    let lo = read_va_u32(space, va)? as u64;
    let hi = read_va_u32(space, va + 4)? as u64;
    Some(lo | (hi << 32))
}

fn write_va_u64(space: &AddressSpace, va: u64, value: u64) -> Option<()> {
    for i in 0..8u64 {
        write_va_u8(space, va + i, ((value >> (i * 8)) & 0xFF) as u8)?;
    }
    Some(())
}

fn read_va_cstr(space: &AddressSpace, va: u64) -> Option<String> {
    let mut out = String::new();
    for i in 0..256u64 {
        let b = read_va_u8(space, va + i)?;
        if b == 0 {
            return Some(out);
        }
        out.push(b as char);
    }
    None
}

/// Maps a supported `(DLL, function)` import to the address of its thunk
/// template. Only the tiny subset a hand-built test image actually needs;
/// extending real Win32 coverage means adding cases (and templates) here.
fn thunk_template_for(dll: &str, name: &str) -> Option<usize> {
    match (dll, name) {
        ("KERNEL32.DLL", "ExitProcess") => Some(exitprocess_thunk as *const () as usize),
        ("KERNEL32.DLL", "WriteConsoleA") => Some(writeconsolea_thunk as *const () as usize),
        _ => None,
    }
}

/// Walks the Import Directory Table, synthesizing a thunk for each
/// supported import into a freshly mapped page and patching the IAT
/// (`FirstThunk`) to point at it -- so `call [iat_slot]` from the loaded
/// image lands on our stub instead of a real DLL that doesn't exist here.
fn resolve_imports(
    space: &AddressSpace,
    image_base: u64,
    import_dir_rva: u32,
) -> Result<(), &'static str> {
    if import_dir_rva == 0 {
        return Ok(());
    }

    let thunk_frame = crate::memory::pmm::alloc_frame().ok_or("out of memory for import thunks")?;
    unsafe {
        core::ptr::write_bytes(
            crate::memory::phys_to_virt(thunk_frame),
            0xCC, // int3 filler -- these bytes are never meant to be reached
            PAGE_SIZE as usize,
        )
    };
    space.map(
        THUNK_PAGE,
        thunk_frame,
        paging::FLAG_PRESENT | paging::FLAG_USER,
    );
    let max_slots = PAGE_SIZE / THUNK_SLOT_SIZE;
    let mut next_slot = 0u64;

    let mut desc_va = image_base + import_dir_rva as u64;
    loop {
        let original_first_thunk = read_va_u32(space, desc_va).ok_or("bad import descriptor")?;
        let name_rva = read_va_u32(space, desc_va + 12).ok_or("bad import descriptor")?;
        let first_thunk = read_va_u32(space, desc_va + 16).ok_or("bad import descriptor")?;
        if original_first_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }

        let dll_name = read_va_cstr(space, image_base + name_rva as u64)
            .ok_or("bad import DLL name")?
            .to_ascii_uppercase();

        let ilt_rva = if original_first_thunk != 0 {
            original_first_thunk
        } else {
            first_thunk
        };

        let mut idx = 0u64;
        loop {
            let thunk_va = image_base + ilt_rva as u64 + idx * 8;
            let entry = read_va_u64(space, thunk_va).ok_or("bad import thunk entry")?;
            if entry == 0 {
                break;
            }
            if entry & IMAGE_ORDINAL_FLAG64 != 0 {
                return Err("ordinal (non-named) imports are not supported");
            }

            let fn_name =
                read_va_cstr(space, image_base + entry + 2).ok_or("bad import function name")?;

            let template = thunk_template_for(&dll_name, &fn_name)
                .ok_or("unsupported import (only a small subset is implemented)")?;

            if next_slot >= max_slots {
                return Err("too many imports for one thunk page");
            }
            let slot_va = THUNK_PAGE + next_slot * THUNK_SLOT_SIZE;
            let slot_phys = paging::translate_in(space.cr3(), THUNK_PAGE)
                .ok_or("thunk page vanished")?
                + next_slot * THUNK_SLOT_SIZE;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    template as *const u8,
                    crate::memory::phys_to_virt(slot_phys),
                    THUNK_COPY_LEN,
                )
            };
            next_slot += 1;

            let iat_va = image_base + first_thunk as u64 + idx * 8;
            write_va_u64(space, iat_va, slot_va).ok_or("failed to patch IAT")?;

            idx += 1;
        }

        desc_va += 20;
    }

    Ok(())
}

// -- Import thunk templates --------------------------------------------
//
// Each is a tiny, hand-written machine-code sequence, compiled as ordinary
// (never directly called) kernel functions purely so the assembler
// generates their bytes for us; `resolve_imports` copies those bytes into
// the target process's own thunk page. They translate the Win64 calling
// convention (args in rcx/rdx/r8/r9) into moon OS's own `int 0x80` ABI
// (args in rdi/rsi, syscall number in rax) -- see `kernel/src/syscall.rs`.

/// `void ExitProcess(UINT uExitCode)` -- uExitCode arrives in ecx.
#[unsafe(naked)]
extern "C" fn exitprocess_thunk() {
    naked_asm!("mov edi, ecx", "xor eax, eax", "int 0x80", "ret")
}

/// `BOOL WriteConsoleA(HANDLE, LPCVOID lpBuffer, DWORD nNumberOfCharsToWrite,
/// LPDWORD lpNumberOfCharsWritten, LPVOID lpReserved)` -- buffer in rdx,
/// count in r8d, out-written-count pointer in r9 (may be null).
#[unsafe(naked)]
extern "C" fn writeconsolea_thunk() {
    naked_asm!(
        "mov rdi, rdx",
        "mov esi, r8d",
        "mov eax, 1",
        "int 0x80",
        "test r9, r9",
        "jz 2f",
        "mov [r9], eax",
        "2:",
        "mov eax, 1",
        "ret",
    )
}
