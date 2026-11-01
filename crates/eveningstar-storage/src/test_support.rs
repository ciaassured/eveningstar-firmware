//! Fixtures and builders shared by the tests.

#![allow(
    clippy::cast_possible_truncation,
    reason = "the generator and the builders keep low bits on purpose, of values that fit"
)]

use crate::Entry;
use crate::table::ENTRY_LEN;

/// The table Nix builds from `nix/board.nix`.
pub(crate) const TABLE: &[u8] = include_bytes!("../testdata/partition-table.bin");

/// The table espflash writes for an 8 MiB flash.
pub(crate) const ESPFLASH_TABLE: &[u8] = include_bytes!("../testdata/espflash-partition-table.bin");

/// A small deterministic pseudo-random generator (xorshift64).
#[derive(Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub(crate) fn byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    /// A number below `n`, which isn't zero.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub(crate) fn fill(&mut self, bytes: &mut [u8]) {
        for b in bytes {
            *b = self.byte();
        }
    }
}

/// The 32 bytes of a partition entry.
pub(crate) fn entry_bytes(entry: &Entry) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..2].copy_from_slice(&0x50AA_u16.to_le_bytes());
    bytes[2] = entry.kind;
    bytes[3] = entry.subtype;
    bytes[4..8].copy_from_slice(&entry.offset.to_le_bytes());
    bytes[8..12].copy_from_slice(&entry.size.to_le_bytes());
    bytes[12..12 + entry.label.len()].copy_from_slice(entry.label.as_bytes());
    bytes[28..].copy_from_slice(&entry.flags.to_le_bytes());
    bytes
}

/// An MD5 entry. The digest is a fixed pattern, as nothing checks it.
pub(crate) fn md5_entry() -> [u8; 32] {
    let mut bytes = [0xFF; 32];
    bytes[..2].copy_from_slice(&0xEBEB_u16.to_le_bytes());
    for (i, b) in bytes[16..].iter_mut().enumerate() {
        *b = i as u8;
    }
    bytes
}

/// A full-length table holding `slots` (entries, an MD5 entry, anything),
/// then erased bytes.
pub(crate) fn table_of(slots: &[[u8; 32]]) -> Vec<u8> {
    let mut table = vec![0xFF; crate::table::MAX_LEN as usize];
    for (i, slot) in slots.iter().enumerate() {
        let at = i * ENTRY_LEN as usize;
        table[at..at + 32].copy_from_slice(slot);
    }
    table
}

/// A table holding `entries`, then an MD5 entry, as ESP-IDF writes one.
pub(crate) fn table_with_md5(entries: &[Entry]) -> Vec<u8> {
    let mut slots: Vec<[u8; 32]> = entries.iter().map(entry_bytes).collect();
    slots.push(md5_entry());
    table_of(&slots)
}

/// A source over `size` erased bytes with `parts` written at their offsets.
pub(crate) fn flash_with(size: usize, parts: &[(u32, &[u8])]) -> Vec<u8> {
    let mut flash = vec![0xFF; size];
    for &(offset, bytes) in parts {
        let at = offset as usize;
        flash[at..at + bytes.len()].copy_from_slice(bytes);
    }
    flash
}
