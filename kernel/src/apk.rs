//! APK reading, as far as "metadata only" goes: a real ZIP container
//! reader (an APK *is* a ZIP archive) that handles both STORED and
//! DEFLATE-compressed entries (the latter via `crate::inflate`, since
//! real-world APKs almost always compress `AndroidManifest.xml`), and a
//! real Android Binary XML (AXML) decoder -- string pool (UTF-8 and
//! UTF-16 variants) plus the namespace/element/attribute node tree --
//! good enough to pull the manifest's `package` name and, when the
//! `<application>` element's `android:label` is a plain string rather
//! than a `@string/...` resource reference, its display label too.
//!
//! What this deliberately does *not* do, and it matters: resolve
//! resource-referenced values (an `android:label` pointing into
//! `resources.arsc`, which is how most published apps actually declare
//! their name -- decoding that resource table is its own substantial
//! format on top of this one), decode anything beyond `package`/`label`
//! (permissions, activities, intent filters), or anything resembling
//! actually running Dalvik/ART bytecode. That last piece is not "write
//! more parsing code" -- it means integrating a real Android runtime,
//! which is its own project. See `crate::androidpkg` for how this
//! metadata is used: registering an entry in the launcher, honestly
//! refusing to "run" it.

use alloc::string::String;
use alloc::vec::Vec;

const EOCD_SIGNATURE: u32 = 0x0605_4B50;
const CENTRAL_DIR_SIGNATURE: u32 = 0x0201_4B50;
const LOCAL_FILE_SIGNATURE: u32 = 0x0403_4B50;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

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

/// Reads and decompresses one entry's data -- STORED entries are copied
/// as-is, DEFLATE entries go through `crate::inflate`'s real RFC 1951
/// decoder. Anything else (the handful of rarer ZIP methods real tools
/// occasionally use) is rejected with a clear error rather than silently
/// returning garbage.
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
        METHOD_DEFLATE => {
            crate::inflate::inflate(raw).ok_or("DEFLATE decompression failed (corrupt stream)")
        }
        _ => Err("unsupported compression method (only STORED/DEFLATE)"),
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

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;
const UTF8_FLAG: u32 = 1 << 8;
const TYPE_STRING: u8 = 0x03;
const NO_ENTRY: u32 = 0xFFFF_FFFF;

/// Reads the 1-or-2-byte variable length prefix the AXML string pool uses
/// for both UTF-8 strings' char-length/byte-length fields: a plain byte if
/// `<= 0x7F`, or two bytes (high bit of the first set, low 15 bits split
/// across both) for longer strings. Returns `(value, bytes_consumed)`.
fn decode_var_len(data: &[u8], pos: usize) -> Option<(usize, usize)> {
    let b0 = *data.get(pos)?;
    if b0 & 0x80 != 0 {
        let b1 = *data.get(pos + 1)?;
        Some((((b0 as usize & 0x7F) << 8) | b1 as usize, 2))
    } else {
        Some((b0 as usize, 1))
    }
}

fn decode_utf8_pool_string(data: &[u8], start: usize) -> Result<String, &'static str> {
    let (_char_len, n1) = decode_var_len(data, start).ok_or("truncated UTF-8 pool string")?;
    let (byte_len, n2) =
        decode_var_len(data, start + n1).ok_or("truncated UTF-8 pool string")?;
    let bytes_start = start + n1 + n2;
    let bytes = data
        .get(bytes_start..bytes_start + byte_len)
        .ok_or("UTF-8 pool string out of bounds")?;
    core::str::from_utf8(bytes)
        .map(String::from)
        .map_err(|_| "pool string is not valid UTF-8")
}

/// The less common variant (older `aapt` output, and any tool that didn't
/// opt into `UTF8_FLAG`): each string is length-prefixed (same 1-or-2-unit
/// scheme, but in 16-bit units) UTF-16LE, NUL-terminated.
fn decode_utf16_pool_string(data: &[u8], start: usize) -> Result<String, &'static str> {
    let u0 = read_u16(data, start).ok_or("truncated UTF-16 pool string")?;
    let (char_len, n_units) = if u0 & 0x8000 != 0 {
        let u1 = read_u16(data, start + 2).ok_or("truncated UTF-16 pool string")?;
        (
            ((u32::from(u0 & 0x7FFF) << 16) | u32::from(u1)) as usize,
            2,
        )
    } else {
        (u0 as usize, 1)
    };
    let units_start = start + n_units * 2;
    let mut units = Vec::with_capacity(char_len);
    for i in 0..char_len {
        units.push(read_u16(data, units_start + i * 2).ok_or("UTF-16 pool string out of bounds")?);
    }
    String::from_utf16(&units).map_err(|_| "pool string is not valid UTF-16")
}

