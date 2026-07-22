//! Filesystem layer. A single mounted RAMFS as the live working tree, with
//! `persist` snapshotting it to a real SATA disk over `drivers::ahci` so it
//! survives a reboot. A real VFS (mount points, multiple backing
//! filesystems, a moonFS/FAT32 driver that supports in-place random-access
//! updates instead of whole-tree snapshots) is future work.

pub mod persist;
pub mod ramfs;

use ramfs::RamFs;
use spin::Mutex;

static ROOT: Mutex<RamFs> = Mutex::new(RamFs::new());

pub fn root() -> &'static Mutex<RamFs> {
    &ROOT
}
