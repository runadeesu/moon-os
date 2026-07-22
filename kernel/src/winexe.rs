//! Launches a `.exe` (PE32+) file straight from RAMFS as a new ring-3
//! process -- the real code path both the File Manager's double-click and
//! its right-click "Run" action go through, and the same one
//! `process::spawn_from_pe`/`pe::load` (see `kernel/src/pe.rs`) already
//! back for the bundled Win32-ish test binary.
//!
//! The honest limit, worth repeating here since this is the user-facing
//! entry point: `pe::load` only resolves a small, fixed subset of
//! `KERNEL32.DLL` imports by synthesizing thunks into moon OS's own
//! `int 0x80` ABI. A `.exe` built for or ported to moon OS runs for real.
//! A real, unmodified Windows program -- anything using the actual Win32
//! API surface, let alone DirectX -- imports functions this loader has
//! never heard of, and `run` reports that plainly (`friendly_error`
//! recognizes `pe.rs`'s "unsupported import"/"ordinal" errors and turns
//! them into that explanation) instead of a generic "launch failed" or,
//! worse, pretending it worked. There is no Win32 API layer, no DirectX,
//! no GPU 3D driver anywhere in this kernel -- that gap is not something
//! this function's error message can close.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Runs the `.exe` at RAMFS path `path`. Distinguishes "this needs real
/// Win32 APIs moon OS doesn't implement" from other failures (bad/corrupt
/// PE file, out of memory) so the caller can show the true reason.
pub fn run(path: &str) -> Result<String, String> {
    let result = run_inner(path);
    use crate::gui::notifications::{push, Category, Kind};
    match &result {
        Ok(name) => push(Kind::Success, Category::Packages, format!("launched {name}")),
        Err(err) => push(Kind::Error, Category::Packages, err.clone()),
    }
    result
}

fn run_inner(path: &str) -> Result<String, String> {
    let bytes: Vec<u8> = {
        let root = crate::fs::root().lock();
        let data = root
            .read(path)
            .ok_or_else(|| format!("no such file: {path}"))?;
        data.to_vec()
    };

    crate::process::spawn_from_pe(&bytes)
        .map(|_entry| String::from(path))
        .map_err(|err| friendly_error(path, err))
}

/// `pe::load`'s import resolver returns these two specific messages for
/// "this import isn't in our tiny supported subset" -- everything else
/// (bad DOS/PE/NT signature, truncated file, out of memory) is a real,
/// distinct failure that gets passed through as-is rather than
/// misdiagnosed as a Win32-API gap.
fn friendly_error(path: &str, err: &'static str) -> String {
    if err.starts_with("unsupported import") || err.starts_with("ordinal") {
        format!(
            "{path}: this is a real Windows program that calls Win32 APIs moon OS doesn't implement (no Win32/DirectX compatibility layer) -- it cannot run here"
        )
    } else {
        format!("{path}: {err}")
    }
}
