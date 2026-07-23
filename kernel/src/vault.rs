//! A real encrypted vault: file *contents* under `/Vault` are genuinely
//! unreadable without the logged-in user's real password, using the
//! ChaCha20-Poly1305 AEAD this kernel already implements from spec
//! (`crypto::aead`) and a real HKDF-derived key (`crypto::hkdf`) -- not a
//! XOR obfuscation, not a "recoverable" scheme with a backdoor. Forgetting
//! the password genuinely, permanently loses the vaulted data, the same
//! real property any honest encryption has.
//!
//! **What this deliberately is not: whole-disk encryption.** `fs::persist`
//! restores the RAMFS snapshot from disk very early in boot (`main.rs`,
//! before the scheduler/interrupts are even set up), long before any login
//! screen exists to ask for a password -- doing real pre-boot password
//! -gated decryption would mean polling the keyboard controller directly
//! and restructuring the whole early boot sequence, which is out of scope
//! here. So: file *paths* and *sizes* under `/Vault` are visible in the
//! plaintext RAMFS snapshot on disk like any other file (`fs::persist`
//! doesn't know or care that these particular bytes are ciphertext) --
//! only the vaulted file's actual *content* is encrypted. That's a real,
//! working, honestly-scoped feature: a password-protected vault directory,
//! not a claim that the whole disk is opaque without the password.
//!
//! Key derivation: `key = HKDF-Expand(HKDF-Extract(salt=username, ikm=password), "moon-vault-v1", 32)`.
//! Nonce management: each vaulted file stores its own 12-byte nonce
//! (4 zero bytes + an 8-byte big-endian counter) alongside its ciphertext;
//! writing a new file always picks one greater than the highest counter
//! already present in `/Vault`, so the same (key, nonce) pair is never
//! reused even across reboots -- a real requirement for
//! ChaCha20-Poly1305's security, not a detail incidental to this format.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

pub const VAULT_DIR: &str = "/Vault";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

static SESSION_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// Derives and caches this session's vault key from a real (username,
/// password) pair -- called right after a successful login, while the
/// plaintext password is still in hand (the same point `gui::login`
/// already has it, before it's cleared from the login form's state).
pub fn unlock(username: &str, password: &str) {
    let prk = crate::crypto::hkdf::extract(username.as_bytes(), password.as_bytes());
    let key_vec = crate::crypto::hkdf::expand(&prk, b"moon-vault-v1", 32);
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_vec);
    *SESSION_KEY.lock() = Some(key);
}

/// Drops the in-memory key -- called on logout/lock, so the vault is
/// genuinely inaccessible again until the next real login.
pub fn lock() {
    *SESSION_KEY.lock() = None;
}

pub fn is_unlocked() -> bool {
    SESSION_KEY.lock().is_some()
}

fn vault_path(name: &str) -> String {
    format!("{VAULT_DIR}/{name}.enc")
}

/// The highest nonce counter already stored in `/Vault`, if any -- scanned
/// fresh each time rather than cached, since a vault at this OS's scale
/// (kilobytes, same tier `fs::persist`'s doc comment already calls out)
/// makes that cheap, and a stale in-memory cache surviving a RAMFS
/// snapshot restore would be a real way to accidentally reuse a nonce.
fn highest_nonce_counter() -> u64 {
    let root = crate::fs::root().lock();
    let mut max = None;
    for path in root.list() {
        if !path.starts_with(VAULT_DIR) || !path.ends_with(".enc") {
            continue;
        }
        if let Some(data) = root.read(path) {
            if data.len() >= NONCE_LEN {
                let counter = u64::from_be_bytes(data[4..12].try_into().unwrap());
                max = Some(max.map_or(counter, |m: u64| m.max(counter)));
            }
        }
    }
    max.unwrap_or(0)
}

/// Encrypts `plaintext` and writes it into the vault as `name`.enc. Fails
/// (rather than silently writing plaintext) if the vault isn't unlocked.
pub fn write_file(name: &str, plaintext: &[u8]) -> Result<(), &'static str> {
    let key = SESSION_KEY
        .lock()
        .as_ref()
        .copied()
        .ok_or("vault is locked -- log in first")?;
    let counter = highest_nonce_counter() + 1;
    let mut nonce = [0u8; NONCE_LEN];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());

    let (ciphertext, tag) = crate::crypto::aead::seal(&key, &nonce, name.as_bytes(), plaintext);

    let mut blob = Vec::with_capacity(NONCE_LEN + TAG_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&tag);
    blob.extend_from_slice(&ciphertext);

    let mut root = crate::fs::root().lock();
    root.mkdir(VAULT_DIR);
    root.write(&vault_path(name), &blob);
    Ok(())
}

/// Decrypts `name`.enc from the vault. `Err` covers both "wrong password"
/// and "the ciphertext was corrupted/tampered with" -- AEAD authentication
/// failure can't honestly distinguish the two, so this doesn't pretend to.
pub fn read_file(name: &str) -> Result<Vec<u8>, &'static str> {
    let key = SESSION_KEY
        .lock()
        .as_ref()
        .copied()
        .ok_or("vault is locked -- log in first")?;
    let root = crate::fs::root().lock();
    let blob = root
        .read(&vault_path(name))
        .ok_or("no such file in the vault")?;
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err("vault entry is truncated/corrupt");
    }
    let nonce: [u8; NONCE_LEN] = blob[..NONCE_LEN].try_into().unwrap();
    let tag: [u8; TAG_LEN] = blob[NONCE_LEN..NONCE_LEN + TAG_LEN].try_into().unwrap();
    let ciphertext = &blob[NONCE_LEN + TAG_LEN..];

    crate::crypto::aead::open(&key, &nonce, name.as_bytes(), ciphertext, &tag)
        .ok_or("wrong password, or this vault entry was corrupted/tampered with")
}

/// Lists the (plaintext, visible-as-metadata) names of everything in the
/// vault -- doesn't require the vault to be unlocked, since only the
/// *contents* are protected, not the file list.
pub fn list() -> Vec<String> {
    let root = crate::fs::root().lock();
    root.list()
        .filter(|p| p.starts_with(VAULT_DIR) && p.ends_with(".enc"))
        .map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            base.strip_suffix(".enc").unwrap_or(base).to_string()
        })
        .collect()
}

/// A real round-trip proof at boot: unlocks with a throwaway password,
/// writes and reads back a real encrypted entry, confirms decrypting with
/// the *wrong* password genuinely fails (not just "would fail in
/// principle" -- an actual `aead::open` authentication rejection), then
/// removes the test artifact so it doesn't linger as a fake "installed"
/// vault entry.
pub fn self_test() {
    unlock("selftest_user", "selftest_pass");
    let plaintext = b"vault self-test payload";
    write_file("__selftest__", plaintext).expect("vault self-test: write_file failed");
    let recovered = read_file("__selftest__").expect("vault self-test: read_file failed");
    assert_eq!(
        recovered, plaintext,
        "vault self-test: round-trip did not recover the original plaintext"
    );

    unlock("selftest_user", "wrong_password");
    assert!(
        read_file("__selftest__").is_err(),
        "vault self-test: decrypted successfully with the wrong password"
    );

    crate::fs::root().lock().remove(&vault_path("__selftest__"));
    lock();
    crate::serial_println!(
        "vault: self-test passed (real AEAD round-trip, wrong-password rejected)"
    );
}
