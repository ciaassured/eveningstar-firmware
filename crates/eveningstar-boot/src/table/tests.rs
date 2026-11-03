#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use eveningstar_esp_formats::pace::Yield;
use eveningstar_esp_formats::table::Match;
use eveningstar_esp_formats::test_support::{
    APP, BOOTLOADER, ESPFLASH_BOOTLOADER, ESPFLASH_TABLE, OTADATA_FIRST_BOOT, TABLE, entry_bytes,
    md5_entry_for, table_of,
};
use eveningstar_esp_formats::{TYPE_APP, TYPE_DATA};
use eveningstar_storage::chain::{Handles, Storage};
use eveningstar_storage::roles::AlternateApp;
use eveningstar_storage::test_support::{RamNor, TestFlash, run};
use eveningstar_storage::{Slot, ViewError};

use super::*;
use crate::test_support::{nix_board, sha256, split_board};

const N: usize = layout::PARTITIONS.len();

/// The entry of a partition after the layout's, inside the flash.
const EXTRA: Entry = Entry {
    label: "extra",
    kind: TYPE_DATA,
    subtype: 0x06,
    offset: 0x7F_0000,
    size: 0x1_0000,
    flags: 0,
};

/// The layout's entries, as the table stores them.
fn layout_entries() -> Vec<[u8; 32]> {
    layout::PARTITIONS.iter().map(entry_bytes).collect()
}

/// The table's region as a board holds it: `table`, then `0x5A` bytes, which
/// stand for leftovers of another table, up to the end of the sector.
fn with_leftovers(table: &[u8]) -> Vec<u8> {
    let mut sector = vec![0x5A; SECTOR_SIZE as usize];
    sector[..table.len()].copy_from_slice(table);
    sector
}

/// The Nix board with `table` at the compiled offset, split.
fn nix_with_table(table: &[u8]) -> Storage<'static> {
    let device = nix_board();
    device.poke(layout::TABLE_OFFSET, &with_leftovers(table));
    split_board(device, layout::OTA_0.offset).0
}

