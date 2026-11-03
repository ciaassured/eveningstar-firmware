#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

extern crate alloc;

use alloc::boxed::Box;

use sha2::{Digest as _, Sha256};

use super::*;
use crate::Entry;
use crate::layout::{CONFIG_0, DIAG, FACTORY, FLASH_SIZE, OTA_0, OTA_1, OTADATA, TABLE_OFFSET};
use crate::table::Match;
use crate::test_support::{
    APP, BOOTLOADER, ESPFLASH_BOOTLOADER, ESPFLASH_TABLE, FLASH_LEN, ImageBuilder, PARTITIONS,
    RamFlash, Rng, TABLE, block_on, bootloader_descriptor_bytes, entry_bytes, espflash_board,
    flash_with, lend, md5_entry_for, nix_board, run, table_of, table_with_md5,
};
use eveningstar_esp_formats::{TYPE_APP, TYPE_DATA};

const N: usize = layout::PARTITIONS.len();

/// A state table for one `split`, leaked so it outlives the handles.
fn state() -> &'static mut State {
    Box::leak(Box::new(State::new()))
}

/// `flash`, leaked so the handles over it can outlive the statement.
fn board(flash: RamFlash) -> &'static RamFlash {
    Box::leak(Box::new(flash))
}

/// `flash` with `parts` written over it.
fn patched(flash: &RamFlash, parts: &[(u32, &[u8])]) -> RamFlash {
    let bytes = flash.bytes();
    let mut all = vec![(0, &bytes[..])];
    all.extend_from_slice(parts);
    RamFlash::new(flash_with(bytes.len(), &all))
}

/// The Nix board with `table` at the compiled offset in place of the Nix
/// table.
fn with_table(table: &[u8]) -> RamFlash {
    let mut erased = vec![0xFF; table::MAX_LEN as usize];
    erased[..table.len()].copy_from_slice(table);
    patched(&nix_board(), &[(TABLE_OFFSET, &erased)])
}

/// `flash` with espflash's bootloader, which isn't known, over the start of
/// the Nix one: its image alone, without the erased bytes the fixture holds
/// after it.
fn unknown_bootloader(flash: &RamFlash) -> RamFlash {
    let len = run(image::verify_bootloader(ESPFLASH_BOOTLOADER))
        .unwrap()
        .size() as usize;
    patched(flash, &[(0, &ESPFLASH_BOOTLOADER[..len])])
}

/// `bootloader` with `edit` made to it and its digest made again to match,
/// as a build of another bootloader would leave it.
fn restamped(bootloader: &[u8], edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut image = bootloader.to_vec();
    edit(&mut image);
    let end = image.len() - 32;
    let digest = Sha256::digest(&image[..end]);
    image[end..].copy_from_slice(&digest);
    image
}

/// A known list of one bootloader, `image`, read at `table_offset`.
fn known_list(image: &[u8], table_offset: u32, rollback: bool) -> &'static [Known] {
    let verified = run(image::verify_bootloader(image)).unwrap();
    Box::leak(Box::new([Known {
        digest: *verified.digest(),
        name: "test",
        table_offset,
        rollback,
    }]))
}

/// The checks every `Storage` passes, whatever the flash.
fn consistent(storage: &Storage<'_>) {
    let report = &storage.report;
    let capacity = report.capacity;
    let inside = |span: Span| assert!(Span::new(0, capacity).contains(span), "{span:?}");

    // The tier agrees with the steps and with the handles.
    let (tier, failed) = report.evaluate();
    assert_eq!((report.tier, report.failed), (tier, failed), "{report:?}");
    assert_eq!(report.failed.is_none(), report.tier == Tier::Boot);
    if let Some(step) = report.failed {
        assert!(!report.passed(step));
        assert!(
            Step::ALL
                .iter()
                .take_while(|s| **s != step)
                .all(|s| report.passed(*s))
        );
    }
    match (&storage.handles, report.tier) {
        (Handles::Unverified(_), Tier::Unverified)
        | (Handles::Layout(_), Tier::Layout)
        | (Handles::Boot(..), Tier::Boot) => {}
        (handles, tier) => panic!("{tier:?} with {handles:?}"),
    }
    assert_eq!(
        report.slot.is_some(),
        report.tier != Tier::Unverified,
        "{report:?}"
    );

    // The readers every tier has.
    let table_end = report.table.offset.unwrap_or(TABLE_OFFSET).min(capacity);
    assert_eq!(storage.bootloader.span(), Span::new(0, table_end));
    assert_eq!(
        storage.partition_table.as_ref().map(Reader::span),
        (capacity >= TABLE_OFFSET + SECTOR_SIZE).then_some(Span::new(TABLE_OFFSET, SECTOR_SIZE))
    );

    match &storage.handles {
        Handles::Unverified(located) => {
            let spans = [
                located.table.as_ref().map(Reader::span),
                located.app.as_ref().map(Reader::span),
                located.otadata.as_ref().map(Reader::span),
            ];
            for span in spans.into_iter().flatten() {
                inside(span);
                assert!(!span.is_empty());
                assert!(!is_sensitive(span), "{span:?}");
            }
            if located.table.is_some() {
                assert_eq!(report.table.source, Some(TableSource::Searched));
                assert!(report.table.partitions.is_some());
            }
            if let Some(app) = &located.app {
                assert_eq!(app.span().offset, report.booted);
            }
            if located.otadata.is_some() {
                assert!(report.table.partitions.is_some());
            }
        }
        Handles::Layout(handles) => layout_consistent(handles, report.slot.unwrap()),
        Handles::Boot(handles, writers) => {
            let slot = report.slot.unwrap();
            layout_consistent(handles, slot);
            assert_eq!(writers.slot(), slot);
            assert_eq!(
                writers.alternate_app.span(),
                Span::of(&slot.other().partition())
            );
            assert_eq!(writers.otadata.span(), Span::of(&OTADATA));
        }
    }
}

