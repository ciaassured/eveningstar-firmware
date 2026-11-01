//! Fixtures and builders shared by the tests.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the generator and the builders keep low bits on purpose, of values that fit"
)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::task::Wake;
use core::pin::pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};

use sha2::{Digest as _, Sha256};

use std::sync::Mutex;

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

/// The otadata partition as the board's bootloader left it on its first boot
/// after `flash-all` wrote it blank.
pub(crate) const OTADATA_FIRST_BOOT: &[u8] = include_bytes!("../testdata/otadata-first-boot.bin");

/// Length of the otadata partition in bytes.
pub(crate) const OTADATA_LEN: usize = crate::layout::OTADATA.size as usize;

/// An otadata partition holding `first` and `second` at the starts of its
/// sectors, erased elsewhere.
pub(crate) fn otadata_of(first: &[u8; 32], second: &[u8; 32]) -> Vec<u8> {
    flash_with(OTADATA_LEN, &[(0, first), (SECTOR_LEN, second)])
}

const SECTOR_LEN: u32 = crate::otadata::SECTOR_SIZE;

/// How a program a power cut stops leaves its bytes. NOR programming only
/// clears bits, so what lands is some of the bits the program clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tear {
    /// Its first `n` bytes land, as a program that runs in address order.
    Prefix(usize),
    /// A random subset of the bits it clears lands, short of all of them,
    /// drawn from the seed: about half, three quarters or fifteen sixteenths
    /// of them.
    Bits(u64),
    /// Every bit it clears lands but one: the `n`th of them, counted modulo
    /// their number.
    AllBut(usize),
}

impl Tear {
    /// Programs `new` over `old` as far as the tear lets it.
    pub(crate) fn land(self, old: &mut [u8], new: &[u8]) {
        // The bits the program clears, in address order.
        let clears: Vec<(usize, u8)> = old
            .iter()
            .zip(new)
            .enumerate()
            .flat_map(|(i, (old, new))| {
                let clear = old & !new;
                (0..8)
                    .map(|bit| 1_u8 << bit)
                    .filter(move |bit| clear & bit != 0)
                    .map(move |bit| (i, bit))
            })
            .collect();
        let lands: Vec<bool> = match self {
            Self::Prefix(n) => clears.iter().map(|&(i, _)| i < n).collect(),
            Self::Bits(seed) => {
                let mut rng = Rng::new(seed);
                let draws = [1, 2, 4][(seed % 3) as usize];
                let mut lands: Vec<bool> = clears
                    .iter()
                    .map(|_| (0..draws).any(|_| rng.below(2) == 0))
                    .collect();
                if !lands.is_empty() && lands.iter().all(|&land| land) {
                    let kept = rng.below(lands.len());
                    lands[kept] = false;
                }
                lands
            }
            Self::AllBut(n) => {
                let kept = n % clears.len().max(1);
                (0..clears.len()).map(|j| j != kept).collect()
            }
        };
        for (&(i, bit), land) in clears.iter().zip(lands) {
            if land {
                old[i] &= !bit;
            }
        }
    }
}

/// Where a power cut stops a planned otadata write: after its erase and its
/// first `done` programs, with the next one torn as `tear` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cut {
    pub(crate) done: usize,
    pub(crate) tear: Tear,
}

/// `write` executed on `otadata`: its sector erased, then its programs made,
/// in order.
pub(crate) fn apply(otadata: &mut [u8], write: &crate::otadata::Write) {
    apply_cut(otadata, write, None);
}

/// `write` on `otadata`, cut short as `cut` says, or whole when it is
/// `None`: its sector erased, then its programs made, in order, until the
/// cut.
pub(crate) fn apply_cut(otadata: &mut [u8], write: &crate::otadata::Write, cut: Option<Cut>) {
    let at = write.offset() as usize;
    otadata[at..at + SECTOR_LEN as usize].fill(0xFF);
    for (index, (offset, bytes)) in write.programs().into_iter().enumerate() {
        let span = &mut otadata[offset as usize..offset as usize + bytes.len()];
        match cut {
            Some(cut) if index == cut.done => {
                cut.tear.land(span, bytes);
                return;
            }
            _ => {
                for (old, new) in span.iter_mut().zip(bytes) {
                    *old &= new;
                }
            }
        }
    }
}