/// The table module of a split board.
fn module(storage: Storage<'static>) -> PartitionTable<'static> {
    let found = match storage.handles {
        Handles::Unverified(located) => located.table,
        Handles::Layout(_) | Handles::Boot(..) => None,
    };
    PartitionTable::new(storage.partition_table, found, &storage.report)
}

/// Every byte of `view`.
fn served(view: &TableView<'_>) -> Vec<u8> {
    let mut bytes = vec![0; view.len() as usize];
    run(view.read(0, &mut bytes)).unwrap();
    bytes
}

fn rows(module: &PartitionTable<'_>, model: &TableModel) -> Vec<PartitionRow> {
    module
        .partitions(model)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn labels(rows: &[PartitionRow]) -> Vec<Option<String>> {
    rows.iter()
        .map(|row| match row {
            PartitionRow::Partition(partition) => partition
                .label()
                .map(|label| label.to_str().unwrap().to_owned()),
            PartitionRow::Md5 { .. } => Some("<md5>".to_owned()),
        })
        .collect()
}

/// The offset of entry `index`, relative to the table.
const fn at(index: usize) -> u32 {
    index as u32 * ENTRY_LEN
}

#[test]
fn the_nix_table_is_covered_by_the_layout_and_published_whole() {
    let (storage, _) = split_board(nix_board(), layout::OTA_0.offset);
    assert_eq!(storage.report.tier, Tier::Boot);
    let module = module(storage);
    let location = module.location();
    assert_eq!(location.source(), Some(TableSource::KnownBootloader));
    assert_eq!(location.offset(), Some(layout::TABLE_OFFSET));
    assert_eq!(location.search(), None);
    assert_eq!(module.span(), Some(EXPECTED.table));

    let model = module.parse().unwrap();
    assert_eq!(model.offset(), layout::TABLE_OFFSET);
    let valid = model.verdict().unwrap();
    assert_eq!(valid.coverage(), Coverage::Layout);
    assert_eq!(valid.end(), N + 1);
    assert_eq!(valid.partitions(), N);
    assert!(!valid.md5_read_as_partition());
    assert!(valid.comparison().matches());
    let md5 = valid.md5().unwrap();
    assert_eq!(md5.index, N);
    assert_eq!(
        md5.digest[..],
        TABLE[at(N) as usize + 16..at(N + 1) as usize]
    );

    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), at(N + 2));
    assert_eq!(view.sanitized(), []);
    assert_eq!(served(&view), TABLE[..at(N + 2) as usize]);
    let digest = run(view.digest(
        Span::new(0, view.len()),
        sha2::Sha256::default(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(digest.sha256, sha256(&TABLE[..at(N + 2) as usize]));
    assert_eq!(digest.epoch, model.epoch());

    let rows = rows(&module, &model);
    assert_eq!(rows.len(), N);
    for (row, expected) in rows.iter().zip(EXPECTED.partitions) {
        let PartitionRow::Partition(partition) = row else {
            panic!("{row:?}");
        };
        assert!(partition.label().unwrap().is_padded(expected.label));
        assert_eq!(
            (
                partition.kind(),
                partition.subtype(),
                partition.offset(),
                partition.size(),
                partition.flags()
            ),
            (
                expected.kind,
                expected.subtype,
                expected.offset,
                expected.size,
                expected.flags
            )
        );
    }
}

#[test]
fn ignored_bytes_that_arent_erased_are_served_erased_and_listed() {
    let mut slots = layout_entries();
    slots.push(md5_entry_for(&slots));
    let mut table = table_of(&slots);
    let padding = at(N) as usize + 2..at(N) as usize + 16;
    let end_tail = at(N + 1) as usize + 4..at(N + 2) as usize;
    table[padding.clone()].fill(0x00);
    table[end_tail.clone()].fill(0x11);
    let module = module(nix_with_table(&table));
    let model = module.parse().unwrap();
    assert_eq!(model.verdict().unwrap().coverage(), Coverage::Layout);

    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), at(N + 2));
    assert_eq!(
        view.sanitized(),
        [Span::new(at(N) + 2, 14), Span::new(at(N + 1) + 4, 28)]
    );
    let mut expected = table[..at(N + 2) as usize].to_vec();
    expected[padding].fill(0xFF);
    expected[end_tail].fill(0xFF);
    let bytes = served(&view);
    assert_eq!(bytes, expected);
    // Nothing past the end entry, where the leftovers lie.
    let mut past = [0; 1];
    assert_eq!(
        run(view.read(view.len(), &mut past)),
        Err(ViewError::OutOfRange)
    );
    let digest = run(view.digest(
        Span::new(0, view.len()),
        sha2::Sha256::default(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(digest.sha256, sha256(&expected));
    assert_ne!(digest.sha256, sha256(&table[..at(N + 2) as usize]));
}

#[test]
fn erased_ignored_bytes_arent_listed() {
    let mut slots = layout_entries();
    slots.push(md5_entry_for(&slots));
    let mut table = table_of(&slots);
    table[at(N + 1) as usize + 31] = 0;
    let module = module(nix_with_table(&table));
    let model = module.parse().unwrap();
    let view = module.published(&model).unwrap();
    // The MD5 entry's padding is erased; only the end entry's tail isn't.
    assert_eq!(view.sanitized(), [Span::new(at(N + 1) + 4, 28)]);
    assert_eq!(served(&view), TABLE[..at(N + 2) as usize]);
}

#[test]
fn a_layout_table_without_an_md5_entry_is_covered_by_the_layout() {
    let mut table = table_of(&layout_entries());
    table[at(N) as usize + 8] = 0x22;
    let storage = nix_with_table(&table);
    assert_eq!(storage.report.tier, Tier::Boot);
    let module = module(storage);
    let model = module.parse().unwrap();
    let valid = model.verdict().unwrap();
    assert_eq!((valid.coverage(), valid.md5()), (Coverage::Layout, None));
    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), at(N + 1));
    assert_eq!(view.sanitized(), [Span::new(at(N) + 4, 28)]);
    let mut expected = table[..at(N + 1) as usize].to_vec();
    expected[at(N) as usize + 4..].fill(0xFF);
    assert_eq!(served(&view), expected);
    assert!(labels(&rows(&module, &model)).iter().all(Option::is_some));
}

#[test]
fn entries_after_the_md5_entry_end_the_view_after_it() {
    let mut slots = layout_entries();
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&EXTRA));
    let mut table = table_of(&slots);
    table[at(N) as usize + 2..at(N) as usize + 16].fill(0x33);
    let storage = nix_with_table(&table);
    assert_eq!(storage.report.tier, Tier::Unverified);
    let module = module(storage);
    assert_eq!(module.span(), Some(EXPECTED.table));
    let model = module.parse().unwrap();
    let valid = model.verdict().unwrap();
    assert_eq!(valid.coverage(), Coverage::Md5);
    assert!(valid.md5_read_as_partition());
    assert_eq!(valid.end(), N + 2);
    assert!(!valid.comparison().matches());

    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), at(N + 1));
    assert_eq!(view.sanitized(), [Span::new(at(N) + 2, 14)]);
    let mut expected = table[..at(N + 1) as usize].to_vec();
    expected[at(N) as usize + 2..at(N) as usize + 16].fill(0xFF);
    assert_eq!(served(&view), expected);

    // The partitions are the layout's, labelled, then the MD5 entry; the
    // entry after it is the one the bootloader leaves out.
    let rows = rows(&module, &model);
    assert_eq!(rows.len(), N + 1);
    assert_eq!(rows[N], PartitionRow::Md5 { index: N });
    assert!(labels(&rows[..N]).iter().all(Option::is_some));
}