/// The layout tier's handles are the layout's regions for `slot`.
fn layout_consistent(handles: &LayoutHandles<'_>, slot: Slot) {
    assert_eq!(handles.current_app.span(), Span::of(&slot.partition()));
    assert_eq!(
        handles.alternate_app.span(),
        Span::of(&slot.other().partition())
    );
    assert_eq!(handles.otadata.span(), Span::of(&OTADATA));
    assert_eq!(handles.current_config.span(), Span::of(&slot.config()));
    assert_eq!(
        handles.alternate_config.span(),
        Span::of(&slot.other().config())
    );
    assert_eq!(handles.diag.span(), Span::of(&DIAG));
}

/// The chain run on `flash` with `bootloaders` as the known list, for the
/// app booted at `booted`.
fn split_known<'f>(flash: &'f RamFlash, booted: u32, bootloaders: &'static [Known]) -> Storage<'f> {
    let storage = run(split_with(lend(flash), state(), booted, bootloaders));
    consistent(&storage);
    assert_eq!(flash.ops(), [], "split never writes");
    storage
}

/// The chain run on `flash` for the app booted at `booted`.
fn split_on(flash: &RamFlash, booted: u32) -> Storage<'_> {
    split_known(flash, booted, known::BOOTLOADERS)
}

/// The unverified tier's readers of `storage`.
fn located<'s, 'f>(storage: &'s Storage<'f>) -> &'s Located<'f> {
    match &storage.handles {
        Handles::Unverified(located) => located,
        handles => panic!("{handles:?}"),
    }
}

/// The spans of what `storage` located: the table, the app and the
/// otadata.
fn located_spans(storage: &Storage<'_>) -> [Option<Span>; 3] {
    let located = located(storage);
    [
        located.table.as_ref().map(Reader::span),
        located.app.as_ref().map(Reader::span),
        located.otadata.as_ref().map(Reader::span),
    ]
}

/// The partitions of `storage`'s located table.
fn partitions(storage: &Storage<'_>) -> Partitions {
    storage.report.table.partitions.unwrap()
}

// The boot tier.

#[test]
fn a_board_flashed_by_nix_reaches_the_boot_tier() {
    let flash = nix_board();
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.tier, Tier::Boot);
    assert_eq!(report.failed, None);
    assert!(Step::ALL.iter().all(|step| report.passed(*step)));
    assert_eq!(report.slot, Some(Slot::Ota0));
    assert_eq!(report.capacity, FLASH_SIZE);
    assert_eq!(report.booted, OTA_0.offset);

    // Step 1.
    let bootloader = report.bootloader;
    assert_eq!(bootloader.flash_size_code, Some(3));
    assert_eq!(bootloader.chip_size(), Some(FLASH_SIZE));
    assert_eq!(bootloader.header.unwrap().segment_count, 3);
    let id = bootloader.image.unwrap();
    assert_eq!(id.size as usize, BOOTLOADER.len());
    assert_eq!(
        known::Hex(&id.digest).to_string(),
        "6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421"
    );
    let known = bootloader.known.unwrap();
    assert_eq!(known.name, "ESP-IDF v5.5.2, app rollback");
    assert_eq!(bootloader.fits, Some(true));

    // Steps 2 to 4.
    let table = report.table;
    assert_eq!(table.source, Some(TableSource::KnownBootloader));
    assert_eq!(table.offset, Some(TABLE_OFFSET));
    assert_eq!(table.search, None, "a known bootloader needs no search");
    let verified = table.verdict.unwrap().unwrap();
    assert_eq!(verified.end, N + 1);
    assert_eq!(verified.md5.unwrap().index, N);
    let partitions = table.partitions.unwrap();
    assert_eq!(partitions.count, N);
    assert!(!partitions.md5_read_as_partition);
    assert!(partitions.comparison.matches());
    assert_eq!(partitions.booted, Some(Span::of(&OTA_0)));
    assert_eq!(partitions.otadata, Some(Span::of(&OTADATA)));

    // The handles.
    assert_eq!(storage.bootloader.span(), Span::new(0, TABLE_OFFSET));
    let Handles::Boot(handles, writers) = &storage.handles else {
        panic!("{:?}", storage.handles);
    };
    layout_consistent(handles, Slot::Ota0);
    assert_eq!(writers.slot(), Slot::Ota0);
    assert_eq!(writers.alternate_app.span(), Span::of(&OTA_1));
}