/// Decodes a `ResStringPool` chunk starting at `data[0]` (i.e. `data`
/// already sliced to the chunk's own start). Returns the decoded strings
/// and the chunk's total size (so the caller can skip past it to whatever
/// comes next).
fn decode_string_pool(data: &[u8]) -> Result<(Vec<String>, usize), &'static str> {
    let chunk_type = read_u16(data, 0).ok_or("truncated string pool header")?;
    if chunk_type != RES_STRING_POOL_TYPE {
        return Err("expected a string pool chunk after the AXML header");
    }
    let chunk_size = read_u32(data, 4).ok_or("truncated string pool header")? as usize;
    let string_count = read_u32(data, 8).ok_or("truncated string pool header")? as usize;
    let flags = read_u32(data, 16).ok_or("truncated string pool header")?;
    let strings_start = read_u32(data, 20).ok_or("truncated string pool header")? as usize;
    let utf8 = flags & UTF8_FLAG != 0;

    const OFFSETS_START: usize = 28;
    let mut strings = Vec::with_capacity(string_count);
    for i in 0..string_count {
        let off = read_u32(data, OFFSETS_START + i * 4).ok_or("truncated string pool offsets")?
            as usize;
        let s_start = strings_start + off;
        let s = if utf8 {
            decode_utf8_pool_string(data, s_start)?
        } else {
            decode_utf16_pool_string(data, s_start)?
        };
        strings.push(s);
    }
    Ok((strings, chunk_size))
}

/// A manifest's real, decoded package name and (best-effort) display
/// label -- everything `crate::androidpkg`'s install flow needs.
pub struct Manifest {
    pub package: String,
    /// `None` when `<application>` has no `android:label`, or declares it
    /// as a `@string/...` (or other) resource reference this parser
    /// doesn't resolve -- see this module's doc comment: that needs a real
    /// `resources.arsc` decoder, a separate, unimplemented format.
    pub label: Option<String>,
}

/// An attribute's value is either a literal string (`rawValue` is a valid
/// string-pool index) or, for `TYPE_STRING`-typed attributes, the same
/// thing encoded in `data` instead -- anything else (a resource
/// reference, an int, a bool, ...) isn't a value this parser can turn
/// into readable text.
fn resolve_attr_value(strings: &[String], raw_value: u32, data_type: u8, data: u32) -> Option<String> {
    if raw_value != NO_ENTRY {
        return strings.get(raw_value as usize).cloned();
    }
    if data_type == TYPE_STRING {
        return strings.get(data as usize).cloned();
    }
    None
}

/// Decodes the real string pool + namespace/element/attribute node tree
/// of an AXML `AndroidManifest.xml` -- as opposed to `parse_axml_header`,
/// which only validates the outer chunk header -- to pull out the real
/// `package` attribute and, best-effort, `<application>`'s
/// `android:label`. Tolerates (skips over) namespace chunks and an
/// optional resource-map chunk, the same real structure `aapt`/`aapt2`
/// actually emit.
pub fn parse_manifest(data: &[u8]) -> Result<Manifest, &'static str> {
    parse_axml_header(data)?;

    let (strings, pool_chunk_size) =
        decode_string_pool(data.get(8..).ok_or("AXML data too short for a string pool")?)?;
    let mut offset = 8 + pool_chunk_size;

    if read_u16(data, offset) == Some(RES_XML_RESOURCE_MAP_TYPE) {
        let size = read_u32(data, offset + 4).ok_or("truncated resource map chunk")? as usize;
        offset += size;
    }

    let mut package: Option<String> = None;
    let mut label: Option<String> = None;

    while offset + 16 <= data.len() {
        let chunk_type = read_u16(data, offset).ok_or("truncated XML node header")?;
        let chunk_size = read_u32(data, offset + 4).ok_or("truncated XML node header")? as usize;
        if chunk_size < 16 || offset + chunk_size > data.len() {
            return Err("malformed XML node chunk size");
        }

        if chunk_type == RES_XML_START_ELEMENT_TYPE {
            let body = offset + 16;
            let name_idx = read_u32(data, body + 4).ok_or("truncated StartElement")?;
            let attr_start =
                read_u16(data, body + 8).ok_or("truncated StartElement")? as usize;
            let attr_size =
                read_u16(data, body + 10).ok_or("truncated StartElement")? as usize;
            let attr_count =
                read_u16(data, body + 12).ok_or("truncated StartElement")? as usize;
            let name = strings
                .get(name_idx as usize)
                .map(String::as_str)
                .unwrap_or("");

            let attrs_base = body + attr_start;
            for i in 0..attr_count {
                let a = attrs_base + i * attr_size;
                let attr_name_idx = read_u32(data, a + 4).ok_or("truncated attribute")?;
                let raw_value = read_u32(data, a + 8).ok_or("truncated attribute")?;
                let data_type = *data.get(a + 15).ok_or("truncated attribute")?;
                let typed_data = read_u32(data, a + 16).ok_or("truncated attribute")?;
                let attr_name = strings
                    .get(attr_name_idx as usize)
                    .map(String::as_str)
                    .unwrap_or("");

                if name == "manifest" && attr_name == "package" {
                    package = resolve_attr_value(&strings, raw_value, data_type, typed_data);
                } else if name == "application" && attr_name == "label" {
                    label = resolve_attr_value(&strings, raw_value, data_type, typed_data);
                }
            }
        }

        offset += chunk_size;
    }

    package
        .map(|package| Manifest { package, label })
        .ok_or("no <manifest package=\"...\"> attribute found")
}

/// Convenience: `find_manifest` + `parse_manifest` in one call -- what
/// `crate::androidpkg`'s install flow uses.
pub fn manifest_from_apk(data: &[u8]) -> Result<Manifest, &'static str> {
    let manifest_bytes = find_manifest(data)?;
    parse_manifest(&manifest_bytes)
}
