//! ChaCha20 (RFC 8439 section 2.3-2.4): the IETF variant with a 32-bit
//! block counter and a 96-bit nonce, as TLS 1.3 uses it.

const CONSTANTS: [u32; 4] = [0x61707865, 0x3320646e, 0x79622d32, 0x6b206574];

#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// Produces one 64-byte keystream block for `key`/`counter`/`nonce`.
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        state[4 + i] =
            u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }

    let initial = state;
    for _ in 0..10 {
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = state[i].wrapping_add(initial[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// XORs `data` with the keystream starting at `counter`, in place --
/// encryption and decryption are the same operation for a stream cipher.
pub fn apply_keystream(key: &[u8; 32], counter: u32, nonce: &[u8; 12], data: &mut [u8]) {
    for (i, chunk) in data.chunks_mut(64).enumerate() {
        let ks = block(key, counter.wrapping_add(i as u32), nonce);
        for (byte, k) in chunk.iter_mut().zip(ks.iter()) {
            *byte ^= k;
        }
    }
}

/// Checks two raw keystream blocks (counter 0 and 1) against known-good
/// output cross-verified against both a from-scratch Python reference and
/// PyCryptodome's ChaCha20 for the same key/nonce/counter.
pub fn self_test() {
    let key: [u8; 32] = core::array::from_fn(|i| i as u8);
    let nonce: [u8; 12] = [
        0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
    ];

    let block0 = block(&key, 0, &nonce);
    assert_eq!(
        block0,
        super::testhex::hex_bytes::<64>("8adc91fd9ff4f0f51b0fad50ff15d637e40efda206cc52c783a74200503c1582cd9833367d0a54d57d3c9e998f490ee69ca34c1ff9e939a75584c52d690a35d4"),
        "chacha20: counter=0 keystream block mismatch"
    );

    let block1 = block(&key, 1, &nonce);
    assert_eq!(
        block1,
        super::testhex::hex_bytes::<64>("10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"),
        "chacha20: counter=1 keystream block mismatch"
    );
}