#[test]
fn a_board_running_from_ota_1_hands_out_the_other_slot_s_regions() {
    let flash = patched(&nix_board(), &[(OTA_1.offset, APP)]);
    let storage = split_on(&flash, OTA_1.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
    assert_eq!(storage.report.slot, Some(Slot::Ota1));
    assert_eq!(partitions(&storage).booted, Some(Span::of(&OTA_1)));
    let Handles::Boot(handles, writers) = &storage.handles else {
        panic!("{:?}", storage.handles);
    };
    assert_eq!(handles.current_app.span(), Span::of(&OTA_1));
    assert_eq!(handles.alternate_app.span(), Span::of(&OTA_0));
    assert_eq!(handles.current_config.span(), Span::of(&layout::CONFIG_1));
    assert_eq!(handles.alternate_config.span(), Span::of(&CONFIG_0));
    assert_eq!(writers.alternate_app.span(), Span::of(&OTA_0));
    assert_eq!(writers.slot(), Slot::Ota1);
}

#[test]
fn the_bootloader_is_hashed_with_yields() {
    let flash = nix_board();
    let done = block_on(split(lend(&flash), state(), OTA_0.offset));
    // 22 KiB hashed, a yield every 1 KiB.
    assert_eq!(
        done.wakes,
        (BOOTLOADER.len() - 32) / image::YIELD_INTERVAL as usize
    );
    assert_eq!(done.output.report.tier, Tier::Boot);
}

#[test]
fn a_known_bootloader_ignores_other_tables() {
    // A table left over below the compiled one, with the booted app.
    let flash = patched(&nix_board(), &[(0x6000, ESPFLASH_TABLE)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
    assert_eq!(storage.report.table.search, None);
}

#[test]
fn a_table_without_an_md5_entry_reaches_the_boot_tier() {
    let table = table_of(&layout::PARTITIONS.map(|entry| entry_bytes(&entry)));
    let storage = split_on(board(with_table(&table)), OTA_0.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
    let verified = storage.report.table.verdict.unwrap().unwrap();
    assert_eq!(verified, Verified { end: N, md5: None });
}

#[test]
fn the_end_entry_s_last_28_bytes_are_never_read() {
    let mut table = TABLE.to_vec();
    table[(N + 1) * 32 + 4..(N + 2) * 32].fill(0x00);
    let storage = split_on(board(with_table(&table)), OTA_0.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
}

#[test]
fn reordered_entries_still_match_the_layout() {
    let mut entries = layout::PARTITIONS.to_vec();
    entries.reverse();
    let storage = split_on(board(with_table(&table_with_md5(&entries))), OTA_0.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
}

// The layout tier.

#[test]
fn an_unknown_bootloader_with_the_layout_s_table_alone_gives_the_layout_tier() {
    let flash = unknown_bootloader(&nix_board());
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.tier, Tier::Layout);
    assert_eq!(report.failed, Some(Step::Bootloader));
    assert!(report.bootloader.image.is_ok());
    assert_eq!(report.bootloader.known, None);
    assert_eq!(report.table.source, Some(TableSource::Searched));
    assert_eq!(report.table.search, Some(Found::One(TABLE_OFFSET)));
    assert_eq!(report.table.offset, Some(TABLE_OFFSET));
    assert!(matches!(storage.handles, Handles::Layout(_)));
}

#[test]
fn a_corrupt_bootloader_is_unknown_and_its_table_is_searched() {
    let mut bootloader = BOOTLOADER.to_vec();
    bootloader[0x100] ^= 0x01;
    let flash = patched(&nix_board(), &[(0, &bootloader)]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert!(matches!(
        report.bootloader.image,
        Err(ImageError::Checksum { .. } | ImageError::DigestMismatch)
    ));
    assert_eq!(report.bootloader.fits, None);
    // Its header still gives the chip size the search judges tables with.
    assert_eq!(report.bootloader.chip_size(), Some(FLASH_SIZE));
    assert_eq!(report.table.source, Some(TableSource::Searched));
    assert_eq!(report.tier, Tier::Layout);
}

#[test]
fn a_known_bootloader_without_rollback_gives_the_layout_tier() {
    let flash = nix_board();
    let bootloaders = known_list(BOOTLOADER, TABLE_OFFSET, false);
    let storage = split_known(&flash, OTA_0.offset, bootloaders);
    let report = storage.report;
    assert_eq!(report.table.source, Some(TableSource::KnownBootloader));
    assert_eq!(report.tier, Tier::Layout);
    assert_eq!(report.failed, Some(Step::Bootloader));
    assert!(matches!(storage.handles, Handles::Layout(_)));
}

// The unverified tier.

#[test]
fn a_board_flashed_by_cargo_run_is_unverified_with_espflash_s_table() {
    let flash = espflash_board();
    let storage = split_on(&flash, 0x1_0000);
    let report = storage.report;
    assert_eq!(report.tier, Tier::Unverified);
    assert_eq!(report.failed, Some(Step::Bootloader));
    assert_eq!(report.table.search, Some(Found::One(TABLE_OFFSET)));
    assert!(report.passed(Step::TableOffset));
    assert!(report.passed(Step::Table));
    assert!(!report.passed(Step::Layout));
    let partitions = partitions(&storage);
    assert_eq!(partitions.count, 3);
    assert_eq!(partitions.comparison.extra, 2);
    assert!(matches!(
        partitions.comparison.expected[0],
        Match::Differs(_)
    ));
    assert_eq!(partitions.comparison.expected[1..], [Match::Missing; N - 1]);
    // Its factory app runs to the end of the flash, over the configurations
    // and diag, so it has no reader; the table has one.
    assert_eq!(partitions.booted, Some(Span::new(0x1_0000, 0x7F_0000)));
    assert_eq!(
        located_spans(&storage),
        [Some(Span::new(TABLE_OFFSET, SECTOR_SIZE)), None, None]
    );
}

#[test]
fn a_table_at_another_offset_under_a_known_bootloader_is_unverified() {
    // The known bootloader reads its table at 0xC000, where the layout's
    // table lies; the compiled offset holds nothing.
    let flash = patched(&with_table(&[]), &[(0xC000, TABLE)]);
    let bootloaders = known_list(BOOTLOADER, 0xC000, true);
    let storage = split_known(&flash, OTA_0.offset, bootloaders);
    let report = storage.report;
    assert_eq!(report.table.source, Some(TableSource::KnownBootloader));
    assert_eq!(report.table.offset, Some(0xC000));
    assert!(partitions(&storage).comparison.matches());
    assert!(!report.passed(Step::Layout));
    assert_eq!(report.failed, Some(Step::Layout));
    assert_eq!(report.tier, Tier::Unverified);
    // The bootloader runs up to the table it reads; the table's reader stays
    // at the compiled offset, and the table found by the known bootloader
    // has no reader of its own.
    assert_eq!(storage.bootloader.span(), Span::new(0, 0xC000));
    assert_eq!(
        storage.partition_table.as_ref().map(Reader::span),
        Some(Span::new(TABLE_OFFSET, SECTOR_SIZE))
    );
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::of(&OTA_0)), Some(Span::of(&OTADATA))]
    );
}

#[test]
fn a_moved_table_is_found_and_unverified() {
    for offset in [0x6000, 0xC000, 0xF000] {
        let flash = unknown_bootloader(&patched(&with_table(&[]), &[(offset, TABLE)]));
        let storage = split_on(&flash, OTA_0.offset);
        let report = storage.report;
        assert_eq!(report.table.search, Some(Found::One(offset)));
        assert_eq!(report.table.source, Some(TableSource::Searched));
        assert_eq!(report.failed, Some(Step::Bootloader));
        assert!(!report.passed(Step::Layout));
        assert_eq!(report.tier, Tier::Unverified);
        assert_eq!(storage.bootloader.span(), Span::new(0, offset));
        assert_eq!(
            located_spans(&storage),
            [
                Some(Span::new(offset, SECTOR_SIZE)),
                Some(Span::of(&OTA_0)),
                Some(Span::of(&OTADATA))
            ],
            "{offset:#x}"
        );
    }
}

#[test]
fn an_unknown_bootloader_with_a_leftover_second_table_is_unverified() {
    let flash = unknown_bootloader(&patched(&nix_board(), &[(0x6000, ESPFLASH_TABLE)]));
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(
        report.table.search,
        Some(Found::Several(0x6000, TABLE_OFFSET))
    );
    assert_eq!(report.table.source, None);
    assert_eq!(report.table.offset, None);
    assert_eq!(report.table.verdict, None);
    assert!(!report.passed(Step::TableOffset));
    assert_eq!(report.tier, Tier::Unverified);
    // No table is located, so the app is the image at the booted offset.
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::new(OTA_0.offset, APP.len() as u32)), None]
    );
    assert_eq!(storage.bootloader.span(), Span::new(0, TABLE_OFFSET));
}

