//! Fixtures and builders for tests: the files in `testdata/`, byte-level
//! constructors of tables, images and otadata, a simulated ESP-IDF
//! bootloader, and a poll loop for futures.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    reason = "the generator and the builders keep low bits on purpose, of values that fit, and panic on malformed input"
)]

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use alloc::vec;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};

use sha2::{Digest as _, Sha256};

use crate::Entry;
use crate::md5::Md5;
use crate::table::ENTRY_LEN;

/// Offset of the board's partition table.
pub const TABLE_OFFSET: u32 = 0x8000;

const fn partition(
    label: &'static str,
    kind: u8,
    subtype: u8,
    offset: u32,
    size: u32,
    flags: u32,
) -> Entry {
    Entry {
        label,
        kind,
        subtype,
        offset,
        size,
        flags,
    }
}

/// The partitions of the table in [`TABLE`], in order. The fixture's tests
/// check that the table holds them.
#[rustfmt::skip]
pub const PARTITIONS: [Entry; 8] = [
    partition("factory",  0x01, 0x06, 0x0000_9000, 0x0000_1000, 0x2),
    partition("otadata",  0x01, 0x00, 0x0000_a000, 0x0000_2000, 0),
    partition("ota_0",    0x00, 0x10, 0x0001_0000, 0x0030_0000, 0),
    partition("ota_1",    0x00, 0x11, 0x0031_0000, 0x0030_0000, 0),
    partition("config_0", 0x01, 0x06, 0x0061_0000, 0x0002_0000, 0),
    partition("config_1", 0x01, 0x06, 0x0063_0000, 0x0002_0000, 0),
    partition("diag",     0x01, 0x06, 0x0065_0000, 0x0004_0000, 0),
    partition("reserved", 0x01, 0x06, 0x0069_0000, 0x0017_0000, 0),
];

/// The table Nix builds from `nix/board.nix`.
pub const TABLE: &[u8] = include_bytes!("../testdata/partition-table.bin");

/// The table espflash writes for an 8 MiB flash.
pub const ESPFLASH_TABLE: &[u8] = include_bytes!("../testdata/espflash-partition-table.bin");

/// A small deterministic pseudo-random generator (xorshift64).
#[derive(Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub fn byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    /// A number below `n`, which isn't zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn fill(&mut self, bytes: &mut [u8]) {
        for b in bytes {
            *b = self.byte();
        }
    }
}