/// The cuts of `write` the tests try: in each of its programs, every prefix
/// and every bit but one, and random subsets of its bits.
pub(crate) fn every_cut(write: &crate::otadata::Write) -> Vec<Cut> {
    let mut cuts = Vec::new();
    for (done, (_, bytes)) in write.programs().into_iter().enumerate() {
        let clears: usize = bytes.iter().map(|b| b.count_zeros() as usize).sum();
        let tears = (0..bytes.len())
            .map(Tear::Prefix)
            .chain((0..clears).map(Tear::AllBut))
            .chain((0..6).map(|seed| Tear::Bits(0x7EA2 + seed)));
        cuts.extend(tears.map(|tear| Cut { done, tear }));
    }
    cuts
}

/// A random cut of `write`.
pub(crate) fn random_cut(rng: &mut Rng, write: &crate::otadata::Write) -> Cut {
    let done = rng.below(3);
    let len = write.programs()[done].1.len();
    let tear = match rng.below(3) {
        0 => Tear::Prefix(rng.below(len)),
        1 => Tear::Bits(rng.next_u64()),
        _ => Tear::AllBut(rng.below(256)),
    };
    Cut { done, tear }
}

/// An `esp_select_entry_t` as ESP-IDF's C code sees it, read from raw bytes
/// without the crate's parser, so the simulated bootloader checks the parser
/// rather than sharing it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IdfEntry {
    pub(crate) seq: u32,
    pub(crate) state: u32,
    pub(crate) crc: u32,
    bytes: [u8; 32],
}

const IDF_NEW: u32 = 0;
const IDF_PENDING_VERIFY: u32 = 1;
const IDF_VALID: u32 = 2;
const IDF_INVALID: u32 = 3;
const IDF_ABORTED: u32 = 4;

impl IdfEntry {
    pub(crate) fn read(otadata: &[u8], index: usize) -> Self {
        let at = index * SECTOR_LEN as usize;
        let bytes: [u8; 32] = otadata[at..at + 32].try_into().unwrap();
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Self {
            seq: word(0),
            state: word(24),
            crc: word(28),
            bytes,
        }
    }

    /// `bootloader_common_ota_select_crc` matches.
    fn crc_ok(&self) -> bool {
        self.crc == crate::otadata::crc(self.seq)
    }

    /// `bootloader_common_ota_select_invalid`, which ignores the CRC.
    fn invalid(&self) -> bool {
        self.seq == u32::MAX || self.state == IDF_INVALID || self.state == IDF_ABORTED
    }

    /// `bootloader_common_ota_select_valid`.
    pub(crate) fn valid(&self) -> bool {
        !self.invalid() && self.crc_ok()
    }

    /// `write_otadata` of this entry with `state`: the sector erased, then the
    /// struct as read written back with only the state changed.
    fn rewrite(&mut self, otadata: &mut [u8], index: usize, state: u32) {
        self.state = state;
        self.bytes[24..28].copy_from_slice(&state.to_le_bytes());
        let at = index * SECTOR_LEN as usize;
        otadata[at..at + SECTOR_LEN as usize].fill(0xFF);
        otadata[at..at + 32].copy_from_slice(&self.bytes);
    }
}

/// `bootloader_common_get_active_otadata`: the index of the active entry.
pub(crate) fn idf_active(entries: &[IdfEntry; 2]) -> Option<usize> {
    match (entries[0].valid(), entries[1].valid()) {
        (true, true) => Some(usize::from(
            entries[0].seq.max(entries[1].seq) != entries[0].seq,
        )),
        (true, false) => Some(0),
        (false, true) => Some(1),
        (false, false) => None,
    }
}

/// The slot ESP-IDF maps sequence number `seq` to with two OTA slots.
pub(crate) fn idf_slot(seq: u32) -> crate::Slot {
    crate::Slot::ALL[(seq.wrapping_sub(1) % 2) as usize]
}

