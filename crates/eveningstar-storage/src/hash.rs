//! SHA-256, behind a trait, so an image can be verified with any
//! implementation of it: [`sha2::Sha256`] in software, which the crate uses
//! unless it is given another, or a hardware accelerator.

/// A SHA-256 hash in progress. A clone carries the state of the hash so far,
/// so two digests can share a prefix that is hashed once.
pub trait Sha256Hasher: Clone {
    /// Hashes `bytes`, after those hashed so far.
    fn update(&mut self, bytes: &[u8]);

    /// The digest of every byte hashed.
    fn finalize(self) -> [u8; 32];
}

impl Sha256Hasher for sha2::Sha256 {
    fn update(&mut self, bytes: &[u8]) {
        sha2::Digest::update(self, bytes);
    }

    fn finalize(self) -> [u8; 32] {
        sha2::Digest::finalize(self).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SHA-256 of "abc", FIPS 180-2's first example.
    const ABC: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];

    #[test]
    fn the_software_hasher_gives_the_sha256_of_its_input() {
        let mut sha = sha2::Sha256::default();
        sha.update(b"a");
        sha.update(b"bc");
        assert_eq!(sha.finalize(), ABC);
    }

    #[test]
    fn a_clone_carries_the_hash_so_far() {
        let mut sha = sha2::Sha256::default();
        sha.update(b"ab");
        let mut copy = sha.clone();
        sha.update(b"c");
        copy.update(b"c");
        assert_eq!(sha.finalize(), ABC);
        assert_eq!(copy.finalize(), ABC);
    }
}
