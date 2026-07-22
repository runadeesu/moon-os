//! Cryptography, written from the relevant specs rather than pulled in as
//! a dependency -- this kernel has no dependency on anything outside
//! `core`/`alloc`. Everything here backs `net::tls`'s TLS 1.3 client, one
//! fixed cipher suite (`TLS_CHACHA20_POLY1305_SHA256` with X25519 key
//! exchange): SHA-256, HMAC, HKDF for the key schedule, ChaCha20-Poly1305
//! for record encryption, X25519 for the key exchange itself.
//!
//! `self_test()` runs every primitive's known-answer test (real FIPS/RFC
//! test vectors, several cross-checked against PyCryptodome during
//! development) and panics on any mismatch -- crypto that's subtly wrong
//! is far worse than crypto that's honestly absent, so nothing here ships
//! unverified.

pub mod aead;
pub mod chacha20;
pub mod hkdf;
pub mod hmac;
pub mod poly1305;
pub mod sha256;
pub mod testhex;
pub mod x25519;

pub fn self_test() {
    sha256::self_test();
    hmac::self_test();
    hkdf::self_test();
    chacha20::self_test();
    poly1305::self_test();
    aead::self_test();
    x25519::self_test();
    crate::serial_println!(
        "crypto: all self-tests passed (sha256/hmac/hkdf/chacha20/poly1305/aead/x25519)"
    );
}
