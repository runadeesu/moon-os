//! X25519 (RFC 7748): Diffie-Hellman key exchange over Curve25519, the key
//! exchange TLS 1.3's `x25519` group uses. Field arithmetic is done with
//! the standard radix-2^51 representation (five 51-bit limbs -- 255 bits
//! total, matching the field's size) so every multiply's partial products
//! fit in a `u128` without needing a general bignum type. The Montgomery
//! ladder itself (`scalarmult`) is transcribed directly from RFC 7748
//! section 5's reference algorithm.
//!
//! `self_test` checks real key-exchange values cross-verified against
//! PyCryptodome's X25519 implementation during development (both a single
//! scalar-times-basepoint computation and a full two-party Diffie-Hellman
//! agreement, confirming both directions produce the same shared secret).

const MASK51: u64 = (1u64 << 51) - 1;
const P_LIMBS: [u64; 5] = [
    0x7ffffffffffed,
    0x7ffffffffffff,
    0x7ffffffffffff,
    0x7ffffffffffff,
    0x7ffffffffffff,
];
const A24: u64 = 121665;

type Fe = [u64; 5];

fn carry_reduce(mut t: [u128; 5]) -> Fe {
    let mut c = (t[0] >> 51) as u64;
    let f0 = (t[0] as u64) & MASK51;
    t[1] += u128::from(c);
    c = (t[1] >> 51) as u64;
    let f1 = (t[1] as u64) & MASK51;
    t[2] += u128::from(c);
    c = (t[2] >> 51) as u64;
    let f2 = (t[2] as u64) & MASK51;
    t[3] += u128::from(c);
    c = (t[3] >> 51) as u64;
    let f3 = (t[3] as u64) & MASK51;
    t[4] += u128::from(c);
    c = (t[4] >> 51) as u64;
    let f4 = (t[4] as u64) & MASK51;

    let mut f0 = f0 + c * 19;
    let c2 = f0 >> 51;
    f0 &= MASK51;
    let f1 = f1 + c2;
    [f0, f1, f2, f3, f4]
}

fn fe_add(a: &Fe, b: &Fe) -> Fe {
    carry_reduce([
        u128::from(a[0]) + u128::from(b[0]),
        u128::from(a[1]) + u128::from(b[1]),
        u128::from(a[2]) + u128::from(b[2]),
        u128::from(a[3]) + u128::from(b[3]),
        u128::from(a[4]) + u128::from(b[4]),
    ])
}

/// `a - b mod p`: adds `2*p`'s limbs first so every limb subtraction stays
/// non-negative in unsigned arithmetic (2p ≡ 0 mod p, so this doesn't
/// change the result).
fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    carry_reduce([
        u128::from(a[0]) + 2 * u128::from(P_LIMBS[0]) - u128::from(b[0]),
        u128::from(a[1]) + 2 * u128::from(P_LIMBS[1]) - u128::from(b[1]),
        u128::from(a[2]) + 2 * u128::from(P_LIMBS[2]) - u128::from(b[2]),
        u128::from(a[3]) + 2 * u128::from(P_LIMBS[3]) - u128::from(b[3]),
        u128::from(a[4]) + 2 * u128::from(P_LIMBS[4]) - u128::from(b[4]),
    ])
}

/// `a * b mod p`, via the standard "group by limb-index-sum, fold the
/// overflow past position 4 back in times 19" trick -- 2^255 == 19 (mod p),
/// and limb position 5 is exactly 2^(51*5) = 2^255, so any product landing
/// there wraps around multiplied by 19. Same derivation as `poly1305`'s
/// `mul_mod`, just radix 2^51/mod 19 instead of radix 2^26/mod 5.
fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let a0 = u128::from(a[0]);
    let a1 = u128::from(a[1]);
    let a2 = u128::from(a[2]);
    let a3 = u128::from(a[3]);
    let a4 = u128::from(a[4]);
    let b0 = u128::from(b[0]);
    let b1 = u128::from(b[1]);
    let b2 = u128::from(b[2]);
    let b3 = u128::from(b[3]);
    let b4 = u128::from(b[4]);

    let b1_19 = b1 * 19;
    let b2_19 = b2 * 19;
    let b3_19 = b3 * 19;
    let b4_19 = b4 * 19;

    let t0 = a0 * b0 + a1 * b4_19 + a2 * b3_19 + a3 * b2_19 + a4 * b1_19;
    let t1 = a0 * b1 + a1 * b0 + a2 * b4_19 + a3 * b3_19 + a4 * b2_19;
    let t2 = a0 * b2 + a1 * b1 + a2 * b0 + a3 * b4_19 + a4 * b3_19;
    let t3 = a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * b4_19;
    let t4 = a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;

    carry_reduce([t0, t1, t2, t3, t4])
}