#[test]
fn an_mmu_located_app_with_no_table_spans_its_image() {
    let flash = unknown_bootloader(&with_table(&[]));
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.table.search, Some(Found::Nothing));
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::new(OTA_0.offset, APP.len() as u32)), None]
    );
    // An image at the end of the flash is cut to nothing past it: one whose
    // headers run past the end has no reader.
    let at = FLASH_SIZE - 0x1_0000;
    let flash = unknown_bootloader(&patched(&with_table(&[]), &[(at, &APP[..0x1_0000])]));
    let storage = split_on(&flash, at);
    assert_eq!(located(&storage).app.as_ref().map(Reader::span), None);
    // Nor has anything that isn't an image.
    let storage = split_on(&flash, 0x2_0000);
    assert_eq!(located_spans(&storage), [None, None, None]);
}

#[test]
fn a_booted_offset_past_the_flash_has_no_search_and_no_app() {
    let flash = unknown_bootloader(&nix_board());
    let storage = split_on(&flash, u32::MAX);
    let report = storage.report;
    assert_eq!(report.table.search, None);
    assert!(!report.passed(Step::BootedSlot));
    assert_eq!(located_spans(&storage), [None, None, None]);
}

#[test]
fn a_bad_md5_under_a_known_bootloader_is_unverified() {
    let mut table = TABLE.to_vec();
    table[N * 32 + 16] ^= 0x01;
    let storage = split_on(board(with_table(&table)), OTA_0.offset);
    let report = storage.report;
    assert!(matches!(
        report.table.verdict,
        Some(Err(TableError::Md5 { index: N, .. }))
    ));
    assert_eq!(report.table.partitions, None);
    assert_eq!(report.failed, Some(Step::Table));
    assert_eq!(report.tier, Tier::Unverified);
    // The located table isn't valid, so the app is the booted image.
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::new(OTA_0.offset, APP.len() as u32)), None]
    );
}