/// The ESP-IDF 5.5 bootloader's choice of app, with
/// `BOOTLOADER_APP_ROLLBACK_ENABLE`, two OTA slots and no factory or test
/// app, followed branch by branch from `bootloader_utility.c`
/// (`bootloader_utility_get_selected_boot_partition`,
/// `bootloader_utility_load_boot_image`, `set_actual_ota_seq`).
///
/// `loadable` says which slots hold an image the bootloader loads. The
/// otadata is updated as the bootloader writes it, and the slot started is
/// returned, `None` when no slot loads (the bootloader resets).
pub(crate) fn boot(otadata: &mut [u8], loadable: [bool; 2]) -> Option<crate::Slot> {
    let mut e = [IdfEntry::read(otadata, 0), IdfEntry::read(otadata, 1)];

    // Every PENDING_VERIFY entry is aborted, whatever its CRC.
    for (i, entry) in e.iter_mut().enumerate() {
        if entry.state == IDF_PENDING_VERIFY {
            entry.rewrite(otadata, i, IDF_ABORTED);
        }
    }

    // Index -1 is the factory app, which the layout doesn't have.
    let mut initial = false;
    let boot_index: i32 = if e[0].invalid() && e[1].invalid() {
        // "No factory image, trying OTA 0", and remember whether the otadata
        // is blank or corrupt in both copies.
        let blank_or_corrupt = |e: &IdfEntry| e.seq == u32::MAX || !e.crc_ok();
        initial = blank_or_corrupt(&e[0]) && blank_or_corrupt(&e[1]);
        0
    } else if let Some(i) = idf_active(&e) {
        if e[i].state == IDF_NEW {
            e[i].rewrite(otadata, i, IDF_PENDING_VERIFY);
        }
        idf_slot(e[i].seq).number() as i32
    } else {
        // "ota data partition invalid and no factory, will try all
        // partitions".
        -1
    };

    // Down from the selected index to the factory app, then up from it.
    let order = (0..=boot_index).rev().chain(boot_index + 1..2);
    for index in order {
        let index = index as usize;
        if loadable[index] {
            if initial {
                let seq = index as u32 + 1;
                let mut entry = [0xFF; 32];
                entry[..4].copy_from_slice(&seq.to_le_bytes());
                entry[24..28].copy_from_slice(&IDF_VALID.to_le_bytes());
                entry[28..].copy_from_slice(&crate::otadata::crc(seq).to_le_bytes());
                otadata[..SECTOR_LEN as usize].fill(0xFF);
                otadata[..32].copy_from_slice(&entry);
            }
            return Some(crate::Slot::ALL[index]);
        }
    }
    None
}

/// Size of the board's flash in bytes.
pub(crate) const FLASH_LEN: usize = crate::layout::FLASH_SIZE as usize;

/// A flash in RAM with NOR semantics: an erase sets a sector to `0xFF`, a
/// program ANDs the bytes in. A call that breaks the [`Flash`] contract
/// (misaligned, past the end, more than a sector programmed at once) panics,
/// as it is a bug in the crate. Every erase and program is logged, and they
/// can be made to fail after a number of them, the first that fails landing
/// in part as a power cut leaves it.
///
/// [`Flash`]: crate::Flash
#[derive(Debug)]
pub(crate) struct RamFlash {
    bytes: Mutex<Vec<u8>>,
    /// Erases and programs that succeeded, as (address, length, is erase).
    ops: Mutex<Vec<Op>>,
    /// The number of erases and programs that succeed before every later one
    /// fails.
    fail_after: Mutex<Option<usize>>,
    /// How the first program that fails lands.
    tear: Mutex<Option<Tear>>,
    /// Reads that touch this range fail.
    fail_reads: Mutex<Option<core::ops::Range<u32>>>,
}

/// An erase or program a [`RamFlash`] carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Erase(u32),
    Program(u32, u32),
}

