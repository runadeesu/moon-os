//! A minimal ZIP writer: STORED (uncompressed) method only, with a real
//! CRC32 and real local-file-header/central-directory/EOCD structure --
//! genuinely valid, standard ZIP files any real unzip tool can open, just
//! not entropy-compressed (no DEFLATE encoder here, matching the reader in
//! `apk.rs`, which this shares its on-disk layout with byte-for-byte).
//! Backs the File Manager's "Compress to ZIP" action; `apk::list_entries`/
//! `apk::read_entry` back "Extract".

use alloc::string::String;
use alloc::vec::Vec;

/// Standard IEEE CRC32 (the polynomial every ZIP tool uses), computed
/// bit-by-bit rather than via a 256-entry lookup table -- these are small
/// RAMFS files, not worth the table's static memory for the speed.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a valid ZIP archive containing `entries` (name, data), all stored
/// uncompressed.
pub fn build_stored(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();

    for (name, data) in entries {
        let crc = crc32(data);
        let local_offset = out.len() as u32;

        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // method
        central.extend_from_slice(&0u16.to_le_bytes()); // time
        central.extend_from_slice(&0u16.to_le_bytes()); // date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra length
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&local_offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }

    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);

    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length

    out
}