#[test]
fn entries_between_the_md5_entry_and_the_end_entry_dont_match_the_layout() {
    let mut slots: Vec<[u8; 32]> = layout::PARTITIONS[..N - 1]
        .iter()
        .map(entry_bytes)
        .collect();
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&layout::PARTITIONS[N - 1]));
    let storage = split_on(board(with_table(&table_of(&slots))), OTA_0.offset);
    let partitions = partitions(&storage);
    assert_eq!(partitions.count, N);
    assert!(partitions.md5_read_as_partition);
    let mut expected = [Match::Matches; N];
    expected[N - 1] = Match::Missing;
    assert_eq!(partitions.comparison.expected, expected);
    assert_eq!(partitions.comparison.extra, 1);
    assert_eq!(storage.report.failed, Some(Step::Layout));
    assert_eq!(storage.report.tier, Tier::Unverified);
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::of(&OTA_0)), Some(Span::of(&OTADATA))]
    );
}

#[test]
fn an_md5_entry_and_an_end_entry_hold_no_partitions() {
    let storage = split_on(
        board(with_table(&table_of(&[md5_entry_for(&[])]))),
        OTA_0.offset,
    );
    let partitions = partitions(&storage);
    assert_eq!(partitions.count, 0);
    assert_eq!(partitions.comparison.expected, [Match::Missing; N]);
    assert_eq!(partitions.booted, None);
    assert_eq!(storage.report.failed, Some(Step::Layout));
    // A valid table without the booted app locates no app.
    assert_eq!(located_spans(&storage), [None, None, None]);
}

