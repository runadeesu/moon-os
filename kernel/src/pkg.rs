//! moon OS's native app package format ("`.mapp`"): a small container
//! wrapping a static ELF64 executable with a name and version, stored as
//! ordinary files in the VFS. Not a real archive format (no compression,
//! no multi-file bundles, no signing yet) -- just enough structure that the
//! package manager, File Manager, and Moon Store can all list/install/run
//! packages by reading this one shared format instead of each inventing
//! their own.
//!
//! Layout (all integers little-endian):
//! ```text
//! offset  size  field
//! 0       4     magic: b"MOAP"
//! 4       2     format_version (currently 1)
//! 6       2     name_len
//! 8       2     version_len
//! 10      4     elf_len
//! 14      ..    name bytes (UTF-8)
//! ..      ..    version bytes (UTF-8)
//! ..      ..    ELF64 executable bytes
//! ```

use alloc::string::String;
use alloc::vec::Vec;

const MAGIC: [u8; 4] = *b"MOAP";
const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 14;

pub struct Package<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub elf: &'a [u8],
}

/// Wraps `elf` with a `name`/`version` header, producing the on-disk bytes
/// of a `.mapp` package.
pub fn build(name: &str, version: &str, elf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + name.len() + version.len() + elf.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(version.len() as u16).to_le_bytes());
    out.extend_from_slice(&(elf.len() as u32).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(version.as_bytes());
    out.extend_from_slice(elf);
    out
}

/// Parses a `.mapp` package's bytes, borrowing straight from `data` (no
/// copying) -- valid for as long as `data` (the RAMFS-backed file) is.
pub fn parse(data: &[u8]) -> Result<Package<'_>, &'static str> {
    if data.len() < HEADER_LEN || data[0..4] != MAGIC {
        return Err("not a .mapp package (bad magic)");
    }
    let format_version = u16::from_le_bytes([data[4], data[5]]);
    if format_version != FORMAT_VERSION {
        return Err("unsupported .mapp format version");
    }
    let name_len = u16::from_le_bytes([data[6], data[7]]) as usize;
    let version_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    let elf_len = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;

    let name_start = HEADER_LEN;
    let version_start = name_start + name_len;
    let elf_start = version_start + version_len;
    let elf_end = elf_start + elf_len;
    if elf_end > data.len() {
        return Err(".mapp package truncated");
    }

    let name = core::str::from_utf8(&data[name_start..version_start])
        .map_err(|_| "package name is not valid UTF-8")?;
    let version = core::str::from_utf8(&data[version_start..elf_start])
        .map_err(|_| "package version is not valid UTF-8")?;
    let elf = &data[elf_start..elf_end];

    Ok(Package { name, version, elf })
}

/// A package as reported by [`crate::pkg::installed`] -- owns its own
/// copies of name/version so callers don't need to hold the RAMFS lock (or
/// the parsed borrow into it) around.
pub struct InstalledPackage {
    pub file_name: String,
    pub name: String,
    pub version: String,
}

/// Lists every `.mapp` package currently in the RAMFS root, parsing just
/// enough of each to report its name/version (skipping anything that
/// doesn't parse rather than failing the whole listing).
pub fn installed() -> Vec<InstalledPackage> {
    let root = crate::fs::root().lock();
    let mut out = Vec::new();
    for file_name in root.list() {
        if !file_name.ends_with(".mapp") {
            continue;
        }
        let Some(data) = root.read(file_name) else {
            continue;
        };
        if let Ok(pkg) = parse(data) {
            out.push(InstalledPackage {
                file_name: String::from(file_name),
                name: String::from(pkg.name),
                version: String::from(pkg.version),
            });
        }
    }
    out
}

/// Loads and spawns the package at RAMFS path `file_name` as a new ring-3
/// process. Returns the package's display name on success (for logging/UI
/// feedback), or an error string on failure (bad package, bad ELF, or out
/// of memory).
pub fn run(file_name: &str) -> Result<String, String> {
    let elf_bytes = {
        let root = crate::fs::root().lock();
        let data = root
            .read(file_name)
            .ok_or_else(|| alloc::format!("no such package: {}", file_name))?;
        let pkg = parse(data).map_err(String::from)?;
        Vec::from(pkg.elf)
    };

    crate::process::spawn_from_elf(&elf_bytes).map_err(String::from)?;

    let root = crate::fs::root().lock();
    let data = root.read(file_name).unwrap();
    let pkg = parse(data).map_err(String::from)?;
    Ok(String::from(pkg.name))
}
