//! An in-memory filesystem: a flat map from absolute path to file contents.
//! Enough for an initrd-style root before any real block-backed filesystem
//! (FAT32/ext2/moonFS) is mounted on top of the AHCI driver.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub struct RamFs {
    files: BTreeMap<String, Vec<u8>>,
}

impl RamFs {
    pub const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    pub fn write(&mut self, path: &str, data: &[u8]) {
        self.files.insert(String::from(path), data.to_vec());
    }

    pub fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    pub fn remove(&mut self, path: &str) -> bool {
        self.files.remove(path).is_some()
    }

    pub fn list(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

impl Default for RamFs {
    fn default() -> Self {
        Self::new()
    }
}
