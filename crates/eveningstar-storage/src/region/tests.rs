#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use super::*;
use crate::layout::{
    CONFIG_0, CONFIG_1, DIAG, FACTORY, FLASH_SIZE, OTA_0, OTA_1, OTADATA, RESERVED,
};
use crate::test_support::{FLASH_LEN, Op, RamFlash, Rng, block_on, data, run};

const SECTOR: u32 = SECTOR_SIZE;

fn flash() -> RamFlash {
    RamFlash::with(FLASH_LEN, &[])
}

/// The whole flash as one region.
fn whole(flash: &RamFlash) -> Region<'_> {
    Region::new(flash, 0, FLASH_SIZE).unwrap()
}

fn erase(region: &Region<'_>, from: u32, to: u32, slot: Option<Slot>) -> Result<(), AccessError> {
    run(region.erase(from, to, slot))
}

fn program(
    region: &Region<'_>,
    offset: u32,
    bytes: &[u8],
    slot: Option<Slot>,
) -> Result<(), AccessError> {
    run(region.program(offset, bytes, slot))
}

// Making regions.

#[test]
fn a_region_lies_inside_the_flash() {
    let flash = RamFlash::with(0x4000, &[]);
    assert!(Region::new(&flash, 0, 0x4000).is_some());
    assert!(Region::new(&flash, 0x4000, 0).is_some());
    assert!(Region::new(&flash, 0x3FFF, 1).is_some());
    assert!(Region::new(&flash, 0, 0x4001).is_none());
    assert!(Region::new(&flash, 0x4000, 1).is_none());
    assert!(Region::new(&flash, 0x4001, 0).is_none());
    assert!(Region::new(&flash, 1, u32::MAX).is_none());
    assert!(Region::new(&flash, u32::MAX, 1).is_none());
    assert!(Region::new(&flash, u32::MAX, 0).is_none());
}

#[test]
fn a_clamped_region_keeps_what_lies_inside_the_flash() {
    let flash = RamFlash::with(0x4000, &[]);
    let span = |r: Region<'_>| (r.offset(), r.len());
    assert_eq!(
        span(Region::clamped(&flash, 0x1000, 0x1000)),
        (0x1000, 0x1000)
    );
    assert_eq!(
        span(Region::clamped(&flash, 0x3000, 0x2000)),
        (0x3000, 0x1000)
    );
    assert_eq!(span(Region::clamped(&flash, 0x5000, 0x1000)), (0x4000, 0));
    assert_eq!(
        span(Region::clamped(&flash, u32::MAX, u32::MAX)),
        (0x4000, 0)
    );
    assert_eq!(span(Region::clamped(&flash, 0, u32::MAX)), (0, 0x4000));
}

// Reading.

#[test]
fn reads_are_bounded_by_the_region() {
    let mut rng = Rng::new(1);
    let bytes = data(&mut rng, 0x3000);
    let flash = RamFlash::with(0x3000, &[(0, &bytes)]);
    let region = Region::new(&flash, 0x1000, 0x1000).unwrap();
    assert_eq!(region.size(), 0x1000);

    let mut buf = [0; 4];
    region.read(0, &mut buf).unwrap();
    assert_eq!(buf, bytes[0x1000..0x1004]);
    region.read(0xFFC, &mut buf).unwrap();
    assert_eq!(buf, bytes[0x1FFC..0x2000]);
    region.read(0x1000, &mut []).unwrap();

    let mut buf = [0; 4];
    for offset in [0xFFD, 0x1000, 0x1001, u32::MAX - 3, u32::MAX] {
        assert_eq!(
            region.read(offset, &mut buf),
            Err(ReadError::OutOfRange),
            "{offset:#x}"
        );
    }
    assert_eq!(region.read(0x1001, &mut []), Err(ReadError::OutOfRange));
    assert_eq!(buf, [0; 4], "nothing read on failure");
}

#[test]
fn a_failed_device_read_is_a_device_error() {
    let flash = RamFlash::with(0x2000, &[]);
    flash.fail_reads(0x1800..0x1801);
    let region = Region::new(&flash, 0x1000, 0x1000).unwrap();
    let mut buf = [0; 16];
    region.read(0x7F0, &mut buf).unwrap();
    assert_eq!(region.read(0x7F1, &mut buf), Err(ReadError::Device));
    assert_eq!(
        run(region.read_async(0, &mut [0; 0x1000])),
        Err(ReadError::Device)
    );
    assert_eq!(run(region.sha256(0, 0x1000)), Err(ReadError::Device));
}

