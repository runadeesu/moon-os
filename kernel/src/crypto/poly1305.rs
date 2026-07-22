//! Poly1305 (RFC 8439 section 2.5): a one-time-key MAC over the integers
//! mod 2^130-5. Implemented with the standard radix-2^26 representation
//! (the accumulator and `r` split into five 26-bit limbs) so every
//! intermediate product fits comfortably in a `u64` -- no bignum type
//! needed, at the cost of needing careful carry propagation between limbs,
//! which is exactly the part most likely to have an off-by-one. `self_test`
//! checks the real RFC 8439 section 2.5.2 known-answer vector.

const MASK26: u64 = (1 << 26) - 1;

fn read_u64_at(buf: &[u8; 17], byte_offset: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..8 {
        let idx = byte_offset + i;
        if idx < 17 {
            v |= u64::from(buf[idx]) << (8 * i);
        }
    }
    v
}

/// Splits a (zero-padded) 17-byte little-endian buffer into five 26-bit
/// limbs covering bits 0..129 -- enough range for a 16-byte message block
/// plus the appended high bit RFC 8439 requires.
fn limbs_from_buf17(buf: &[u8; 17]) -> [u64; 5] {
    let w0 = read_u64_at(buf, 0);
    let w1 = read_u64_at(buf, 3);
    let w2 = read_u64_at(buf, 6);
    let w3 = read_u64_at(buf, 9);
    let w4 = read_u64_at(buf, 13);
    [
        w0 & MASK26,
        (w1 >> 2) & MASK26,
        (w2 >> 4) & MASK26,
        (w3 >> 6) & MASK26,
        w4 & MASK26,
    ]
}

fn clamp_and_split_r(r_bytes: &[u8]) -> [u64; 5] {
    let mut clamped = [0u8; 16];
    clamped.copy_from_slice(&r_bytes[..16]);
    clamped[3] &= 15;
    clamped[7] &= 15;
    clamped[11] &= 15;
    clamped[15] &= 15;
    clamped[4] &= 252;
    clamped[8] &= 252;
    clamped[12] &= 252;

    let mut buf17 = [0u8; 17];
    buf17[..16].copy_from_slice(&clamped);
    limbs_from_buf17(&buf17)
}

/// One accumulator update: `h = (h + block) * r mod (2^130 - 5)`, with the
/// standard "multiply by 5" trick for folding each limb pair's overflow
/// past position 4 back in (since 2^130 == 5 mod (2^130-5), any carry out
/// of the top limb gets multiplied by 5 instead of dropped).
fn mul_mod(h: &mut [u64; 5], r: &[u64; 5]) {
    let r0 = r[0];
    let r1 = r[1];
    let r2 = r[2];
    let r3 = r[3];
    let r4 = r[4];
    let r1_5 = r1 * 5;
    let r2_5 = r2 * 5;
    let r3_5 = r3 * 5;
    let r4_5 = r4 * 5;

    let h0 = h[0];
    let h1 = h[1];
    let h2 = h[2];
    let h3 = h[3];
    let h4 = h[4];

    let t0 = h0 * r0 + h1 * r4_5 + h2 * r3_5 + h3 * r2_5 + h4 * r1_5;
    let mut t1 = h0 * r1 + h1 * r0 + h2 * r4_5 + h3 * r3_5 + h4 * r2_5;
    let mut t2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * r4_5 + h4 * r3_5;
    let mut t3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * r4_5;
    let mut t4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

    let mut c = t0 >> 26;
    h[0] = t0 & MASK26;
    t1 += c;
    c = t1 >> 26;
    h[1] = t1 & MASK26;
    t2 += c;
    c = t2 >> 26;
    h[2] = t2 & MASK26;
    t3 += c;
    c = t3 >> 26;
    h[3] = t3 & MASK26;
    t4 += c;
    c = t4 >> 26;
    h[4] = t4 & MASK26;
    h[0] += c * 5;
    c = h[0] >> 26;
    h[0] &= MASK26;
    h[1] += c;
}

const P_LOW128: u128 = u128::MAX - 4;
const P_EXTRA: u8 = 3;

/// Computes the Poly1305 tag for `message` under one-time key `key`
/// (32 bytes: `r` (bytes 0..16, clamped) concatenated with `s` (16..32)).
pub fn mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    let r = clamp_and_split_r(&key[0..16]);
    let mut h = [0u64; 5];

    let mut offset = 0;
    while offset < message.len() {
        let end = (offset + 16).min(message.len());
        let chunk = &message[offset..end];
        let mut buf = [0u8; 17];
        buf[..chunk.len()].copy_from_slice(chunk);
        buf[chunk.len()] = 1;
        let limbs = limbs_from_buf17(&buf);
        for i in 0..5 {
            h[i] += limbs[i];
        }
        mul_mod(&mut h, &r);
        offset = end;
    }

    // One more full carry pass so every limb is strictly < 2^26 before the
    // exact reduction below.
    let mut c;
    c = h[0] >> 26;
    h[0] &= MASK26;
    h[1] += c;
    c = h[1] >> 26;
    h[1] &= MASK26;
    h[2] += c;
    c = h[2] >> 26;
    h[2] &= MASK26;
    h[3] += c;
    c = h[3] >> 26;
    h[3] &= MASK26;
    h[4] += c;
    c = h[4] >> 26;
    h[4] &= MASK26;
    h[0] += c * 5;
    c = h[0] >> 26;
    h[0] &= MASK26;
    h[1] += c;

    // Recombine into an exact (low128, extra-2-bits) value -- h fits in at
    // most 130 bits at this point, one bit more than `u128` holds.
    let low128: u128 = u128::from(h[0])
        | (u128::from(h[1]) << 26)
        | (u128::from(h[2]) << 52)
        | (u128::from(h[3]) << 78)
        | ((u128::from(h[4]) & 0xFF_FFFF) << 104);
    let mut extra = ((h[4] >> 24) & 0b11) as u8;
    let mut low128 = low128;

    // Subtract p = 2^130-5 at most once or twice if h landed >= p (lazy
    // reduction only guarantees h is *close* to canonical, not exactly).
    while extra > P_EXTRA || (extra == P_EXTRA && low128 >= P_LOW128) {
        if low128 >= P_LOW128 {
            low128 -= P_LOW128;
            extra -= P_EXTRA;
        } else {
            low128 = low128.wrapping_sub(P_LOW128);
            extra = extra - P_EXTRA - 1;
        }
    }

    let s = u128::from_le_bytes(key[16..32].try_into().unwrap());
    let tag = low128.wrapping_add(s);
    tag.to_le_bytes()
}

/// RFC 8439 section 2.5.2's known-answer test: a fixed one-time key over
/// `"Cryptographic Forum Research Group"`.
pub fn self_test() {
    let key32: [u8; 32] = super::testhex::hex_bytes(
        "85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b",
    );
    let tag = mac(&key32, b"Cryptographic Forum Research Group");
    let expected: [u8; 16] = super::testhex::hex_bytes("a8061dc1305136c6c22b8baf0c0127a9");
    assert_eq!(
        tag, expected,
        "poly1305: RFC 8439 2.5.2 test vector mismatch"
    );
}