/// The 32 bytes of a partition entry.
pub fn entry_bytes(entry: &Entry) -> [u8; 32] {
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

/// An MD5 entry whose digest is a fixed pattern, the MD5 of no entries the
/// tests build.
pub fn md5_entry() -> [u8; 32] {
    let mut bytes = [0xFF; 32];
    bytes[..2].copy_from_slice(&0xEBEB_u16.to_le_bytes());
    for (i, b) in bytes[16..].iter_mut().enumerate() {
        *b = i as u8;
    }
    bytes
}

/// The MD5 entry ESP-IDF writes after `slots`: their MD5 as its digest.
pub fn md5_entry_for(slots: &[[u8; 32]]) -> [u8; 32] {
    let mut md5 = Md5::new();
    for slot in slots {
        md5.update(slot);
    }
    let mut bytes = md5_entry();
    bytes[16..].copy_from_slice(&md5.finalize());
    bytes
}

/// A full-length table holding `slots` (entries, an MD5 entry, anything),
/// then erased bytes.
pub fn table_of(slots: &[[u8; 32]]) -> Vec<u8> {
    let mut table = vec![0xFF; crate::table::MAX_LEN as usize];
    for (i, slot) in slots.iter().enumerate() {
        let at = i * ENTRY_LEN as usize;
        table[at..at + 32].copy_from_slice(slot);
    }
    table
}

/// A table holding `entries`, then their MD5 entry, as ESP-IDF writes one.
pub fn table_with_md5(entries: &[Entry]) -> Vec<u8> {
    let mut slots: Vec<[u8; 32]> = entries.iter().map(entry_bytes).collect();
    slots.push(md5_entry_for(&slots));
    table_of(&slots)
}

/// A source over `size` erased bytes with `parts` written at their offsets.
pub fn flash_with(size: usize, parts: &[(u32, &[u8])]) -> Vec<u8> {
    let mut flash = vec![0xFF; size];
    for &(offset, bytes) in parts {
        let at = offset as usize;
        flash[at..at + bytes.len()].copy_from_slice(bytes);
    }
    flash
}

/// The bootloader Nix builds.
pub const BOOTLOADER: &[u8] = include_bytes!("../testdata/bootloader.bin");

/// The first 32 KiB espflash writes: its bootloader, then erased bytes.
pub const ESPFLASH_BOOTLOADER: &[u8] = include_bytes!("../testdata/espflash-bootloader.bin");

/// An app image of the firmware, made by espflash.
pub const APP: &[u8] = include_bytes!("../testdata/app.bin");

/// What running a future to completion took.
#[derive(Debug)]
pub struct Run<T> {
    pub output: T,
    /// Times the future was polled.
    pub polls: usize,
    /// Times it woke its waker.
    pub wakes: usize,
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
pub fn block_on<F: Future>(future: F) -> Run<F::Output> {
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
pub fn run<F: Future>(future: F) -> F::Output {
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
pub struct ImageBuilder {
    pub header: [u8; 24],
    pub segments: Vec<(u32, Vec<u8>)>,
}

impl ImageBuilder {
    /// An ESP32-C6 image of `segments`, with a digest appended.
    pub fn new(segments: Vec<(u32, Vec<u8>)>) -> Self {
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
    pub fn build(&self) -> Vec<u8> {
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
pub fn app_descriptor_bytes(project: &str, version: &str) -> Vec<u8> {
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
pub fn bootloader_descriptor_bytes() -> Vec<u8> {
    let mut desc = vec![0; 80];
    desc[0] = 0x50;
    desc[3] = 2;
    desc[4..8].copy_from_slice(&3_u32.to_le_bytes());
    desc[8..14].copy_from_slice(b"v5.5.2");
    desc[40..59].copy_from_slice(b"Nov  1 2026 12:00:0");
    desc
}

/// `len` bytes of data from `rng`.
pub fn data(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    rng.fill(&mut data);
    data
}

/// A small app: a mapped segment with the descriptor, then two in RAM.
pub fn app_image() -> ImageBuilder {
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
pub fn bootloader_image() -> ImageBuilder {
    let mut rng = Rng::new(0xB007);
    let mut first = bootloader_descriptor_bytes();
    first.extend(data(&mut rng, 0x400));
    ImageBuilder::new(vec![
        (0x4087_5730, first),
        (0x4086_B910, data(&mut rng, 0x804)),
    ])
}

/// The flash size of the board whose table is [`TABLE`]: 8 MiB.
pub const FLASH_SIZE: u32 = 0x80_0000;

/// The bytes of a layout record of the table at `table_offset` on a flash of
/// `flash_size` bytes holding `entries`, assembled byte by byte as the
/// format describes it.
pub fn layout_record_bytes(table_offset: u32, flash_size: u32, entries: &[Entry]) -> Vec<u8> {
    let mut record = b"EVENINGSTAR-LYT1".to_vec();
    record.extend_from_slice(&table_offset.to_le_bytes());
    record.extend_from_slice(&flash_size.to_le_bytes());
    record.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    record.extend_from_slice(&[0; 4]);
    for entry in entries {
        record.extend_from_slice(&entry_bytes(entry));
    }
    record
}

/// The layout record of the board whose table is [`TABLE`].
pub fn board_layout_record() -> Vec<u8> {
    layout_record_bytes(TABLE_OFFSET, FLASH_SIZE, &PARTITIONS)
}

/// Offset of an app's layout record in its image: after the image header,
/// the first segment's header and the 256-byte app descriptor.
pub const LAYOUT_RECORD_AT: usize = 24 + 8 + 256;

/// `image` with its checksum and digest made again to match its segments,
/// after `edit` changed the bytes of its segments in place.
pub fn restamp(image: &[u8], edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut stamped = image.to_vec();
    edit(&mut stamped);
    let end = stamped.len() - 32;
    // The checksum is the XOR of the segment data, so it changes by the XOR
    // of the bytes changed.
    let change = image[..end]
        .iter()
        .zip(&stamped[..end])
        .fold(0, |change, (old, new)| change ^ old ^ new);
    stamped[end - 1] ^= change;
    let digest = Sha256::digest(&stamped[..end]);
    stamped[end..].copy_from_slice(&digest);
    stamped
}

/// The app image `image`, whose first segment holds at least the descriptor
/// and `record`, with `record` written right after its descriptor, over what
/// the segment held there, and its checksum and digest made again.
pub fn with_layout(image: &[u8], record: &[u8]) -> Vec<u8> {
    let first_len = u32::from_le_bytes(image[28..32].try_into().unwrap()) as usize;
    assert!(
        first_len >= 256 + record.len(),
        "the first segment is too short"
    );
    restamp(image, |image| {
        image[LAYOUT_RECORD_AT..LAYOUT_RECORD_AT + record.len()].copy_from_slice(record);
    })
}

/// [`app_image`] with the board's layout record after its descriptor.
pub fn app_image_with_layout() -> ImageBuilder {
    let mut builder = app_image();
    let record = board_layout_record();
    builder.segments[0].1[256..256 + record.len()].copy_from_slice(&record);
    builder
}

/// Offset of segment `index`'s header in `image`.
pub fn segment_at(image: &[u8], index: usize) -> usize {
    let mut at = 24;
    for _ in 0..index {
        let len = u32::from_le_bytes(image[at + 4..at + 8].try_into().unwrap());
        at += 8 + len as usize;
    }
    at
}

/// The otadata partition as the board's bootloader left it on its first boot
/// after `flash-all` wrote it blank.
pub const OTADATA_FIRST_BOOT: &[u8] = include_bytes!("../testdata/otadata-first-boot.bin");

/// Length of the otadata partition in bytes.
pub const OTADATA_LEN: usize = 2 * SECTOR_LEN as usize;

/// An otadata partition holding `first` and `second` at the starts of its
/// sectors, erased elsewhere.
pub fn otadata_of(first: &[u8; 32], second: &[u8; 32]) -> Vec<u8> {
    flash_with(OTADATA_LEN, &[(0, first), (SECTOR_LEN, second)])
}

const SECTOR_LEN: u32 = crate::otadata::SECTOR_SIZE;

/// How a program a power cut stops leaves its bytes. NOR programming only
/// clears bits, so what lands is some of the bits the program clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tear {
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
    pub fn land(self, old: &mut [u8], new: &[u8]) {
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
pub struct Cut {
    pub done: usize,
    pub tear: Tear,
}

/// `write` executed on `otadata`: its sector erased, then its programs made,
/// in order.
pub fn apply(otadata: &mut [u8], write: &crate::otadata::Write) {
    apply_cut(otadata, write, None);
}

/// `write` on `otadata`, cut short as `cut` says, or whole when it is
/// `None`: its sector erased, then its programs made, in order, until the
/// cut.
pub fn apply_cut(otadata: &mut [u8], write: &crate::otadata::Write, cut: Option<Cut>) {
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
pub fn every_cut(write: &crate::otadata::Write) -> Vec<Cut> {
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
pub fn random_cut(rng: &mut Rng, write: &crate::otadata::Write) -> Cut {
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
pub struct IdfEntry {
    pub seq: u32,
    pub state: u32,
    pub crc: u32,
    bytes: [u8; 32],
}

const IDF_NEW: u32 = 0;
const IDF_PENDING_VERIFY: u32 = 1;
const IDF_VALID: u32 = 2;
const IDF_INVALID: u32 = 3;
const IDF_ABORTED: u32 = 4;

impl IdfEntry {
    pub fn read(otadata: &[u8], index: usize) -> Self {
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
    pub fn valid(&self) -> bool {
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
pub fn idf_active(entries: &[IdfEntry; 2]) -> Option<usize> {
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
pub fn idf_slot(seq: u32) -> crate::Slot {
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
pub fn boot(otadata: &mut [u8], loadable: [bool; 2]) -> Option<crate::Slot> {
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

/// Whether the ESP-IDF bootloader loads the app image at the start of
/// `partition`, as `esp_image_verify` checks it without secure boot: the
/// magic, the chip, at most 16 segments, each a whole number of words inside
/// the partition, the checksum, and the SHA-256 when one is appended. It reads
/// the format on its own, apart from the crate's parser, so the simulated
/// boots check what the crate verifies.
pub fn idf_loads(partition: &[u8]) -> bool {
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