#[test]
fn async_reads_yield_after_every_4_kib() {
    let mut rng = Rng::new(2);
    let bytes = data(&mut rng, 0x4000);
    let flash = RamFlash::with(0x4000, &[(0, &bytes)]);
    let region = whole_of(&flash);
    for (len, yields) in [
        (0, 0),
        (1, 0),
        (4095, 0),
        (4096, 1),
        (4097, 1),
        (8192, 2),
        (0x4000, 4),
    ] {
        let mut buf = vec![0; len];
        let done = block_on(region.read_async(0, &mut buf));
        assert_eq!(done.output, Ok(()));
        assert_eq!(done.wakes, yields, "{len} bytes");
        assert_eq!(buf, bytes[..len]);
    }
    let mut buf = vec![0; 0x2001];
    let done = block_on(region.read_async(0x2000, &mut buf));
    assert_eq!(done.output, Err(ReadError::OutOfRange));
    assert_eq!(done.wakes, 0, "the range is checked before any read");
    assert!(buf.iter().all(|&b| b == 0));
}

fn whole_of(flash: &RamFlash) -> Region<'_> {
    Region::clamped(flash, 0, u32::MAX)
}

#[test]
fn hashes_match_the_bytes_and_yield_after_every_4_kib() {
    let mut rng = Rng::new(3);
    let bytes = data(&mut rng, 0x5000);
    let flash = RamFlash::with(0x5000, &[(0, &bytes)]);
    let region = Region::new(&flash, 0x800, 0x4800).unwrap();
    for (offset, len, yields) in [
        (0, 0, 0),
        (0, 1, 0),
        (3, 4093, 0),
        (0, 4096, 1),
        (1, 4097, 1),
        (0x100, 0x2100, 2),
        (0, 0x4800, 4),
    ] {
        let done = block_on(region.sha256(offset, len));
        let at = 0x800 + offset as usize;
        let expected: [u8; 32] = Sha256::digest(&bytes[at..at + len as usize]).into();
        assert_eq!(done.output, Ok(expected), "{offset:#x}+{len:#x}");
        assert_eq!(done.wakes, yields, "{offset:#x}+{len:#x}");
    }
    for (offset, len) in [(0, 0x4801), (0x4800, 1), (1, u32::MAX), (u32::MAX, 2)] {
        let done = block_on(region.sha256(offset, len));
        assert_eq!(done.output, Err(ReadError::OutOfRange));
        assert_eq!(done.wakes, 0);
    }
}

// The guard.

/// Every protected range, absolute, when running from `running`.
fn protected(running: Slot) -> [(u32, u32); 4] {
    let span = |entry: Entry| (entry.offset, entry.end());
    [
        (0, BOOT_END),
        span(FACTORY),
        span(RESERVED),
        span(running.partition()),
    ]
}

#[test]
fn the_boot_chain_ends_at_the_first_partition() {
    assert_eq!(BOOT_END, 0x9000);
    assert_eq!(BOOT_END, FACTORY.offset);
}

#[test]
fn the_guard_covers_exactly_the_protected_ranges() {
    for running in Slot::ALL {
        let other = Some(running);
        for (start, end) in protected(running) {
            // Every sector inside, and ranges crossing either edge.
            for sector in (start..end).step_by(SECTOR as usize).take(64) {
                assert!(is_protected(sector, sector + 1, other), "{sector:#x}");
            }
            assert!(is_protected(end - 1, end, other));
            assert!(is_protected(start, start + 1, other));
            if start > 0 {
                assert!(is_protected(start - 1, start + 1, other));
            }
            assert!(is_protected(end - 1, end + 1, other));
            // Empty ranges touch nothing.
            assert!(!is_protected(start, start, other));
        }
        // What the handles write, and the slot OTA writes.
        let writable = [
            CONFIG_0,
            CONFIG_1,
            DIAG,
            OTADATA,
            running.other().partition(),
        ];
        for entry in writable {
            assert!(
                !is_protected(entry.offset, entry.end(), other),
                "{}",
                entry.label
            );
        }
        // Without the running slot, neither slot is written.
        assert!(is_protected(OTA_0.offset, OTA_0.offset + 4, None));
        assert!(is_protected(OTA_1.end() - 4, OTA_1.end(), None));
        for entry in [CONFIG_0, CONFIG_1, DIAG, OTADATA] {
            assert!(
                !is_protected(entry.offset, entry.end(), None),
                "{}",
                entry.label
            );
        }
    }
}