#[test]
fn of_duplicate_otadata_entries_the_last_is_located() {
    let mut entries = layout::PARTITIONS.to_vec();
    entries.push(Entry {
        label: "otadata",
        offset: 0xD000,
        ..OTADATA
    });
    let storage = split_on(board(with_table(&table_with_md5(&entries))), OTA_0.offset);
    let partitions = partitions(&storage);
    assert_eq!(partitions.otadata, Some(Span::new(0xD000, OTADATA.size)));
    assert_eq!(partitions.comparison.extra, 1);
    assert_eq!(storage.report.tier, Tier::Unverified);
    assert_eq!(
        located(&storage).otadata.as_ref().map(Reader::span),
        Some(Span::new(0xD000, OTADATA.size))
    );
}

#[test]
fn of_duplicate_app_entries_the_last_is_located() {
    let mut entries = layout::PARTITIONS.to_vec();
    entries.push(Entry {
        label: "ota_0b",
        size: 0x10_0000,
        ..OTA_0
    });
    let storage = split_on(board(with_table(&table_with_md5(&entries))), OTA_0.offset);
    assert_eq!(
        located(&storage).app.as_ref().map(Reader::span),
        Some(Span::new(OTA_0.offset, 0x10_0000))
    );
}

#[test]
fn a_partition_past_the_header_s_flash_size_is_invalid_inside_a_larger_chip() {
    // A known bootloader whose header names 4 MiB, on an 8 MiB chip.
    let bootloader = restamped(BOOTLOADER, |image| image[3] = 0x20);
    let flash = patched(&nix_board(), &[(0, &bootloader)]);
    let bootloaders = known_list(&bootloader, TABLE_OFFSET, true);
    let storage = split_known(&flash, OTA_0.offset, bootloaders);
    let report = storage.report;
    assert_eq!(report.bootloader.chip_size(), Some(0x40_0000));
    assert_eq!(report.capacity, FLASH_SIZE);
    assert_eq!(
        report.table.verdict,
        Some(Err(TableError::PastChip {
            index: 3,
            offset: OTA_1.offset,
            size: OTA_1.size
        }))
    );
    assert_eq!(report.failed, Some(Step::Table));
    assert_eq!(report.tier, Tier::Unverified);
}

#[test]
fn an_unknown_flash_size_code_is_2_mib() {
    let bootloader = restamped(BOOTLOADER, |image| image[3] = 0xF0);
    let flash = patched(&nix_board(), &[(0, &bootloader)]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.bootloader.flash_size_code, Some(0xF));
    assert_eq!(report.bootloader.chip_size(), Some(0x20_0000));
    // Unknown, so searched, and no table fits 2 MiB.
    assert_eq!(report.table.search, Some(Found::Nothing));
    assert_eq!(report.tier, Tier::Unverified);
}

#[test]
fn an_erased_bootloader_s_header_still_gives_a_flash_size() {
    let flash = patched(&nix_board(), &[(0, &vec![0xFF; BOOTLOADER.len()])]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.bootloader.header, Err(ImageError::Magic(0xFF)));
    assert_eq!(report.bootloader.image, Err(ImageError::Magic(0xFF)));
    assert_eq!(report.bootloader.chip_size(), Some(0x20_0000));
    assert_eq!(report.tier, Tier::Unverified);
}

/// A bootloader image whose first segment holds the Nix table at 0x1000 of
/// the flash, so the image runs past the table.
fn bootloader_around_a_table() -> Vec<u8> {
    let mut data = bootloader_descriptor_bytes();
    data.resize(0x2000, 0);
    // The segment's data starts at 32.
    data[0x1000 - 32..0x1000 - 32 + TABLE.len()].copy_from_slice(TABLE);
    ImageBuilder::new(vec![(0x4087_5730, data)]).build()
}

#[test]
fn a_bootloader_image_running_past_the_table_offset_is_reported() {
    let bootloader = bootloader_around_a_table();
    let flash = patched(&with_table(&[]), &[(0, &bootloader)]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    let id = report.bootloader.image.unwrap();
    assert!(id.size > 0x1000);
    assert_eq!(report.table.search, Some(Found::One(0x1000)));
    assert_eq!(report.bootloader.fits, Some(false));
    // Its region ends at the table, before its image does.
    assert_eq!(storage.bootloader.span(), Span::new(0, 0x1000));
    assert_eq!(report.tier, Tier::Unverified);

    // Known, at that offset, the same.
    let bootloaders = known_list(&bootloader, 0x1000, true);
    let storage = split_known(&flash, OTA_0.offset, bootloaders);
    assert_eq!(
        storage.report.table.source,
        Some(TableSource::KnownBootloader)
    );
    assert_eq!(storage.report.bootloader.fits, Some(false));
    assert_eq!(storage.bootloader.span(), Span::new(0, 0x1000));
}

#[test]
fn the_bootloader_fits_below_the_compiled_offset_when_no_table_is_located() {
    let flash = unknown_bootloader(&with_table(&[]));
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.bootloader.fits, Some(true));
    assert_eq!(storage.bootloader.span(), Span::new(0, TABLE_OFFSET));
}

