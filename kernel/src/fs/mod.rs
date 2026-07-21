//! Filesystem layer. Just a single mounted RAMFS for now; a real VFS (mount
//! points, multiple backing filesystems, a moonFS/FAT32 driver on top of
//! `drivers::ahci`) is future work once there's more than one filesystem to
//! dispatch between.

pub mod ramfs;

use ramfs::RamFs;
use spin::Mutex;

static ROOT: Mutex<RamFs> = Mutex::new(RamFs::new());

pub fn root() -> &'static Mutex<RamFs> {
    &ROOT
}
