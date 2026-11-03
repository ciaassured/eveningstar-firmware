//! MD5 (RFC 1321), for the digest the partition table stores. It holds one
//! 64-byte block and the state, and hashes as bytes arrive.

/// The shift of each step, four per round.
static SHIFTS: [[u32; 4]; 4] = [
    [7, 12, 17, 22],
    [5, 9, 14, 20],
    [4, 11, 16, 23],
    [6, 10, 15, 21],
];

/// The constant of each step: the integer part of `abs(sin(i + 1)) * 2^32`.
static SINES: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

const BLOCK_LEN: usize = 64;

/// Where the length goes in the last block.
const LENGTH_AT: usize = BLOCK_LEN - 8;

/// An MD5 hash in progress.
#[derive(Clone, Debug)]
pub(crate) struct Md5 {
    state: [u32; 4],
    block: [u8; BLOCK_LEN],
    /// Bytes of `block` filled.
    filled: usize,
    /// Bytes hashed so far, modulo 2^64.
    len: u64,
}

impl Md5 {
    pub(crate) const fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            block: [0; BLOCK_LEN],
            filled: 0,
            len: 0,
        }
    }

    /// Hashes `bytes`, after those hashed so far.
    pub(crate) fn update(&mut self, mut bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        while !bytes.is_empty() {
            let free = &mut self.block[self.filled..];
            let take = free.len().min(bytes.len());
            let (now, rest) = bytes.split_at(take);
            free[..take].copy_from_slice(now);
            self.filled += take;
            bytes = rest;
            if self.filled == BLOCK_LEN {
                compress(&mut self.state, &self.block);
                self.filled = 0;
            }
        }
    }

    /// The digest of every byte hashed.
    #[must_use]
    pub(crate) fn finalize(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        if self.filled > LENGTH_AT {
            self.block[self.filled..].fill(0);
            compress(&mut self.state, &self.block);
            self.filled = 0;
        }
        self.block[self.filled..LENGTH_AT].fill(0);
        self.block[LENGTH_AT..].copy_from_slice(&bits.to_le_bytes());
        compress(&mut self.state, &self.block);
        let mut digest = [0; 16];
        for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *bytes = word.to_le_bytes();
        }
        digest
    }
}

/// Hashes one block into `state`.
#[allow(
    clippy::many_single_char_names,
    reason = "the names RFC 1321 gives the state words and the round function"
)]
fn compress(state: &mut [u32; 4], block: &[u8; BLOCK_LEN]) {
    let mut words = [0; 16];
    for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*bytes);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for (step, &sine) in SINES.iter().enumerate() {
        let round = step / 16;
        let (f, word) = match round {
            0 => ((b & c) | (!b & d), step),
            1 => ((d & b) | (!d & c), (5 * step + 1) % 16),
            2 => (b ^ c ^ d, (3 * step + 5) % 16),
            _ => (c ^ (b | !d), (7 * step) % 16),
        };
        let sum = f
            .wrapping_add(a)
            .wrapping_add(sine)
            .wrapping_add(words[word]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(sum.rotate_left(SHIFTS[round][step % 4]));
    }
    for (word, add) in state.iter_mut().zip([a, b, c, d]) {
        *word = word.wrapping_add(add);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5(bytes: &[u8]) -> [u8; 16] {
        let mut md5 = Md5::new();
        md5.update(bytes);
        md5.finalize()
    }

    fn hex(digest: [u8; 16]) -> String {
        use core::fmt::Write as _;
        digest.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
    }

    #[test]
    fn the_rfc_1321_test_suite() {
        let suite: [(&[u8], &str); 7] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", "0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                b"abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, digest) in suite {
            assert_eq!(hex(md5(input)), digest, "{}", input.escape_ascii());
        }
    }

    #[test]
    fn padding_that_spills_into_another_block() {
        // 55 bytes leave room for the 0x80 and the length; 56 to 63 don't.
        for (len, digest) in [
            (55, "ef1772b6dff9a122358552954ad0df65"),
            (56, "3b0c8ac703f828b04c6c197006d17218"),
            (63, "b06521f39153d618550606be297466d5"),
            (64, "014842d480b571495a4a0363793f7367"),
        ] {
            assert_eq!(hex(md5(&vec![b'a'; len])), digest, "{len}");
        }
    }

    #[test]
    fn updates_in_parts_give_the_digest_of_the_whole() {
        let bytes: Vec<u8> = (0..=255).cycle().take(300).collect();
        let whole = md5(&bytes);
        for part in [1, 7, 32, 63, 64, 65, 300] {
            let mut md5 = Md5::new();
            for chunk in bytes.chunks(part) {
                md5.update(chunk);
            }
            assert_eq!(md5.finalize(), whole, "{part}");
        }
    }
}
