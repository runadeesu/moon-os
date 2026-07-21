//! An in-memory filesystem: a flat map from absolute path to file contents,
//! plus an explicit set of directory paths (auto-extended from both
//! `mkdir` and any nested file path -- `write("/apps/init.mapp", ..)`
//! implies `/apps` is a directory even without an explicit `mkdir`).
//! Enough for an initrd-style root before any real block-backed filesystem
//! (FAT32/ext2/moonFS) is mounted on top of the AHCI driver.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub struct RamFs {
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
}

impl RamFs {
    pub const fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
        }
    }

    pub fn write(&mut self, path: &str, data: &[u8]) {
        self.ensure_parents(path);
        self.files.insert(String::from(path), data.to_vec());
    }

    pub fn read(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    pub fn remove(&mut self, path: &str) -> bool {
        self.files.remove(path).is_some()
    }

    /// Renames/moves a file. Fails (returns `false`, no-op) if `from`
    /// doesn't exist or `to` already does.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        if from == to || self.files.contains_key(to) {
            return false;
        }
        let Some(data) = self.files.remove(from) else {
            return false;
        };
        self.ensure_parents(to);
        self.files.insert(String::from(to), data);
        true
    }

    pub fn copy(&mut self, from: &str, to: &str) -> bool {
        if self.files.contains_key(to) {
            return false;
        }
        let Some(data) = self.files.get(from).cloned() else {
            return false;
        };
        self.ensure_parents(to);
        self.files.insert(String::from(to), data);
        true
    }

    /// Every file path in the whole tree, flat -- used where a real
    /// directory structure doesn't matter (e.g. `pkg::installed` scanning
    /// for `.mapp` files anywhere).
    pub fn list(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn mkdir(&mut self, path: &str) {
        self.ensure_parents(path);
        self.dirs.insert(String::from(path));
    }

    /// Removes an explicitly-created empty directory entry. Directories
    /// implied by a nested file path disappear on their own once that file
    /// (and any siblings) are gone -- there's no separate bookkeeping to
    /// clean up for those.
    pub fn rmdir(&mut self, path: &str) -> bool {
        self.dirs.remove(path)
    }

    pub fn is_dir(&self, path: &str) -> bool {
        if path == "/" {
            return true;
        }
        self.dirs.contains(path)
            || self
                .files
                .keys()
                .any(|f| f.starts_with(&format!("{path}/")))
    }

    pub fn exists(&self, path: &str) -> bool {
        path == "/" || self.files.contains_key(path) || self.is_dir(path)
    }

    pub fn file_size(&self, path: &str) -> Option<usize> {
        self.files.get(path).map(Vec::len)
    }

    fn ensure_parents(&mut self, path: &str) {
        let Some(idx) = path.rfind('/') else {
            return;
        };
        if idx == 0 {
            return; // parent is "/", always exists implicitly
        }
        let parent = String::from(&path[..idx]);
        if self.dirs.insert(parent.clone()) {
            self.ensure_parents(&parent);
        }
    }

    /// Direct children of `parent` (files and directories, both explicit
    /// and implied by nested file paths) as `(full_path, is_dir)` pairs,
    /// sorted by path.
    pub fn list_dir(&self, parent: &str) -> Vec<(String, bool)> {
        let prefix = if parent == "/" {
            String::from("/")
        } else {
            format!("{parent}/")
        };

        let mut seen = BTreeSet::new();
        let mut out = Vec::new();

        for path in self.files.keys() {
            let Some(rest) = path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let name = rest.split('/').next().unwrap_or(rest);
            let is_dir = name.len() != rest.len();
            let full = format!("{prefix}{name}");
            if seen.insert(full.clone()) {
                out.push((full, is_dir));
            }
        }
        for path in &self.dirs {
            let Some(rest) = path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            if rest.is_empty() || rest.contains('/') {
                continue;
            }
            let full = format!("{prefix}{rest}");
            if seen.insert(full.clone()) {
                out.push((full, true));
            }
        }

        out.sort();
        out
    }
}

impl Default for RamFs {
    fn default() -> Self {
        Self::new()
    }
}