fn fe_square(a: &Fe) -> Fe {
    fe_mul(a, a)
}

fn fe_mul_small(a: &Fe, k: u64) -> Fe {
    let k = u128::from(k);
    carry_reduce([
        u128::from(a[0]) * k,
        u128::from(a[1]) * k,
        u128::from(a[2]) * k,
        u128::from(a[3]) * k,
        u128::from(a[4]) * k,
    ])
}

/// `a^(p-2) mod p` via Fermat's little theorem -- a plain square-and-
/// multiply over the fixed 255-bit exponent `p-2`, not the fastest
/// addition-chain implementations use, but a much smaller surface for a
/// mistake, and this only runs once per handshake.
fn fe_invert(a: &Fe) -> Fe {
    const EXPONENT: [u8; 32] = [
        0xeb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x7f,
    ];
    let mut result: Fe = [1, 0, 0, 0, 0];
    for bit_index in (0..255).rev() {
        result = fe_square(&result);
        let byte = EXPONENT[bit_index / 8];
        if (byte >> (bit_index % 8)) & 1 == 1 {
            result = fe_mul(&result, a);
        }
    }
    result
}

fn fe_from_bytes(bytes: &[u8; 32]) -> Fe {
    let mut b = *bytes;
    b[31] &= 0x7f; // RFC 7748 decodeUCoordinate: mask the top bit for a 255-bit field.

    let load64 = |byte_offset: usize| -> u64 {
        let mut v = 0u64;
        for i in 0..8 {
            let idx = byte_offset + i;
            if idx < 32 {
                v |= u64::from(b[idx]) << (8 * i);
            }
        }
        v
    };

    [
        load64(0) & MASK51,
        (load64(6) >> 3) & MASK51,
        (load64(12) >> 6) & MASK51,
        (load64(19) >> 1) & MASK51,
        (load64(25) >> 4) & MASK51,
    ]
}

