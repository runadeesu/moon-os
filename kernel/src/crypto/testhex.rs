//! A tiny hex-string parser used only by the crypto self-tests below to
//! write RFC/FIPS known-answer test vectors as readable hex literals
//! instead of Rust byte-array literals.

use alloc::vec::Vec;

fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("bad hex digit"),
    }
}

pub fn hex_bytes<const N: usize>(s: &str) -> [u8; N] {
    let b = s.as_bytes();
    assert_eq!(
        b.len(),
        N * 2,
        "hex literal has the wrong length for {N} bytes"
    );
    let mut out = [0u8; N];
    for i in 0..N {
        out[i] = (nibble(b[i * 2]) << 4) | nibble(b[i * 2 + 1]);
    }
    out
}

pub fn hex_vec(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    assert_eq!(b.len() % 2, 0, "hex literal has an odd number of digits");
    let mut out = Vec::with_capacity(b.len() / 2);
    for chunk in b.chunks_exact(2) {
        out.push((nibble(chunk[0]) << 4) | nibble(chunk[1]));
    }
    out
}