#[test]
fn an_entry_after_the_md5_entry_has_no_label() {
    // Two entries after the MD5 entry: the bootloader loads the first, which
    // the MD5 doesn't cover.
    let mut slots = layout_entries();
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&EXTRA));
    slots.push(entry_bytes(&Entry {
        offset: 0x7E_0000,
        ..EXTRA
    }));
    let module = module(nix_with_table(&table_of(&slots)));
    let model = module.parse().unwrap();
    let rows = rows(&module, &model);
    assert_eq!(rows.len(), N + 2);
    let PartitionRow::Partition(after) = rows[N + 1] else {
        panic!("{:?}", rows[N + 1]);
    };
    assert_eq!(after.label(), None);
    assert_eq!((after.offset(), after.size()), (EXTRA.offset, EXTRA.size));
    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), at(N + 1));
}

#[test]
fn a_table_without_an_md5_entry_below_the_layout_tier_publishes_no_bytes_or_labels() {
    let mut slots = layout_entries();
    slots.push(entry_bytes(&EXTRA));
    let storage = nix_with_table(&table_of(&slots));
    assert_eq!(storage.report.tier, Tier::Unverified);
    let module = module(storage);
    let model = module.parse().unwrap();
    let valid = model.verdict().unwrap();
    assert_eq!(valid.coverage(), Coverage::Uncovered);
    assert_eq!(valid.md5(), None);
    assert_eq!(
        module.published(&model).unwrap_err(),
        Unpublished::Unverified
    );
    // The fields, without labels.
    let rows = rows(&module, &model);
    assert_eq!(rows.len(), N + 1);
    assert!(labels(&rows).iter().all(Option::is_none));
    let PartitionRow::Partition(last) = rows[N] else {
        panic!("{:?}", rows[N]);
    };
    assert_eq!(
        (
            last.kind(),
            last.subtype(),
            last.offset(),
            last.size(),
            last.flags()
        ),
        (
            EXTRA.kind,
            EXTRA.subtype,
            EXTRA.offset,
            EXTRA.size,
            EXTRA.flags
        )
    );
    // The comparison by label is with the layout's own labels only.
    assert_eq!(valid.comparison().extra, 1);
    assert!(
        valid
            .comparison()
            .expected
            .iter()
            .all(|m| *m == Match::Matches)
    );
}

/// A board flashed by `cargo run`: espflash's bootloader and default table,
/// and the app in its `factory` partition at 0x10000.
fn espflash_board() -> RamNor {
    RamNor::with(
        layout::FLASH_SIZE,
        &[
            (0, ESPFLASH_BOOTLOADER),
            (0x8000, ESPFLASH_TABLE),
            (0x1_0000, APP),
        ],
    )
}

#[test]
fn a_table_the_search_found_alone_is_read_and_covered_by_its_md5() {
    let (storage, _) = split_board(espflash_board(), 0x1_0000);
    assert_eq!(storage.report.tier, Tier::Unverified);
    let module = module(storage);
    assert_eq!(module.location().source(), Some(TableSource::Searched));
    assert_eq!(module.location().search(), Some(Found::One(0x8000)));
    let model = module.parse().unwrap();
    let valid = model.verdict().unwrap();
    assert_eq!(valid.coverage(), Coverage::Md5);
    let view = module.published(&model).unwrap();
    assert!(matches!(view, TableView::Found(_)));
    assert_eq!(view.len(), at(5));
    assert_eq!(served(&view), ESPFLASH_TABLE[..at(5) as usize]);
    assert_eq!(
        labels(&rows(&module, &model)),
        [Some("nvs"), Some("phy_init"), Some("factory")].map(|l| l.map(str::to_owned))
    );
}

#[test]
fn several_tables_found_publish_nothing_but_their_offsets() {
    let device = espflash_board();
    device.poke(0x9000, ESPFLASH_TABLE);
    let (storage, _) = split_board(device, 0x1_0000);
    let module = module(storage);
    let location = module.location();
    assert_eq!(location.search(), Some(Found::Several(0x8000, 0x9000)));
    assert_eq!((location.source(), location.offset()), (None, None));
    assert_eq!(module.span(), None);
    assert_eq!(module.parse(), Err(LoadError::Absent));
}

