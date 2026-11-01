//! Fixtures and builders shared by the tests.

#![allow(
    clippy::cast_possible_truncation,
    reason = "the generator and the builders keep low bits on purpose, of values that fit"
)]

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use core::pin::pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};

use sha2::{Digest as _, Sha256};

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

/// The bootloader Nix builds.
pub(crate) const BOOTLOADER: &[u8] = include_bytes!("../testdata/bootloader.bin");

/// The first 32 KiB espflash writes: its bootloader, then erased bytes.
pub(crate) const ESPFLASH_BOOTLOADER: &[u8] = include_bytes!("../testdata/espflash-bootloader.bin");

/// An app image of the firmware, made by espflash.
pub(crate) const APP: &[u8] = include_bytes!("../testdata/app.bin");

/// What running a future to completion took.
#[derive(Debug)]
pub(crate) struct Run<T> {
    pub(crate) output: T,
    /// Times the future was polled.
    pub(crate) polls: usize,
    /// Times it woke its waker.
    pub(crate) wakes: usize,
}

#[derive(Default)]
struct CountingWaker(AtomicUsize);

impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// Polls `future` until it completes, counting polls and wakes. A future that
/// returns `Pending` without waking its waker would never be polled again by a
/// real executor, so that fails the run.
pub(crate) fn block_on<F: Future>(future: F) -> Run<F::Output> {
    let counter = Arc::new(CountingWaker::default());
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    let mut polls = 0;
    loop {
        let woken = counter.0.load(Ordering::Relaxed);
        polls += 1;
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return Run {
                output,
                polls,
                wakes: counter.0.load(Ordering::Relaxed),
            };
        }
        assert!(
            counter.0.load(Ordering::Relaxed) > woken,
            "pending without a wake"
        );
    }
}

/// Polls `future` until it completes, with a waker that does nothing.
pub(crate) fn run<F: Future>(future: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

/// An image assembled from a header and segments, with its padding, checksum
/// and digest computed.
#[derive(Clone, Debug)]
pub(crate) struct ImageBuilder {
    pub(crate) header: [u8; 24],
    pub(crate) segments: Vec<(u32, Vec<u8>)>,
}

impl ImageBuilder {
    /// An ESP32-C6 image of `segments`, with a digest appended.
    pub(crate) fn new(segments: Vec<(u32, Vec<u8>)>) -> Self {
        let mut header = [0; 24];
        header[0] = 0xE9;
        header[1] = segments.len() as u8;
        header[2] = 0x02;
        header[3] = 0x30;
        header[4..8].copy_from_slice(&0x4080_0000_u32.to_le_bytes());
        header[8] = 0xEE;
        header[12..14].copy_from_slice(&0x000D_u16.to_le_bytes());
        header[17..19].copy_from_slice(&99_u16.to_le_bytes());
        header[23] = 1;
        Self { header, segments }
    }

    /// The image's bytes.
    pub(crate) fn build(&self) -> Vec<u8> {
        let mut image = self.header.to_vec();
        let mut checksum = 0xEF;
        for (load_addr, data) in &self.segments {
            image.extend_from_slice(&load_addr.to_le_bytes());
            image.extend_from_slice(&(data.len() as u32).to_le_bytes());
            image.extend_from_slice(data);
            checksum = data.iter().fold(checksum, |sum, b| sum ^ b);
        }
        while !(image.len() + 1).is_multiple_of(16) {
            image.push(0);
        }
        image.push(checksum);
        if self.header[23] != 0 {
            let digest = Sha256::digest(&image);
            image.extend_from_slice(&digest);
        }
        image
    }
}

/// An `esp_app_desc_t` naming `project` and `version`.
pub(crate) fn app_descriptor_bytes(project: &str, version: &str) -> Vec<u8> {
    let mut desc = vec![0; 256];
    desc[..4].copy_from_slice(&0xABCD_5432_u32.to_le_bytes());
    desc[4..8].copy_from_slice(&7_u32.to_le_bytes());
    desc[16..16 + version.len()].copy_from_slice(version.as_bytes());
    desc[48..48 + project.len()].copy_from_slice(project.as_bytes());
    desc[80..88].copy_from_slice(b"12:34:56");
    desc[96..107].copy_from_slice(b"2026-11-01\0");
    desc[112..118].copy_from_slice(b"v5.5.2");
    for (i, b) in desc[144..176].iter_mut().enumerate() {
        *b = i as u8;
    }
    desc
}

/// An `esp_bootloader_desc_t`.
pub(crate) fn bootloader_descriptor_bytes() -> Vec<u8> {
    let mut desc = vec![0; 80];
    desc[0] = 0x50;
    desc[3] = 2;
    desc[4..8].copy_from_slice(&3_u32.to_le_bytes());
    desc[8..14].copy_from_slice(b"v5.5.2");
    desc[40..59].copy_from_slice(b"Nov  1 2026 12:00:0");
    desc
}

/// `len` bytes of data from `rng`.
pub(crate) fn data(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    rng.fill(&mut data);
    data
}

/// A small app: a mapped segment with the descriptor, then two in RAM.
pub(crate) fn app_image() -> ImageBuilder {
    let mut rng = Rng::new(0xA77);
    let mut first = app_descriptor_bytes("eveningstar", "1.2.3");
    first.extend(data(&mut rng, 0x1F00));
    ImageBuilder::new(vec![
        (0x4200_0020, first),
        (0x4080_0000, data(&mut rng, 0x1234)),
        (0x4081_0000, data(&mut rng, 0x10)),
    ])
}

/// A small bootloader: a segment with the descriptor, then one more.
pub(crate) fn bootloader_image() -> ImageBuilder {
    let mut rng = Rng::new(0xB007);
    let mut first = bootloader_descriptor_bytes();
    first.extend(data(&mut rng, 0x400));
    ImageBuilder::new(vec![
        (0x4087_5730, first),
        (0x4086_B910, data(&mut rng, 0x804)),
    ])
}

/// Offset of segment `index`'s header in `image`.
pub(crate) fn segment_at(image: &[u8], index: usize) -> usize {
    let mut at = 24;
    for _ in 0..index {
        let len = u32::from_le_bytes(image[at + 4..at + 8].try_into().unwrap());
        at += 8 + len as usize;
    }
    at
}
