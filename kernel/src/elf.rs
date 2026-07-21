//! A minimal ELF64 loader: parses just enough of the header and PT_LOAD
//! program headers to map a static, non-PIE executable into a fresh
//! [`AddressSpace`] and hand back its entry point. No dynamic linking, no
//! relocations, no PIE -- moon OS's own userland (`userland/init`) is built
//! as a plain statically-linked ET_EXEC binary at a fixed base, which is
//! all this needs to support for now.

use crate::memory::paging::{self, AddressSpace};

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PF_W: u32 = 2;

const PAGE_SIZE: u64 = 4096;

fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}

fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}

fn read_u64(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}

pub struct Loaded {
    pub entry: u64,
}

/// Parses `data` as an ELF64 executable and maps its PT_LOAD segments into
/// `space`. Returns the entry point virtual address on success.
pub fn load(space: &AddressSpace, data: &[u8]) -> Result<Loaded, &'static str> {
    if data.len() < 64 || data[0..4] != ELF_MAGIC {
        return Err("not an ELF file");
    }
    if data[4] != ELFCLASS64 {
        return Err("not a 64-bit ELF");
    }
    if data[5] != ELFDATA2LSB {
        return Err("not little-endian");
    }

    let e_type = read_u16(data, 16);
    let e_machine = read_u16(data, 18);
    let e_entry = read_u64(data, 24);
    let e_phoff = read_u64(data, 32);
    let e_phentsize = read_u16(data, 54);
    let e_phnum = read_u16(data, 56);

    if e_type != ET_EXEC {
        return Err("not a static ET_EXEC binary (no PIE/dynamic-link support yet)");
    }
    if e_machine != EM_X86_64 {
        return Err("not an x86_64 binary");
    }

    for i in 0..e_phnum as usize {
        let ph_off = e_phoff as usize + i * e_phentsize as usize;
        if ph_off + 56 > data.len() {
            return Err("program header out of bounds");
        }
        let p_type = read_u32(data, ph_off);
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = read_u32(data, ph_off + 4);
        let p_offset = read_u64(data, ph_off + 8);
        let p_vaddr = read_u64(data, ph_off + 16);
        let p_filesz = read_u64(data, ph_off + 32);
        let p_memsz = read_u64(data, ph_off + 40);

        load_segment(space, data, p_offset, p_vaddr, p_filesz, p_memsz, p_flags)?;
    }

    Ok(Loaded { entry: e_entry })
}

/// Allocates+maps zeroed pages covering `[vaddr, vaddr+memsz)`, then copies
/// the file-backed `[offset, offset+filesz)` bytes in (`filesz <= memsz`;
/// the rest -- e.g. .bss -- stays zeroed, matching what a loader owes any
/// ELF segment).
fn load_segment(
    space: &AddressSpace,
    data: &[u8],
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    flags: u32,
) -> Result<(), &'static str> {
    if memsz == 0 {
        return Ok(());
    }

    // No FLAG_NO_EXECUTE here even for non-executable segments: that bit is
    // reserved (not just permission-checked) unless EFER.NXE is enabled,
    // which nothing sets up yet -- setting it now would just be a #GP
    // waiting to happen. W^X enforcement is a gap for later, not today.
    let page_flags = paging::FLAG_PRESENT
        | paging::FLAG_USER
        | if flags & PF_W != 0 {
            paging::FLAG_WRITABLE
        } else {
            0
        };

    let start_page = vaddr & !(PAGE_SIZE - 1);
    let end_page = (vaddr + memsz).div_ceil(PAGE_SIZE) * PAGE_SIZE;

    let mut page = start_page;
    while page < end_page {
        let frame = crate::memory::pmm::alloc_frame().ok_or("out of memory loading ELF segment")?;
        let frame_virt = crate::memory::phys_to_virt(frame);
        unsafe { core::ptr::write_bytes(frame_virt, 0, PAGE_SIZE as usize) };
        space.map(page, frame, page_flags);
        page += PAGE_SIZE;
    }

    // Copy the file-backed bytes in a page at a time (translating each
    // destination page once, not once per byte).
    let mut copied = 0u64;
    while copied < filesz {
        let va = vaddr + copied;
        let page_base = va & !(PAGE_SIZE - 1);
        let page_off = va - page_base;
        let chunk = (PAGE_SIZE - page_off).min(filesz - copied);

        let phys = paging::translate_in(space.cr3(), page_base)
            .ok_or("ELF segment page vanished mid-copy")?;
        let dst = crate::memory::phys_to_virt(phys + page_off);
        let src = &data[(offset + copied) as usize..(offset + copied + chunk) as usize];
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), dst, chunk as usize) };

        copied += chunk;
    }

    Ok(())
}