#[test]
fn erases_and_programs_of_protected_ranges_are_refused_untouched() {
    for running in Slot::ALL {
        let flash = flash();
        let region = whole(&flash);
        let other = Some(running);
        for (start, end) in protected(running) {
            // The whole range, a sector at each end, and a range crossing in.
            let mut ranges = vec![(start, end), (start, start + SECTOR), (end - SECTOR, end)];
            if start >= SECTOR {
                ranges.push((start - SECTOR, start + SECTOR));
            }
            if end + SECTOR <= FLASH_SIZE {
                ranges.push((end - SECTOR, end + SECTOR));
            }
            for (from, to) in ranges {
                assert_eq!(
                    erase(&region, from, to, other),
                    Err(AccessError::Protected),
                    "erase {from:#x}..{to:#x}, running {running:?}"
                );
            }
            for at in [start, end - 4] {
                assert_eq!(
                    program(&region, at, &[0; 4], other),
                    Err(AccessError::Protected),
                    "program {at:#x}, running {running:?}"
                );
            }
            if start >= 4 {
                assert_eq!(
                    program(&region, start - 4, &[0; 8], other),
                    Err(AccessError::Protected)
                );
            }
        }
        assert_eq!(flash.ops(), [], "nothing was erased or programmed");
        assert!(flash.bytes().iter().all(|&b| b == 0xFF));
    }
}

#[test]
fn the_running_slot_is_refused_and_the_other_written_only_when_running_is_known() {
    for running in Slot::ALL {
        let flash = flash();
        let region = whole(&flash);
        let other = running.other().partition();
        let ours = running.partition();
        let known = Some(running);
        assert_eq!(
            erase(&region, ours.offset, ours.offset + SECTOR, known),
            Err(AccessError::Protected)
        );
        assert_eq!(
            program(&region, ours.end() - 4, &[0; 4], known),
            Err(AccessError::Protected)
        );
        // Without the running slot, neither slot is written.
        for slot in [ours, other] {
            assert_eq!(
                erase(&region, slot.offset, slot.offset + SECTOR, None),
                Err(AccessError::Protected)
            );
        }
        erase(&region, other.offset, other.offset + SECTOR, known).unwrap();
        program(&region, other.end() - 4, &[0; 4], known).unwrap();
        // A range from the other slot into the running one is refused whole.
        let (low, high) = match running {
            Slot::Ota0 => (ours, other),
            Slot::Ota1 => (other, ours),
        };
        assert_eq!(
            erase(&region, low.end() - SECTOR, high.offset + SECTOR, known),
            Err(AccessError::Protected)
        );
        assert_eq!(
            flash.ops(),
            [Op::Erase(other.offset), Op::Program(other.end() - 4, 4)]
        );
    }
}

#[test]
fn a_region_inside_a_partition_is_guarded_by_absolute_address() {
    let flash = flash();
    // A region straddling the end of diag and the start of reserved.
    let region = Region::new(&flash, DIAG.end() - SECTOR, 2 * SECTOR).unwrap();
    run(region.erase(0, SECTOR, None)).unwrap();
    assert_eq!(
        erase(&region, SECTOR, 2 * SECTOR, None),
        Err(AccessError::Protected)
    );
    assert_eq!(
        erase(&region, 0, 2 * SECTOR, None),
        Err(AccessError::Protected)
    );
    assert_eq!(
        program(&region, SECTOR, &[0; 4], None),
        Err(AccessError::Protected)
    );
    program(&region, SECTOR - 4, &[0; 4], None).unwrap();
    assert_eq!(
        flash.ops(),
        [
            Op::Erase(DIAG.end() - SECTOR),
            Op::Program(DIAG.end() - 4, 4)
        ]
    );
}

// Erasing.

