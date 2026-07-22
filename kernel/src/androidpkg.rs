//! The "installed Android apps" registry: metadata only. Installing an
//! `.apk` here means real work -- reading the real ZIP container,
//! DEFLATE-decompressing the real `AndroidManifest.xml` entry, and
//! decoding its real AXML string pool/element tree to recover the actual
//! `package` name and (best-effort) `android:label` (see `crate::apk`'s
//! doc comment for exactly what that parser does and doesn't resolve).
//! What it deliberately does *not* do is make the app's Java/Dalvik code
//! executable: there is no ART/Dalvik bytecode interpreter and no Android
//! framework (ActivityManager, Binder, SurfaceFlinger, ...) anywhere in
//! this kernel, so [`launch`] always fails, honestly, with that reason --
//! never silently, never by pretending. This exists so a real double
//! -click "Install" flow in the File Manager has somewhere real to
//! register into, and so the launcher has something true to say when the
//! user tries to open one.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const REGISTRY_DIR: &str = "/.android";

pub struct InstalledApk {
    pub package: String,
    pub label: String,
}

fn registry_path(package: &str) -> String {
    format!("{REGISTRY_DIR}/{package}.apkinfo")
}

/// Parses `apk_bytes` as a real APK (ZIP + DEFLATE + AXML manifest) and, on
/// success, records its package/label in the registry -- a real RAMFS
/// write, not a UI-only flag. Returns the parsed metadata for the caller
/// to show as status/notification text.
pub fn install(apk_bytes: &[u8]) -> Result<InstalledApk, String> {
    let manifest = crate::apk::manifest_from_apk(apk_bytes).map_err(String::from)?;
    let label = manifest.label.clone().unwrap_or_else(|| {
        format!(
            "{} (no plain-text label -- likely a @string/... resource reference)",
            manifest.package
        )
    });

    let mut root = crate::fs::root().lock();
    root.mkdir(REGISTRY_DIR);
    let contents = format!("{}\n{}\n", manifest.package, label);
    root.write(&registry_path(&manifest.package), contents.as_bytes());

    Ok(InstalledApk {
        package: manifest.package,
        label,
    })
}

pub fn is_installed(package: &str) -> bool {
    crate::fs::root().lock().exists(&registry_path(package))
}

pub fn uninstall(package: &str) -> bool {
    crate::fs::root().lock().remove(&registry_path(package))
}

/// Lists every "installed" Android package -- real entries read back from
/// the registry files `install` wrote, not a static demo list.
pub fn installed() -> Vec<InstalledApk> {
    let root = crate::fs::root().lock();
    let mut out = Vec::new();
    for path in root.list() {
        if !path.starts_with(REGISTRY_DIR) || !path.ends_with(".apkinfo") {
            continue;
        }
        let Some(data) = root.read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(data);
        let mut lines = text.lines();
        if let (Some(package), Some(label)) = (lines.next(), lines.next()) {
            out.push(InstalledApk {
                package: String::from(package),
                label: String::from(label),
            });
        }
    }
    out
}

/// "Launching" an installed Android package. Always fails -- there is no
/// Android runtime in this kernel to run its bytecode, and this function
/// exists specifically so that fact is reported plainly instead of the
/// launcher silently doing nothing or pretending to open the app.
pub fn launch(_package: &str) -> Result<(), String> {
    Err(String::from(
        "moon OS has no Android runtime (no Dalvik/ART interpreter, no Android framework) -- this app's code cannot run here",
    ))
}