impl RamFlash {
    /// A flash holding `bytes`.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        assert!(u32::try_from(bytes.len()).is_ok());
        Self {
            bytes: Mutex::new(bytes),
            ops: Mutex::default(),
            fail_after: Mutex::default(),
            tear: Mutex::default(),
            fail_reads: Mutex::default(),
        }
    }

    /// An erased flash of `len` bytes with `parts` written at their offsets.
    pub(crate) fn with(len: usize, parts: &[(u32, &[u8])]) -> Self {
        Self::new(flash_with(len, parts))
    }

    /// Makes every erase and program fail once `n` more have succeeded.
    pub(crate) fn fail_after(&self, n: usize) {
        let done = self.ops.lock().unwrap().len();
        *self.fail_after.lock().unwrap() = Some(done + n);
    }

    /// Cuts the power once `n` more erases and programs have succeeded: every
    /// later one fails, and when the first to fail is a program, it lands as
    /// `tear` says all the same.
    pub(crate) fn cut_after(&self, n: usize, tear: Tear) {
        self.fail_after(n);
        *self.tear.lock().unwrap() = Some(tear);
    }

    /// Makes every read that touches `range` fail.
    pub(crate) fn fail_reads(&self, range: core::ops::Range<u32>) {
        *self.fail_reads.lock().unwrap() = Some(range);
    }

    /// Lets every erase, program and read succeed again.
    pub(crate) fn heal(&self) {
        *self.fail_after.lock().unwrap() = None;
        *self.tear.lock().unwrap() = None;
        *self.fail_reads.lock().unwrap() = None;
    }

    /// Writes `bytes` at `address` as they are, without logging them or
    /// failing: what the bootloader writes, or what decays on the flash.
    pub(crate) fn poke(&self, address: u32, bytes: &[u8]) {
        let range = self.range(address, bytes.len());
        self.bytes.lock().unwrap()[range].copy_from_slice(bytes);
    }

    /// What `read` returns given the bytes, without copying them.
    pub(crate) fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        read(&self.bytes.lock().unwrap())
    }

    /// The erases and programs carried out so far.
    pub(crate) fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    /// A copy of the bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }

    /// A copy of the `len` bytes at `address`.
    pub(crate) fn at(&self, address: u32, len: usize) -> Vec<u8> {
        let at = address as usize;
        self.bytes.lock().unwrap()[at..at + len].to_vec()
    }

    fn range(&self, address: u32, len: usize) -> core::ops::Range<usize> {
        let start = address as usize;
        let end = start.checked_add(len).expect("range overflows");
        assert!(
            end <= self.bytes.lock().unwrap().len(),
            "{start:#x}..{end:#x} is past the end of the flash"
        );
        start..end
    }

    /// Logs `op`, or fails it when the failure point has been reached.
    fn record(&self, op: Op) -> Result<(), crate::DeviceError> {
        let mut ops = self.ops.lock().unwrap();
        if self
            .fail_after
            .lock()
            .unwrap()
            .is_some_and(|n| ops.len() >= n)
        {
            return Err(crate::DeviceError);
        }
        ops.push(op);
        Ok(())
    }
}

/// A [`RamFlash`] lent to [`crate::split`], which takes its flash mutably, while
/// the test keeps reading it: the box is leaked, so the borrow lasts as long as
/// `flash` does.
pub(crate) fn lend(flash: &RamFlash) -> &mut dyn crate::Flash {
    Box::leak(Box::new(Lent(flash)))
}

/// A shared [`RamFlash`] as a [`crate::Flash`], for [`lend`].
struct Lent<'a>(&'a RamFlash);

impl crate::Flash for Lent<'_> {
    fn capacity(&self) -> u32 {
        crate::Flash::capacity(self.0)
    }
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), crate::DeviceError> {
        crate::Flash::read(self.0, address, buf)
    }
    fn erase_sector(&self, address: u32) -> Result<(), crate::DeviceError> {
        crate::Flash::erase_sector(self.0, address)
    }
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), crate::DeviceError> {
        crate::Flash::program(self.0, address, bytes)
    }
}

impl crate::Flash for RamFlash {
    fn capacity(&self) -> u32 {
        self.bytes.lock().unwrap().len() as u32
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), crate::DeviceError> {
        let range = self.range(address, buf.len());
        let fails = self.fail_reads.lock().unwrap().clone();
        if let Some(fails) = fails
            && (range.start as u32) < fails.end
            && fails.start < range.end as u32
        {
            return Err(crate::DeviceError);
        }
        buf.copy_from_slice(&self.bytes.lock().unwrap()[range]);
        Ok(())
    }