/// Fully reduces `f` below `p` (it may be anywhere in `[0, a small multiple
/// of p)` after the loose per-operation reduction above) and serializes it
/// as 32 little-endian bytes.
fn fe_to_bytes(f: &Fe) -> [u8; 32] {
    let mut f = *f;
    // A couple of conditional-subtract passes are always enough here: every
    // fe_* op above keeps values within a small constant factor of p.
    for _ in 0..2 {
        let mut borrow: i128 = 0;
        let mut sub = [0i128; 5];
        for i in 0..5 {
            let d = i128::from(f[i]) - i128::from(P_LIMBS[i]) - borrow;
            if d < 0 {
                sub[i] = d + (1i128 << 51);
                borrow = 1;
            } else {
                sub[i] = d;
                borrow = 0;
            }
        }
        if borrow == 0 {
            for i in 0..5 {
                f[i] = sub[i] as u64;
            }
        }
    }

    // Reverse of `fe_from_bytes`'s extraction: OR each limb, shifted into
    // place, into a byte scratch buffer wide enough that no limb's 51 bits
    // (shifted by up to 7 bits to align within its start byte) can run past
    // the end -- 40 bytes covers bit 254 (limb 4 starts at byte 25) with
    // plenty of headroom, avoiding the earlier version's bug of silently
    // truncating limbs 2-4 through a too-narrow u128 shift.
    let mut acc = [0u8; 40];
    const LIMB_BIT_OFFSET: [usize; 5] = [0, 51, 102, 153, 204];
    for (i, &bit_off) in LIMB_BIT_OFFSET.iter().enumerate() {
        let byte_off = bit_off / 8;
        let shift = bit_off % 8;
        let val_bytes = (u128::from(f[i]) << shift).to_le_bytes();
        for (j, vb) in val_bytes.iter().enumerate() {
            if byte_off + j < 40 {
                acc[byte_off + j] |= vb;
            }
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&acc[..32]);
    out
}

fn cswap(swap: bool, x2: &mut Fe, x3: &mut Fe) {
    if swap {
        core::mem::swap(x2, x3);
    }
}

/// The Montgomery ladder from RFC 7748 section 5, x-coordinate-only scalar
/// multiplication. `scalar` is clamped per the RFC (bit 254 set, bits 0-2
/// and 255 cleared) before use.
pub fn scalarmult(scalar: &[u8; 32], u_bytes: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let x1 = fe_from_bytes(u_bytes);
    let mut x2: Fe = [1, 0, 0, 0, 0];
    let mut z2: Fe = [0, 0, 0, 0, 0];
    let mut x3 = x1;
    let mut z3: Fe = [1, 0, 0, 0, 0];
    let mut swap = false;

    for t in (0..255).rev() {
        let bit = ((k[t / 8] >> (t % 8)) & 1) == 1;
        swap ^= bit;
        cswap(swap, &mut x2, &mut x3);
        cswap(swap, &mut z2, &mut z3);
        swap = bit;

        let a = fe_add(&x2, &z2);
        let aa = fe_square(&a);
        let b = fe_sub(&x2, &z2);
        let bb = fe_square(&b);
        let e = fe_sub(&aa, &bb);
        let c = fe_add(&x3, &z3);
        let d = fe_sub(&x3, &z3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        let x3_new = fe_square(&fe_add(&da, &cb));
        let z3_diff = fe_square(&fe_sub(&da, &cb));
        let z3_new = fe_mul(&x1, &z3_diff);
        let x2_new = fe_mul(&aa, &bb);
        let z2_new = fe_mul(&e, &fe_add(&aa, &fe_mul_small(&e, A24)));

        x2 = x2_new;
        z2 = z2_new;
        x3 = x3_new;
        z3 = z3_new;
    }
    cswap(swap, &mut x2, &mut x3);
    cswap(swap, &mut z2, &mut z3);

    let z2_inv = fe_invert(&z2);
    fe_to_bytes(&fe_mul(&x2, &z2_inv))
}

pub const BASEPOINT: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// Derives the public key for private scalar `scalar` (basepoint 9).
pub fn public_key(scalar: &[u8; 32]) -> [u8; 32] {
    scalarmult(scalar, &BASEPOINT)
}

/// Checks a scalar-times-basepoint computation and a full two-party
/// Diffie-Hellman exchange (both directions must produce the same shared
/// secret) against values cross-verified with PyCryptodome during
/// development.
pub fn self_test() {
    let seed1: [u8; 32] = super::testhex::hex_bytes(
        "b34ad5ce4acb47ae20b2c0d3b3654d1969e11bd43f6bc670abb4e4ba54f9cc59",
    );
    let pub1: [u8; 32] = super::testhex::hex_bytes(
        "4d04f6a67744ba8991dc5d7f54cf0bfedc39bbcbe55d61fcd6c224c1d63e624d",
    );
    let seed2: [u8; 32] = super::testhex::hex_bytes(
        "77836e6df859710f826d92724bcd3a1291d3aa6498bf75fab70a853065f5b5d6",
    );
    let pub2: [u8; 32] = super::testhex::hex_bytes(
        "340f89d4505e32484cd771e946357e2ce23f2898cd701ec610ac8b4d54b60a06",
    );
    let shared: [u8; 32] = super::testhex::hex_bytes(
        "f275c94cd30a77d0cf9561b0a5e59c9c3f542d252a7b4f280930dab10aab0d35",
    );

    assert_eq!(
        public_key(&seed1),
        pub1,
        "x25519: seed1 -> public key mismatch"
    );
    assert_eq!(
        public_key(&seed2),
        pub2,
        "x25519: seed2 -> public key mismatch"
    );

    let shared_a = scalarmult(&seed1, &pub2);
    let shared_b = scalarmult(&seed2, &pub1);
    assert_eq!(
        shared_a, shared_b,
        "x25519: DH agreement mismatch between directions"
    );
    assert_eq!(shared_a, shared, "x25519: DH shared secret mismatch");
}