#[test]
fn erases_are_bounded_and_aligned() {
    let flash = flash();
    let region = Region::of_entry(&flash, CONFIG_0);
    let len = CONFIG_0.size;
    for (from, to) in [
        (SECTOR, 0),
        (0, len + SECTOR),
        (len, len + SECTOR),
        (u32::MAX, u32::MAX),
    ] {
        assert_eq!(
            erase(&region, from, to, None),
            Err(AccessError::OutOfRange),
            "{from:#x}..{to:#x}"
        );
    }
    for (from, to) in [
        (1, SECTOR),
        (0, SECTOR - 1),
        (0, 1),
        (SECTOR + 4, 2 * SECTOR),
        (len - 1, len),
    ] {
        assert_eq!(
            erase(&region, from, to, None),
            Err(AccessError::NotAligned),
            "{from:#x}..{to:#x}"
        );
    }
    // Empty ranges, at either end, erase nothing.
    erase(&region, 0, 0, None).unwrap();
    erase(&region, len, len, None).unwrap();
    assert_eq!(flash.ops(), []);
    // The last sector, and the whole partition.
    erase(&region, len - SECTOR, len, None).unwrap();
    erase(&region, 0, len, None).unwrap();
    let mut expected = vec![Op::Erase(CONFIG_0.end() - SECTOR)];
    expected.extend(
        (CONFIG_0.offset..CONFIG_0.end())
            .step_by(SECTOR as usize)
            .map(Op::Erase),
    );
    assert_eq!(flash.ops(), expected);
}

#[test]
fn an_erase_sets_its_sectors_and_nothing_else_to_ff() {
    let flash = RamFlash::with(
        FLASH_LEN,
        &[(DIAG.offset - 16, &[0; 32 + 3 * SECTOR as usize])],
    );
    let region = Region::of_entry(&flash, DIAG);
    erase(&region, SECTOR, 2 * SECTOR, None).unwrap();
    assert!(
        flash
            .at(DIAG.offset - 16, 16 + SECTOR as usize)
            .iter()
            .all(|&b| b == 0)
    );
    assert!(
        flash
            .at(DIAG.offset + SECTOR, SECTOR as usize)
            .iter()
            .all(|&b| b == 0xFF)
    );
    assert!(
        flash
            .at(DIAG.offset + 2 * SECTOR, SECTOR as usize)
            .iter()
            .all(|&b| b == 0)
    );
}

#[test]
fn an_erase_yields_after_every_sector() {
    let flash = flash();
    let region = Region::of_entry(&flash, DIAG);
    for sectors in [0, 1, 2, 5] {
        let done = block_on(region.erase(0, sectors * SECTOR, None));
        assert_eq!(done.output, Ok(()));
        assert_eq!(done.wakes, sectors as usize);
    }
    let done = block_on(region.erase(0, 1, None));
    assert_eq!(done.output, Err(AccessError::NotAligned));
    assert_eq!(done.wakes, 0);
}

#[test]
fn a_failing_erase_stops_at_the_failed_sector() {
    let flash = flash();
    let region = Region::of_entry(&flash, DIAG);
    flash.fail_after(2);
    let done = block_on(region.erase(0, 4 * SECTOR, None));
    assert_eq!(done.output, Err(AccessError::Device));
    assert_eq!(done.wakes, 2);
    assert_eq!(
        flash.ops(),
        [Op::Erase(DIAG.offset), Op::Erase(DIAG.offset + SECTOR)]
    );
}

// Programming.

#[test]
fn programs_are_aligned_and_bounded() {
    let flash = flash();
    let region = Region::of_entry(&flash, CONFIG_1);
    let len = CONFIG_1.size;
    for (offset, n) in [(1, 4), (2, 4), (0, 3), (0, 5), (4, 1), (len - 3, 3)] {
        assert_eq!(
            program(&region, offset, &vec![0; n], None),
            Err(AccessError::NotAligned),
            "{n} bytes at {offset:#x}"
        );
    }
    for (offset, n) in [
        (len - 4, 8),
        (len, 4),
        (len + 4, 0),
        (u32::MAX - 3, 4),
        (u32::MAX - 3, 8),
    ] {
        assert_eq!(
            program(&region, offset, &vec![0; n], None),
            Err(AccessError::OutOfRange),
            "{n} bytes at {offset:#x}"
        );
    }
    program(&region, len, &[], None).unwrap();
    program(&region, 0, &[], None).unwrap();
    assert_eq!(flash.ops(), []);
    program(&region, len - 4, &[0; 4], None).unwrap();
    program(&region, 0, &[0; 4], None).unwrap();
    assert_eq!(
        flash.ops(),
        [
            Op::Program(CONFIG_1.end() - 4, 4),
            Op::Program(CONFIG_1.offset, 4)
        ]
    );
}

