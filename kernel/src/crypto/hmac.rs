//! HMAC-SHA256 (RFC 2104 / FIPS 198-1), built directly on `sha256::hash`.

use super::sha256;

const BLOCK_SIZE: usize = 64;

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block_key = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let hashed = sha256::hash(key);
        block_key[..32].copy_from_slice(&hashed);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= block_key[i];
        opad[i] ^= block_key[i];
    }

    let mut inner_input = alloc::vec::Vec::with_capacity(BLOCK_SIZE + message.len());
    inner_input.extend_from_slice(&ipad);
    inner_input.extend_from_slice(message);
    let inner_hash = sha256::hash(&inner_input);

    let mut outer_input = alloc::vec::Vec::with_capacity(BLOCK_SIZE + 32);
    outer_input.extend_from_slice(&opad);
    outer_input.extend_from_slice(&inner_hash);
    sha256::hash(&outer_input)
}

/// Checks against RFC 4231 test case 1 (an all-`0x0b` 20-byte key over
/// `"Hi There"`).
pub fn self_test() {
    let key = [0x0bu8; 20];
    let mac = hmac_sha256(&key, b"Hi There");
    assert_eq!(
        mac,
        sha256::hex32("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
        "hmac-sha256: RFC 4231 test case 1 mismatch"
    );
}