    fn erase_sector(&self, address: u32) -> Result<(), crate::DeviceError> {
        assert!(
            address.is_multiple_of(crate::SECTOR_SIZE),
            "erase at {address:#x}"
        );
        let range = self.range(address, crate::SECTOR_SIZE as usize);
        if let Err(error) = self.record(Op::Erase(address)) {
            // A cut erase lands whole or not at all here.
            *self.tear.lock().unwrap() = None;
            return Err(error);
        }
        self.bytes.lock().unwrap()[range].fill(0xFF);
        Ok(())
    }

    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), crate::DeviceError> {
        assert!(
            address.is_multiple_of(crate::WORD_SIZE) && bytes.len().is_multiple_of(4),
            "program of {} bytes at {address:#x}",
            bytes.len()
        );
        assert!(
            bytes.len() <= crate::SECTOR_SIZE as usize,
            "program of {} bytes at once",
            bytes.len()
        );
        let range = self.range(address, bytes.len());
        let recorded = self.record(Op::Program(address, bytes.len() as u32));
        let mut flash = self.bytes.lock().unwrap();
        if recorded.is_err() {
            if let Some(tear) = self.tear.lock().unwrap().take() {
                tear.land(&mut flash[range], bytes);
            }
            return recorded;
        }
        for (old, new) in flash[range].iter_mut().zip(bytes) {
            *old &= new;
        }
        Ok(())
    }
}

/// The otadata partition's offset.
pub(crate) const OTADATA_AT: u32 = crate::layout::OTADATA.offset;

/// A board flashed by `flash-all`: the Nix bootloader and table, the otadata
/// the bootloader writes on its first boot, and the app in `ota_0`.
pub(crate) fn nix_board() -> RamFlash {
    RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (crate::layout::TABLE_OFFSET, TABLE),
            (OTADATA_AT, OTADATA_FIRST_BOOT),
            (crate::layout::OTA_0.offset, APP),
        ],
    )
}

/// A board flashed by `cargo run`: espflash's bootloader and default table,
/// and the app in its `factory` partition at 0x10000.
pub(crate) fn espflash_board() -> RamFlash {
    RamFlash::with(
        FLASH_LEN,
        &[
            (0, ESPFLASH_BOOTLOADER),
            (0x8000, ESPFLASH_TABLE),
            (0x1_0000, APP),
        ],
    )
}

/// Whether the ESP-IDF bootloader loads the app image at the start of
/// `partition`, as `esp_image_verify` checks it without secure boot: the
/// magic, the chip, at most 16 segments, each a whole number of words inside
/// the partition, the checksum, and the SHA-256 when one is appended. It reads
/// the format on its own, apart from the crate's parser, so the simulated
/// boots check what the crate verifies.
pub(crate) fn idf_loads(partition: &[u8]) -> bool {
    let word = |at: usize| {
        partition
            .get(at..at + 4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
    };
    let Some(header) = partition.get(..24) else {
        return false;
    };
    if header[0] != 0xE9 || header[12..14] != [0x0D, 0x00] || header[1] > 16 {
        return false;
    }
    let mut at = 24;
    let mut checksum = 0xEF;
    for _ in 0..header[1] {
        let Some(len) = word(at + 4) else {
            return false;
        };
        let Some(data) = partition.get(at + 8..at + 8 + len) else {
            return false;
        };
        if len % 4 != 0 {
            return false;
        }
        checksum = data.iter().fold(checksum, |sum, b| sum ^ b);
        at += 8 + len;
    }
    let end = (at | 0x0F) + 1;
    if partition.get(end - 1) != Some(&checksum) {
        return false;
    }
    header[23] == 0
        || partition
            .get(end..end + 32)
            .is_some_and(|stored| Sha256::digest(&partition[..end])[..] == *stored)
}

/// One boot of `flash` by the simulated ESP-IDF bootloader ([`boot`]), which
/// loads a slot when [`idf_loads`] says so and writes the otadata back
/// unlogged: the slot started, `None` when none loads.
pub(crate) fn boot_flash(flash: &RamFlash) -> Option<crate::Slot> {
    let loadable = crate::Slot::ALL.map(|slot| {
        let entry = slot.partition();
        flash.with_bytes(|bytes| idf_loads(&bytes[entry.offset as usize..entry.end() as usize]))
    });
    let mut otadata = flash.at(OTADATA_AT, OTADATA_LEN);
    let booted = boot(&mut otadata, loadable);
    flash.poke(OTADATA_AT, &otadata);
    booted
}