#[test]
fn a_region_that_isnt_word_aligned_refuses_programs() {
    let flash = flash();
    let region = Region::new(&flash, DIAG.offset + 2, 64).unwrap();
    assert_eq!(
        program(&region, 0, &[0; 4], None),
        Err(AccessError::NotAligned)
    );
    assert_eq!(erase(&region, 0, 0, None), Err(AccessError::NotAligned));
    assert_eq!(flash.ops(), []);
}

#[test]
fn programming_only_clears_bits() {
    let flash = flash();
    let region = Region::of_entry(&flash, DIAG);
    program(&region, 8, &[0xF0, 0x0F, 0xAA, 0xFF], None).unwrap();
    program(&region, 8, &[0x3C, 0xFF, 0x55, 0x81], None).unwrap();
    assert_eq!(
        flash.at(DIAG.offset + 4, 12),
        [
            0xFF, 0xFF, 0xFF, 0xFF, 0x30, 0x0F, 0x00, 0x81, 0xFF, 0xFF, 0xFF, 0xFF
        ]
    );
    // Programming ones changes nothing; an erase brings the ones back.
    program(&region, 8, &[0xFF; 4], None).unwrap();
    assert_eq!(flash.at(DIAG.offset + 8, 4), [0x30, 0x0F, 0x00, 0x81]);
    erase(&region, 0, SECTOR, None).unwrap();
    assert_eq!(flash.at(DIAG.offset + 8, 4), [0xFF; 4]);
}

#[test]
fn a_program_runs_in_steps_of_at_most_4_kib_and_yields_after_each() {
    let mut rng = Rng::new(4);
    for (len, steps) in [(4, 1), (4096, 1), (4100, 2), (12_288, 3), (12_292, 4)] {
        let flash = flash();
        let region = Region::of_entry(&flash, DIAG);
        let bytes = data(&mut rng, len);
        let done = block_on(region.program(4, &bytes, None));
        assert_eq!(done.output, Ok(()));
        assert_eq!(done.wakes, steps, "{len} bytes");
        let ops = flash.ops();
        assert_eq!(ops.len(), steps);
        let mut at = DIAG.offset + 4;
        for (i, op) in ops.iter().enumerate() {
            let step = (len as u32 - (at - DIAG.offset - 4)).min(4096);
            assert_eq!(*op, Op::Program(at, step), "step {i} of {len} bytes");
            at += step;
        }
        assert_eq!(flash.at(DIAG.offset + 4, len), bytes);
    }
}

#[test]
fn a_failing_program_stops_at_the_failed_step() {
    let flash = flash();
    let region = Region::of_entry(&flash, DIAG);
    flash.fail_after(1);
    let done = block_on(region.program(0, &[0; 8192], None));
    assert_eq!(done.output, Err(AccessError::Device));
    assert_eq!(done.wakes, 1);
    assert_eq!(flash.ops(), [Op::Program(DIAG.offset, 4096)]);
    assert!(
        flash
            .at(DIAG.offset + 4096, 4096)
            .iter()
            .all(|&b| b == 0xFF)
    );
}

#[test]
fn access_errors_map_to_nor_flash_kinds() {
    assert_eq!(
        AccessError::OutOfRange.kind(),
        NorFlashErrorKind::OutOfBounds
    );
    assert_eq!(
        AccessError::NotAligned.kind(),
        NorFlashErrorKind::NotAligned
    );
    assert_eq!(AccessError::Protected.kind(), NorFlashErrorKind::Other);
    assert_eq!(AccessError::Device.kind(), NorFlashErrorKind::Other);
    assert_eq!(
        AccessError::from(ReadError::OutOfRange),
        AccessError::OutOfRange
    );
    assert_eq!(AccessError::from(ReadError::Device), AccessError::Device);
    assert_eq!(AccessError::from(ReadError::Stale), AccessError::Device);
}

#[test]
fn the_debug_output_shows_the_span_only() {
    let flash = RamFlash::with(0x2000, &[(0, b"secret!!")]);
    let region = Region::new(&flash, 0, 0x2000).unwrap();
    let debug = format!("{region:?}");
    assert_eq!(debug, "Region { offset: 0, len: 8192, .. }");
}

impl<'f> Region<'f> {
    /// The partition `entry`, which lies inside `flash`.
    fn of_entry(flash: &'f dyn Flash, entry: Entry) -> Self {
        Self::new(flash, entry.offset, entry.size).unwrap()
    }
}
