//! The first, deliberately small slice of "APK compatibility" from
//! ROADMAP.md's M8: a ZIP container reader (an APK *is* a ZIP archive)
//! good enough to list entries and pull out `AndroidManifest.xml`, plus
//! enough of the Android Binary XML (AXML) format to validate that what
//! we pulled out really is one and read its chunk header.
//!
//! What this deliberately does *not* do: decompress DEFLATE entries (only
//! STORED/uncompressed ones -- real-world APKs are almost always DEFLATE
//! -compressed, so this can't read a genuine downloaded APK's manifest
//! yet; a real inflate implementation is a substantial follow-up), decode
//! the AXML string pool or element tree (package name, permissions,
//! activities -- all future work), or anything resembling actually running
//! Dalvik/ART bytecode. That last piece is not "write more parsing code" --
//! it means integrating a real Android runtime, which is its own project,
//! not something this pass attempts even a first slice of.

use alloc::string::String;
use alloc::vec::Vec;

const EOCD_SIGNATURE: u32 = 0x0605_4B50;
const CENTRAL_DIR_SIGNATURE: u32 = 0x0201_4B50;
const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4B50;

const METHOD_STORED: u16 = 0;

fn read_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    local_header_offset: u32,
}

/// Finds the End Of Central Directory record by scanning backward from the
/// end of the file -- it's a fixed 22-byte structure, but a variable-length
/// (and rarely used) trailing comment means its exact offset isn't fixed,
/// so a real reader has to search for the signature rather than assume
/// it's at `len - 22`.
fn find_eocd(data: &[u8]) -> Result<usize, &'static str> {
    if data.len() < 22 {
        return Err("not a ZIP file (too small)");
    }
    let search_start = data.len().saturating_sub(22 + 0xFFFF);
    for offset in (search_start..=data.len() - 22).rev() {
        if read_u32(data, offset) == Some(EOCD_SIGNATURE) {
            return Ok(offset);
        }
    }
    Err("not a ZIP file (no End Of Central Directory record found)")
}

/// Lists every entry in the ZIP's central directory.
pub fn list_entries(data: &[u8]) -> Result<Vec<ZipEntry>, &'static str> {
    let eocd = find_eocd(data)?;
    let entry_count = read_u16(data, eocd + 10).ok_or("truncated EOCD record")? as usize;
    let central_dir_offset = read_u32(data, eocd + 16).ok_or("truncated EOCD record")? as usize;

    let mut entries = Vec::with_capacity(entry_count);
    let mut offset = central_dir_offset;
    for _ in 0..entry_count {
        if read_u32(data, offset) != Some(CENTRAL_DIR_SIGNATURE) {
            return Err("malformed central directory entry");
        }
        let method = read_u16(data, offset + 10).ok_or("truncated central directory entry")?;
        let compressed_size =
            read_u32(data, offset + 20).ok_or("truncated central directory entry")?;
        let uncompressed_size =
            read_u32(data, offset + 24).ok_or("truncated central directory entry")?;
        let name_len =
            read_u16(data, offset + 28).ok_or("truncated central directory entry")? as usize;
        let extra_len =
            read_u16(data, offset + 30).ok_or("truncated central directory entry")? as usize;
        let comment_len =
            read_u16(data, offset + 32).ok_or("truncated central directory entry")? as usize;
        let local_header_offset =
            read_u32(data, offset + 42).ok_or("truncated central directory entry")?;

        let name_start = offset + 46;
        let name_bytes = data
            .get(name_start..name_start + name_len)
            .ok_or("central directory entry name out of bounds")?;
        let name = core::str::from_utf8(name_bytes)
            .map_err(|_| "central directory entry name is not valid UTF-8")?;

        entries.push(ZipEntry {
            name: String::from(name),
            method,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });

        offset = name_start + name_len + extra_len + comment_len;
    }

    Ok(entries)
}

/// Reads (and, for STORED entries only, "decompresses" -- i.e. copies
/// as-is) one entry's data. DEFLATE entries are rejected with a clear
/// error rather than silently returning garbage.
pub fn read_entry(data: &[u8], entry: &ZipEntry) -> Result<Vec<u8>, &'static str> {
    let offset = entry.local_header_offset as usize;
    if read_u32(data, offset) != Some(LOCAL_FILE_SIGNATURE) {
        return Err("malformed local file header");
    }
    let name_len = read_u16(data, offset + 26).ok_or("truncated local file header")? as usize;
    let extra_len = read_u16(data, offset + 28).ok_or("truncated local file header")? as usize;
    let data_start = offset + 30 + name_len + extra_len;
    let data_end = data_start + entry.compressed_size as usize;
    let raw = data
        .get(data_start..data_end)
        .ok_or("local file entry data out of bounds")?;

    match entry.method {
        METHOD_STORED => Ok(raw.to_vec()),
        _ => Err("only STORED (uncompressed) entries are supported -- no DEFLATE decoder yet"),
    }
}

/// Convenience: lists entries, finds `AndroidManifest.xml`, and reads it.
pub fn find_manifest(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let entries = list_entries(data)?;
    let manifest = entries
        .iter()
        .find(|e| e.name == "AndroidManifest.xml")
        .ok_or("no AndroidManifest.xml in this archive")?;
    read_entry(data, manifest)
}

const RES_XML_TYPE: u16 = 0x0003;

pub struct AxmlHeader {
    pub chunk_type: u16,
    pub header_size: u16,
    pub chunk_size: u32,
}

/// Validates and reads the Android Binary XML `ResChunk_header` (type,
/// headerSize, size) that every AXML file -- `AndroidManifest.xml` inside
/// a real APK is always this format, never plain-text XML -- starts with.
/// Does not decode the string pool or element tree that follows.
pub fn parse_axml_header(data: &[u8]) -> Result<AxmlHeader, &'static str> {
    let chunk_type = read_u16(data, 0).ok_or("AXML data too short for a chunk header")?;
    let header_size = read_u16(data, 2).ok_or("AXML data too short for a chunk header")?;
    let chunk_size = read_u32(data, 4).ok_or("AXML data too short for a chunk header")?;

    if chunk_type != RES_XML_TYPE {
        return Err("not Android Binary XML (unexpected chunk type)");
    }
    if header_size != 8 {
        return Err("not Android Binary XML (unexpected header size)");
    }

    Ok(AxmlHeader {
        chunk_type,
        header_size,
        chunk_size,
    })
}