#[test]
fn a_table_the_bootloader_refuses_publishes_nothing() {
    let mut slots = layout_entries();
    let mut md5 = md5_entry_for(&slots);
    md5[31] ^= 1;
    slots.push(md5);
    let module = module(nix_with_table(&table_of(&slots)));
    let model = module.parse().unwrap();
    // The reason, without the digests stored and computed.
    assert_eq!(model.verdict(), Err(TableFailure::Md5 { index: N }));
    assert_eq!(
        module.published(&model).unwrap_err(),
        Unpublished::Unverified
    );
    assert_eq!(
        module.partitions(&model).map(|_| ()),
        Err(LoadError::Absent)
    );
}

#[test]
fn failures_keep_no_value_of_the_table_refused() {
    assert_eq!(
        TableFailure::from(TableError::Magic {
            index: 3,
            magic: 0x1234
        }),
        TableFailure::Magic { index: 3 }
    );
    assert_eq!(
        TableFailure::from(TableError::PastChip {
            index: 2,
            offset: 1,
            size: 2
        }),
        TableFailure::PastChip { index: 2 }
    );
    assert_eq!(
        TableFailure::from(TableError::Md5 {
            index: 4,
            stored: [1; 16],
            computed: [2; 16]
        }),
        TableFailure::Md5 { index: 4 }
    );
}

#[test]
fn the_expected_layout_is_the_compiled_one() {
    assert_eq!(EXPECTED.bootloader, Span::new(0, 0x8000));
    assert_eq!(EXPECTED.table, Span::new(0x8000, 0x1000));
    assert_eq!(EXPECTED.partitions, layout::PARTITIONS);
    assert!(
        EXPECTED
            .partitions
            .iter()
            .any(|entry| entry.kind == TYPE_APP)
    );
}

/// A table module over a table placed in `ota_1`, which a writer can write,
/// as the search would locate it, and the flash.
fn module_in_ota_1() -> (PartitionTable<'static>, TestFlash) {
    let (storage, _) = split_board(espflash_board(), 0x1_0000);
    let mut report = storage.report;
    let offset = layout::OTA_1.offset;
    report.table.offset = Some(offset);
    let device = RamNor::with(layout::FLASH_SIZE, &[(offset, ESPFLASH_TABLE)]);
    let flash = TestFlash::new(device);
    let found = flash.reader::<FoundTable>(Span::new(offset, SECTOR_SIZE));
    (PartitionTable::new(None, Some(found), &report), flash)
}

#[test]
fn a_model_its_rows_and_its_view_go_stale_once_the_region_is_written() {
    let (module, flash) = module_in_ota_1();
    let mut writer = flash.writer::<AlternateApp>(Span::from(layout::OTA_1), Slot::Ota0);
    let model = module.parse().unwrap();
    let view = module.published(&model).unwrap();
    let mut rows = module.partitions(&model).unwrap();
    assert!(rows.next().unwrap().is_ok());

    run(writer.begin().program(0x1_0000, &[0; 4])).unwrap();
    let mut byte = [0; 1];
    assert_eq!(run(view.read(0, &mut byte)), Err(ViewError::Stale));
    assert_eq!(rows.next(), Some(Err(ReadError::Stale)));
    assert_eq!(module.published(&model).unwrap_err(), Unpublished::Stale);
    assert_eq!(module.partitions(&model).map(|_| ()), Err(LoadError::Stale));
    let digest = run(view.digest(
        Span::new(0, 1),
        sha2::Sha256::default(),
        &mut Yield,
        &mut || {},
    ));
    assert_eq!(digest, Err(ViewError::Stale));

    let again = module.parse().unwrap();
    assert_ne!(again.epoch(), model.epoch());
    assert!(module.published(&again).is_ok());

    let session = writer.begin();
    assert_eq!(module.parse(), Err(LoadError::Busy));
    drop(session);
}

#[test]
fn a_session_during_the_parse_makes_it_stale() {
    let (module, flash) = module_in_ota_1();
    let mut writer = flash.writer::<AlternateApp>(Span::from(layout::OTA_1), Slot::Ota0);
    flash.device().before_call(2, move || {
        run(writer.begin().program(0x1_0000, &[0; 4])).unwrap();
    });
    assert_eq!(module.parse(), Err(LoadError::Stale));
    assert!(module.parse().is_ok());
}

#[test]
fn the_module_can_be_shared_from_a_static() {
    fn shared<T: Sync + Send>() {}
    shared::<PartitionTable<'static>>();
}

#[test]
fn a_table_not_located_is_not_read() {
    // The layout's bytes at the compiled offset, but no table located.
    let device = RamNor::with(
        layout::FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (layout::TABLE_OFFSET, TABLE),
            (layout::OTADATA.offset, OTADATA_FIRST_BOOT),
        ],
    );
    let (storage, _) = split_board(device, u32::MAX);
    let mut report = storage.report;
    report.table.offset = None;
    let module = PartitionTable::new(storage.partition_table, None, &report);
    assert_eq!(module.span(), None);
    assert_eq!(module.parse(), Err(LoadError::Absent));
}