#[test]
fn a_located_partition_over_a_sensitive_span_has_no_reader() {
    // An app entry at the booted offset running into config_0, an otadata
    // entry inside diag, and the table found over factory's sector.
    let entries = [
        Entry {
            size: CONFIG_0.end() - OTA_0.offset,
            ..OTA_0
        },
        Entry {
            offset: DIAG.offset,
            ..OTADATA
        },
    ];
    let table = table_with_md5(&entries);
    let flash = unknown_bootloader(&patched(&with_table(&[]), &[(FACTORY.offset, &table)]));
    let storage = split_on(&flash, OTA_0.offset);
    let partitions = partitions(&storage);
    assert_eq!(storage.report.table.offset, Some(FACTORY.offset));
    assert_eq!(
        partitions.booted,
        Some(Span::new(OTA_0.offset, entries[0].size))
    );
    assert_eq!(
        partitions.otadata,
        Some(Span::new(DIAG.offset, OTADATA.size))
    );
    assert_eq!(located_spans(&storage), [None, None, None]);
}

#[test]
fn a_located_partition_past_the_flash_is_cut_to_it() {
    // A 4 MiB flash whose bootloader names 8 MiB: ota_1 runs past its end.
    let mut bytes = nix_board().bytes();
    bytes.truncate(0x40_0000);
    let flash = RamFlash::new(bytes);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert!(report.passed(Step::Layout));
    assert!(!report.passed(Step::FlashSize));
    assert_eq!(report.failed, Some(Step::FlashSize));
    assert_eq!(report.tier, Tier::Unverified);
    assert_eq!(
        located_spans(&storage),
        [None, Some(Span::of(&OTA_0)), Some(Span::of(&OTADATA))]
    );
    // Booted from ota_1, its partition is cut to the flash's end.
    let flash = RamFlash::new({
        let mut bytes = patched(&nix_board(), &[(OTA_1.offset, APP)]).bytes();
        bytes.truncate(0x40_0000);
        bytes
    });
    let storage = split_on(&flash, OTA_1.offset);
    assert_eq!(
        located(&storage).app.as_ref().map(Reader::span),
        Some(Span::new(OTA_1.offset, 0x40_0000 - OTA_1.offset))
    );
}

#[test]
fn a_booted_offset_that_isnt_a_slot_is_unverified() {
    for booted in [0x2_0000, 0x1_1000, 0] {
        let storage = split_on(board(nix_board()), booted);
        assert_eq!(storage.report.failed, Some(Step::BootedSlot));
        assert_eq!(storage.report.tier, Tier::Unverified);
        assert_eq!(storage.report.slot, None);
    }
}

#[test]
fn a_flash_too_small_for_the_table_has_no_table_reader() {
    let flash = RamFlash::new(BOOTLOADER.to_vec());
    let storage = split_on(&flash, OTA_0.offset);
    assert!(storage.partition_table.is_none());
    assert_eq!(
        storage.bootloader.span(),
        Span::new(0, BOOTLOADER.len() as u32)
    );
    assert_eq!(storage.report.tier, Tier::Unverified);
    let empty = RamFlash::new(Vec::new());
    let storage = split_on(&empty, OTA_0.offset);
    assert_eq!(storage.bootloader.span(), Span::new(0, 0));
    assert!(storage.report.bootloader.header.is_err());
    assert_eq!(storage.report.bootloader.chip_size(), None);
}

#[test]
fn the_first_step_that_failed_is_reported() {
    let mut report = split_on(board(nix_board()), OTA_0.offset).report;
    assert_eq!(report.evaluate(), (Tier::Boot, None));
    report.capacity = FLASH_SIZE - 1;
    assert_eq!(report.evaluate(), (Tier::Unverified, Some(Step::FlashSize)));
    report.booted = 0;
    assert_eq!(report.evaluate(), (Tier::Unverified, Some(Step::FlashSize)));
    report.table.offset = Some(0xC000);
    assert_eq!(report.evaluate(), (Tier::Unverified, Some(Step::Layout)));
    report.table.partitions = None;
    assert_eq!(report.evaluate(), (Tier::Unverified, Some(Step::Table)));
    report.table.offset = None;
    assert_eq!(
        report.evaluate(),
        (Tier::Unverified, Some(Step::TableOffset))
    );
    report.bootloader.known = None;
    assert_eq!(
        report.evaluate(),
        (Tier::Unverified, Some(Step::Bootloader))
    );
}

