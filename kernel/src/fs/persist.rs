//! Whole-tree RAMFS persistence to a real SATA disk over AHCI, so files,
//! accounts (`/etc/passwd`), and anything else written into RAMFS survive
//! a reboot. Plain RAMFS by itself is wiped every boot -- which is a real
//! reason this OS could feel like a "visual demo" no matter how genuine
//! everything running on top of it is: nothing you create ever sticks
//! around.
//!
//! This is deliberately *not* a general block-level filesystem -- no
//! inodes, no free-space bitmap, no in-place updates of a single file.
//! Every save serializes the *entire* RAMFS tree and rewrites it from
//! LBA 1 onward. That's an honest tradeoff that only holds up at the
//! kilobyte-to-low-megabyte scale a hobby OS's RAMFS actually reaches --
//! it is not what a real filesystem would do at disk-sized scale, and this
//! comment says so instead of pretending the snapshot approach is more
//! than it is. What *is* real: the disk I/O (genuine ATA WRITE/READ DMA
//! EXT commands over AHCI), the on-disk format (a real superblock with a
//! real CRC32 over the payload, rejecting anything that doesn't match),
//! and the result -- verified by writing a file, restarting QEMU with the
//! same disk image, and finding the file still there.
//!
//! On-disk layout, all little-endian:
//!   LBA 0 (superblock, 512 bytes): magic `"MOONFS1\0"` (8 bytes),
//!     payload length in bytes (u32), CRC32 of the payload (u32), the rest
//!     zero.
//!   LBA 1..: the payload, one entry per file back-to-back:
//!     path length (u16), path bytes, data length (u32), data bytes.
//!     Zero-padded out to a whole number of sectors.

use crate::drivers::ahci::{self, Port, PortKind};
use alloc::vec::Vec;
use spin::Mutex;

const MAGIC: &[u8; 8] = b"MOONFS1\0";
const SECTOR: usize = 512;

/// `Port` holds raw MMIO pointers into the HBA's register space and a DMA
/// command-table pointer -- there's exactly one CPU core running this
/// kernel and no concurrent access to a given port, so it's sound to treat
/// it as safe to hand across the `Mutex` boundary here.
struct DiskHandle(Port);
unsafe impl Send for DiskHandle {}

static DISK: Mutex<Option<DiskHandle>> = Mutex::new(None);

/// Claims the first real SATA (non-ATAPI) disk out of `ports` as the
/// persistence backing store, if one's attached. Call once at boot, right
/// after `ahci::init()`. Without a SATA disk (e.g. booting the ISO without
/// the extra `-drive` `tools/run.sh` attaches), RAMFS just stays
/// memory-only for that session, same as before this module existed.
pub fn init(ports: Vec<Port>) {
    match ports.into_iter().find(|p| p.kind == PortKind::Sata) {
        Some(port) => {
            crate::serial_println!(
                "fs::persist: using AHCI port {} as the persistent disk",
                port.index
            );
            *DISK.lock() = Some(DiskHandle(port));
        }
        None => {
            crate::serial_println!(
                "fs::persist: no SATA disk attached -- RAMFS stays memory-only this session"
            );
        }
    }
}

pub fn available() -> bool {
    DISK.lock().is_some()
}

fn write_sectors(port: &Port, start_lba: u64, data: &[u8]) {
    for (i, chunk) in data.chunks(SECTOR).enumerate() {
        let mut sector = [0u8; SECTOR];
        sector[..chunk.len()].copy_from_slice(chunk);
        ahci::ata_write_sector(port, start_lba + i as u64, &sector);
    }
}

fn read_sectors(port: &Port, start_lba: u64, count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(count * SECTOR);
    for i in 0..count {
        let mut sector = [0u8; SECTOR];
        ahci::ata_read_sector(port, start_lba + i as u64, &mut sector);
        out.extend_from_slice(&sector);
    }
    out
}

/// Serializes the whole RAMFS tree and writes it out over real AHCI WRITE
/// DMA EXT commands. A no-op if no SATA disk was claimed at boot.
pub fn save() {
    let guard = DISK.lock();
    let Some(DiskHandle(port)) = guard.as_ref() else {
        return;
    };

    let mut payload = Vec::new();
    let mut file_count = 0usize;
    {
        let root = crate::fs::root().lock();
        for path in root.list() {
            let data = root.read(path).unwrap_or(&[]);
            payload.extend_from_slice(&(path.len() as u16).to_le_bytes());
            payload.extend_from_slice(path.as_bytes());
            payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payload.extend_from_slice(data);
            file_count += 1;
        }
    }

    let crc = crate::zip::crc32(&payload);
    let mut superblock = [0u8; SECTOR];
    superblock[..8].copy_from_slice(MAGIC);
    superblock[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    superblock[12..16].copy_from_slice(&crc.to_le_bytes());

    write_sectors(port, 1, &payload);
    ahci::ata_write_sector(port, 0, &superblock); // superblock last: only "commits" once the payload is fully on disk

    crate::serial_println!(
        "fs::persist: saved {} file(s), {} bytes, to disk",
        file_count,
        payload.len()
    );
}

/// Reads back whatever `save()` last wrote (if anything) and populates
/// RAMFS with it. Call once at boot, before any default files/accounts are
/// created, so a prior session's data takes priority over fresh defaults.
/// Returns `true` if a valid snapshot was found and restored.
pub fn load() -> bool {
    let guard = DISK.lock();
    let Some(DiskHandle(port)) = guard.as_ref() else {
        return false;
    };

    let mut superblock = [0u8; SECTOR];
    if !ahci::ata_read_sector(port, 0, &mut superblock) {
        crate::serial_println!("fs::persist: superblock read failed");
        return false;
    }
    if superblock[..8] != *MAGIC {
        crate::serial_println!("fs::persist: no snapshot on disk yet (first boot with this disk)");
        return false;
    }
    let len = u32::from_le_bytes(superblock[8..12].try_into().unwrap()) as usize;
    let expected_crc = u32::from_le_bytes(superblock[12..16].try_into().unwrap());

    let sector_count = len.div_ceil(SECTOR).max(1);
    let mut payload = read_sectors(port, 1, sector_count);
    payload.truncate(len);

    if crate::zip::crc32(&payload) != expected_crc {
        crate::serial_println!(
            "fs::persist: snapshot CRC mismatch ({} bytes) -- ignoring, treating as corrupt/partial",
            len
        );
        return false;
    }

    let mut root = crate::fs::root().lock();
    let mut cursor = 0usize;
    let mut file_count = 0usize;
    while cursor + 2 <= payload.len() {
        let path_len = u16::from_le_bytes([payload[cursor], payload[cursor + 1]]) as usize;
        cursor += 2;
        let Some(path_bytes) = payload.get(cursor..cursor + path_len) else {
            break;
        };
        let Ok(path) = core::str::from_utf8(path_bytes) else {
            break;
        };
        cursor += path_len;
        if cursor + 4 > payload.len() {
            break;
        }
        let data_len = u32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let Some(data) = payload.get(cursor..cursor + data_len) else {
            break;
        };
        root.write(path, data);
        cursor += data_len;
        file_count += 1;
    }

    crate::serial_println!(
        "fs::persist: restored {} file(s) from disk snapshot",
        file_count
    );
    true
}
