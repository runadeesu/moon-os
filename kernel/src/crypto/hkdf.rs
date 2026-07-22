//! HKDF (RFC 5869) over SHA-256, plus TLS 1.3's `HKDF-Expand-Label`
//! (RFC 8446 section 7.1) that `tls`'s key schedule is built entirely out
//! of.

use super::hmac::hmac_sha256;
use alloc::vec::Vec;

pub fn extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    hmac_sha256(salt, ikm)
}

pub fn expand(prk: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let mut okm = Vec::with_capacity(length);
    let mut t: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while okm.len() < length {
        let mut input = Vec::with_capacity(t.len() + info.len() + 1);
        input.extend_from_slice(&t);
        input.extend_from_slice(info);
        input.push(counter);
        t = hmac_sha256(prk, &input).to_vec();
        okm.extend_from_slice(&t);
        counter += 1;
    }
    okm.truncate(length);
    okm
}

/// RFC 8446 7.1: `HkdfLabel` is `length(u16) || label<1..255> || context<0..255>`,
/// where `label` itself is `"tls13 " || label_str` length-prefixed by one
/// byte, and `context` is the transcript hash (or empty), also
/// length-prefixed by one byte.
pub fn expand_label(secret: &[u8], label: &str, context: &[u8], length: usize) -> Vec<u8> {
    let full_label = alloc::format!("tls13 {label}");
    let mut hkdf_label = Vec::with_capacity(2 + 1 + full_label.len() + 1 + context.len());
    hkdf_label.extend_from_slice(&(length as u16).to_be_bytes());
    hkdf_label.push(full_label.len() as u8);
    hkdf_label.extend_from_slice(full_label.as_bytes());
    hkdf_label.push(context.len() as u8);
    hkdf_label.extend_from_slice(context);
    expand(secret, &hkdf_label, length)
}

/// RFC 5869 appendix A.1, test case 1 (SHA-256).
pub fn self_test() {
    let ikm = [0x0bu8; 22];
    let salt: [u8; 13] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
    ];
    let info = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
    let prk = extract(&salt, &ikm);
    let expected_prk =
        super::sha256::hex32("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");
    assert_eq!(prk, expected_prk, "hkdf: RFC 5869 test case 1 PRK mismatch");

    let okm = expand(&prk, &info, 42);
    let expected_okm: [u8; 42] = [
        0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0, 0x36, 0x2f,
        0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0, 0x2d, 0x56, 0xec, 0xc4,
        0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87, 0x18, 0x58, 0x65,
    ];
    assert_eq!(
        okm.as_slice(),
        &expected_okm[..],
        "hkdf: RFC 5869 test case 1 OKM mismatch"
    );
}
