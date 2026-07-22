//! AEAD_CHACHA20_POLY1305 (RFC 8439 section 2.8): combines `chacha20` and
//! `poly1305` into an authenticated cipher -- the only AEAD TLS 1.3 needs
//! from this kernel (cipher suite `TLS_CHACHA20_POLY1305_SHA256`).

use super::{chacha20, poly1305};
use alloc::vec::Vec;

fn pad16_len(len: usize) -> usize {
    (16 - (len % 16)) % 16
}

fn poly1305_key(key: &[u8; 32], nonce: &[u8; 12]) -> [u8; 32] {
    let block0 = chacha20::block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);
    otk
}

fn mac_input(aad: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(
        aad.len() + pad16_len(aad.len()) + ciphertext.len() + pad16_len(ciphertext.len()) + 16,
    );
    data.extend_from_slice(aad);
    data.resize(data.len() + pad16_len(aad.len()), 0);
    data.extend_from_slice(ciphertext);
    data.resize(data.len() + pad16_len(ciphertext.len()), 0);
    data.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    data.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    data
}

/// Encrypts `plaintext` in place (returned as ciphertext) and returns the
/// 16-byte authentication tag over `aad || ciphertext`.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> (Vec<u8>, [u8; 16]) {
    let otk = poly1305_key(key, nonce);
    let mut ciphertext = plaintext.to_vec();
    chacha20::apply_keystream(key, 1, nonce, &mut ciphertext);
    let tag = poly1305::mac(&otk, &mac_input(aad, &ciphertext));
    (ciphertext, tag)
}

/// Verifies the tag and decrypts, returning `None` on authentication
/// failure -- callers must never use the plaintext if this returns `None`
/// (a real, checked failure, not best-effort).
pub fn open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; 16],
) -> Option<Vec<u8>> {
    let otk = poly1305_key(key, nonce);
    let expected_tag = poly1305::mac(&otk, &mac_input(aad, ciphertext));
    if !constant_time_eq(&expected_tag, tag) {
        return None;
    }
    let mut plaintext = ciphertext.to_vec();
    chacha20::apply_keystream(key, 1, nonce, &mut plaintext);
    Some(plaintext)
}

/// Tag comparison shouldn't short-circuit on the first differing byte --
/// doing so leaks timing information about how much of a forged tag an
/// attacker got right, letting them recover a valid tag one byte at a time.
fn constant_time_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// RFC 8439 section 2.8.2's full AEAD known-answer test.
pub fn self_test() {
    let key: [u8; 32] = super::testhex::hex_bytes(
        "808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f",
    );
    let nonce: [u8; 12] = super::testhex::hex_bytes("070000004041424344454647");
    let aad: [u8; 12] = super::testhex::hex_bytes("50515253c0c1c2c3c4c5c6c7");
    let plaintext =
        b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

    let (ciphertext, tag) = seal(&key, &nonce, &aad, plaintext);
    let expected_ct: Vec<u8> = super::testhex::hex_vec(
        "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116",
    );
    let expected_tag: [u8; 16] = super::testhex::hex_bytes("1ae10b594f09e26a7e902ecbd0600691");

    assert_eq!(
        ciphertext, expected_ct,
        "aead: RFC 8439 2.8.2 ciphertext mismatch"
    );
    assert_eq!(tag, expected_tag, "aead: RFC 8439 2.8.2 tag mismatch");

    let opened = open(&key, &nonce, &aad, &ciphertext, &tag);
    assert_eq!(
        opened.as_deref(),
        Some(plaintext.as_ref()),
        "aead: open() did not recover the original plaintext"
    );

    let mut tampered_tag = tag;
    tampered_tag[0] ^= 1;
    assert!(
        open(&key, &nonce, &aad, &ciphertext, &tampered_tag).is_none(),
        "aead: open() must reject a tampered tag"
    );
}
