//! A real RFC 1951 (DEFLATE) decompressor -- "raw deflate," no zlib/gzip
//! wrapper. Needed because real-world ZIP/APK files almost always compress
//! their entries with DEFLATE, and `apk.rs`'s reader could previously only
//! read STORED (uncompressed) entries, which meant it could not read the
//! `AndroidManifest.xml` out of a genuine, unmodified APK. Modeled on the
//! classic `puff.c` reference algorithm (fixed/dynamic Huffman blocks,
//! canonical-code construction, the standard length/distance extra-bit
//! tables) -- a small, well-specified, from-scratch implementation, not a
//! port of zlib's C.
//!
//! Deliberately not attempted: a DEFLATE *encoder* (this project's ZIP
//! writer in `zip.rs` still only writes STORED, which is a fully valid
//! choice for any ZIP reader, ours included) and the gzip/zlib container
//! formats (two-byte zlib header + Adler32, or the ten-byte gzip header +
//! CRC32 trailer) -- callers that need those wrap this with their own
//! header/trailer handling. Just the DEFLATE bitstream itself.

use alloc::vec;
use alloc::vec::Vec;

const MAX_BITS: usize = 15;

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u32,
    bit_cnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    /// Pulls `need` bits (`need` <= 16), least-significant-bit first -- the
    /// packing DEFLATE uses for every field except Huffman codes themselves
    /// (RFC 1951 section 3.1.1).
    fn bits(&mut self, need: u32) -> Option<u32> {
        while self.bit_cnt < need {
            let byte = *self.data.get(self.byte_pos)?;
            self.byte_pos += 1;
            self.bit_buf |= u32::from(byte) << self.bit_cnt;
            self.bit_cnt += 8;
        }
        let val = self.bit_buf & ((1u32 << need) - 1);
        self.bit_buf >>= need;
        self.bit_cnt -= need;
        Some(val)
    }

    /// Discards any partial byte so the next read starts byte-aligned --
    /// needed before a stored (uncompressed) block's length header.
    fn align_to_byte(&mut self) {
        self.bit_buf = 0;
        self.bit_cnt = 0;
    }

    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.data.get(self.byte_pos..self.byte_pos + n)?;
        self.byte_pos += n;
        Some(slice)
    }
}

/// A canonical Huffman code table: `count[len]` codes have length `len`,
/// and `symbol` lists the symbols in canonical order -- exactly the
/// `puff.c` "construct" representation, which turns decode into cheap
/// running-offset math instead of building an explicit tree.
struct Huffman {
    count: [u16; MAX_BITS + 1],
    symbol: Vec<u16>,
}

fn construct(lengths: &[u8]) -> Huffman {
    let mut count = [0u16; MAX_BITS + 1];
    for &len in lengths {
        count[len as usize] += 1;
    }
    count[0] = 0;

    let mut offsets = [0u16; MAX_BITS + 2];
    for len in 1..=MAX_BITS {
        offsets[len + 1] = offsets[len] + count[len];
    }

    let mut symbol = vec![0u16; lengths.len()];
    for (sym, &len) in lengths.iter().enumerate() {
        if len != 0 {
            symbol[offsets[len as usize] as usize] = sym as u16;
            offsets[len as usize] += 1;
        }
    }

    Huffman { count, symbol }
}