#[test]
fn only_a_known_bootloader_with_rollback_reaches_the_boot_tier() {
    static NO_ROLLBACK: Known = Known {
        digest: [0; 32],
        name: "no rollback",
        table_offset: TABLE_OFFSET,
        rollback: false,
    };
    let mut report = split_on(board(nix_board()), OTA_0.offset).report;
    report.bootloader.known = Some(&NO_ROLLBACK);
    assert_eq!(report.evaluate(), (Tier::Layout, Some(Step::Bootloader)));
}

#[test]
fn each_tier_hands_out_exactly_its_handles() {
    // Unverified: located readers only, no slot, configuration or writer.
    let storage = split_on(board(espflash_board()), 0x1_0000);
    assert!(matches!(storage.handles, Handles::Unverified(_)));
    // Layout: readers of both slots and the otadata, writers of the
    // configurations and diag, and no boot writers.
    let storage = split_on(board(unknown_bootloader(&nix_board())), OTA_0.offset);
    assert!(matches!(storage.handles, Handles::Layout(_)));
    // Boot: the layout tier's and the boot writers.
    let storage = split_on(board(nix_board()), OTA_0.offset);
    assert!(matches!(storage.handles, Handles::Boot(..)));
}

#[test]
fn the_tier_s_handles_are_made_once_per_state_table() {
    // `split` borrows the device and the state table mutably for as long as
    // the handles live; readers of one role share no state with another
    // table's.
    let flash = nix_board();
    let first = run(split(lend(&flash), state(), OTA_0.offset));
    let second = run(split(lend(&flash), state(), OTA_0.offset));
    let (Handles::Boot(_, mut writers), Handles::Boot(handles, _)) =
        (first.handles, second.handles)
    else {
        panic!();
    };
    run(writers.otadata.begin().erase(0, SECTOR_SIZE)).unwrap();
    // The second split's reader belongs to another state table, as a test
    // that splits twice makes; a boot splits once.
    assert!(handles.otadata.write_state().is_stable());
}

// Never panicking.

#[test]
fn random_flashes_never_panic() {
    let mut rng = Rng::new(0x5EED);
    for round in 0..300 {
        let len = [0, 1, 31, 0x1000, 0x8000, 0x8C00, 0x9000, 0x1_0000, 0x2_0000][round % 9]
            + rng.below(0x100);
        let mut bytes = vec![0; len];
        rng.fill(&mut bytes);
        // Some with the real boot chain at the start.
        if round % 3 == 0 && len >= 0x8C00 {
            bytes[..BOOTLOADER.len()].copy_from_slice(BOOTLOADER);
            bytes[0x8000..0x8C00].copy_from_slice(TABLE);
        }
        let flash = RamFlash::new(bytes);
        let booted = match rng.below(5) {
            0 => OTA_0.offset,
            1 => u32::MAX,
            2 => rng.next_u32(),
            3 => (rng.below(len + 1) as u32) & !0xFFFF,
            _ => rng.below(len + 1) as u32,
        };
        let storage = split_on(&flash, booted);
        assert!(storage.report.tier == Tier::Unverified || len >= FLASH_LEN);
    }
}

#[test]
fn mutated_boards_never_panic() {
    let mut rng = Rng::new(0xB0A7D);
    let flashes = [nix_board(), espflash_board()];
    let images = flashes.map(|flash| flash.bytes());
    // Where the chain reads: the bootloader, the table and the app's first
    // segment.
    let hot = [0..BOOTLOADER.len(), 0x8000..0x8C00, 0x1_0000..0x1_0200];
    for round in 0..120 {
        let mut bytes = images[round % 2].clone();
        for _ in 0..=rng.below(8) {
            let range = &hot[rng.below(hot.len())];
            let at = range.start + rng.below(range.len());
            bytes[at] = rng.byte();
        }
        // Now and then a table entry that is an app or the otadata at a
        // random place.
        if rng.below(3) == 0 {
            let kind = [TYPE_APP, TYPE_DATA][rng.below(2)];
            let entry = Entry {
                kind,
                subtype: [0x00, 0x10, 0x11][rng.below(3)],
                offset: rng.next_u32() & !0xFFF,
                size: rng.next_u32(),
                ..PARTITIONS[0]
            };
            let at = 0x8000 + 32 * rng.below(9);
            bytes[at..at + 32].copy_from_slice(&entry_bytes(&entry));
        }
        let flash = RamFlash::new(bytes);
        let booted = [OTA_0.offset, OTA_1.offset, 0x2_0000, u32::MAX][rng.below(4)];
        split_on(&flash, booted);
    }
}