/// Decodes one symbol using `h`, reading one bit at a time and building the
/// code most-significant-bit first (the one place DEFLATE's bit order
/// flips relative to every other field) via the standard canonical-decode
/// running index.
fn decode(reader: &mut BitReader, h: &Huffman) -> Option<u16> {
    let mut code: i32 = 0;
    let mut first: i32 = 0;
    let mut index: i32 = 0;
    for len in 1..=MAX_BITS {
        code |= reader.bits(1)? as i32;
        let count = h.count[len] as i32;
        if code - first < count {
            return Some(h.symbol[(index + (code - first)) as usize]);
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    None
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit_lengths = [0u8; 288];
    for (i, l) in lit_lengths.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist_lengths = [5u8; 30];
    (construct(&lit_lengths), construct(&dist_lengths))
}

fn dynamic_tables(reader: &mut BitReader) -> Option<(Huffman, Huffman)> {
    let hlit = reader.bits(5)? as usize + 257;
    let hdist = reader.bits(5)? as usize + 1;
    let hclen = reader.bits(4)? as usize + 4;

    let mut cl_lengths = [0u8; 19];
    for &order in CODE_LENGTH_ORDER.iter().take(hclen) {
        cl_lengths[order] = reader.bits(3)? as u8;
    }
    let cl_table = construct(&cl_lengths);

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = decode(reader, &cl_table)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                let prev = if i == 0 { 0 } else { lengths[i - 1] };
                let repeat = reader.bits(2)? + 3;
                for _ in 0..repeat {
                    if i >= lengths.len() {
                        return None;
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = reader.bits(3)? + 3;
                i += repeat as usize;
            }
            18 => {
                let repeat = reader.bits(7)? + 11;
                i += repeat as usize;
            }
            _ => return None,
        }
    }
    if i != lengths.len() {
        return None;
    }

    let lit_table = construct(&lengths[..hlit]);
    let dist_table = construct(&lengths[hlit..]);
    Some((lit_table, dist_table))
}

/// Runs one compressed block's symbol stream against `out`, using `lit`/
/// `dist` as the (fixed or dynamic) Huffman tables for this block.
fn inflate_block(reader: &mut BitReader, lit: &Huffman, dist: &Huffman, out: &mut Vec<u8>) -> Option<()> {
    loop {
        let sym = decode(reader, lit)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Some(()),
            257..=285 => {
                let idx = (sym - 257) as usize;
                let extra = LENGTH_EXTRA[idx] as u32;
                let length = LENGTH_BASE[idx] as usize + reader.bits(extra)? as usize;

                let dist_sym = decode(reader, dist)? as usize;
                if dist_sym >= DIST_BASE.len() {
                    return None;
                }
                let dist_extra = DIST_EXTRA[dist_sym] as u32;
                let distance = DIST_BASE[dist_sym] as usize + reader.bits(dist_extra)? as usize;
                if distance > out.len() {
                    return None; // back-reference points before the start of output
                }

                let start = out.len() - distance;
                for k in 0..length {
                    let byte = out[start + k]; // may itself be part of this same copy (overlap is legal)
                    out.push(byte);
                }
            }
            _ => return None,
        }
    }
}

/// Inflates a raw DEFLATE bitstream (no zlib/gzip header). `None` on any
/// malformed input (truncated stream, bad Huffman table, back-reference
/// past the start of output) rather than panicking or returning garbage.
pub fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut reader = BitReader::new(data);
    let mut out = Vec::new();

    loop {
        let is_final = reader.bits(1)? == 1;
        let block_type = reader.bits(2)?;
        match block_type {
            0 => {
                reader.align_to_byte();
                let len_bytes = reader.read_bytes(4)?;
                let len = u16::from_le_bytes([len_bytes[0], len_bytes[1]]) as usize;
                let nlen = u16::from_le_bytes([len_bytes[2], len_bytes[3]]);
                if nlen != !(len as u16) {
                    return None;
                }
                out.extend_from_slice(reader.read_bytes(len)?);
            }
            1 => {
                let (lit, dist) = fixed_tables();
                inflate_block(&mut reader, &lit, &dist, &mut out)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut reader)?;
                inflate_block(&mut reader, &lit, &dist, &mut out)?;
            }
            _ => return None,
        }
        if is_final {
            return Some(out);
        }
    }
}

/// Cross-verified against Python's `zlib.compressobj(..., wbits=-15)`
/// output for the same two inputs, at the two block-type "shapes" that
/// matter: a tiny fixed-Huffman-only block, and a longer, repetitive input
/// that a real compressor turns into a dynamic-Huffman block full of
/// back-references.
pub fn self_test() {
    let fixed_compressed = crate::crypto::testhex::hex_vec("cb48cdc9c90700");
    let fixed_expected = b"hello";
    assert_eq!(
        inflate(&fixed_compressed).as_deref(),
        Some(fixed_expected.as_slice()),
        "inflate: fixed-Huffman self-test failed"
    );

    let dyn_compressed = crate::crypto::testhex::hex_vec(
        "edcbb115c3200c04d0556e023670e1226586904176b001291827c4d39b97ac90924e77f7551e8ce7e1ed8629cb3b61968af588ba435e9c51da1ce8fcc0c962bea9e38e3bfe2b1e93cbe2dd9d929f792fa6c680d20e28d98d161eac44c395a20636a4da5e270ec3edd76054bd00",
    );
    let mut dyn_expected = alloc::string::String::new();
    for _ in 0..20 {
        dyn_expected.push_str("the quick brown fox jumps over the lazy dog. ");
    }
    dyn_expected.push_str("AndroidManifest.xml test package=com.example.app label=Example App");
    assert_eq!(
        inflate(&dyn_compressed).as_deref(),
        Some(dyn_expected.as_bytes()),
        "inflate: dynamic-Huffman self-test failed"
    );

    crate::serial_println!("inflate: all self-tests passed (fixed/dynamic Huffman DEFLATE)");
}
