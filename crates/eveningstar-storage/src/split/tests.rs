#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use super::*;
use crate::Source;
use crate::layout::{CONFIG_0, CONFIG_1, DIAG, OTA_0, OTA_1, OTADATA, TABLE_OFFSET};
use crate::otadata::{SelectEntry, State};
use crate::partitions::SensitiveRead;
use crate::region::is_protected;
use crate::table::Match;
use crate::test_support::{
    APP, BOOTLOADER, ESPFLASH_BOOTLOADER, ESPFLASH_TABLE, FLASH_LEN, OTADATA_AT,
    OTADATA_FIRST_BOOT, RamFlash, Rng, TABLE, block_on, espflash_board, flash_with, lend,
    nix_board, run, table_with_md5,
};

const NIX_NAME: &str = "ESP-IDF v5.5.2, app rollback";

fn span(p: &impl SensitiveRead) -> (u32, u32) {
    (p.offset(), p.len())
}

fn entry_span(entry: Entry) -> (u32, u32) {
    (entry.offset, entry.size)
}

/// `flash` with `parts` written over it.
fn patched(flash: &RamFlash, parts: &[(u32, &[u8])]) -> RamFlash {
    let bytes = flash.bytes();
    let mut all = vec![(0, &bytes[..])];
    all.extend_from_slice(parts);
    RamFlash::new(flash_with(bytes.len(), &all))
}

/// An otadata partition holding one entry, `seq` with `state`, in its first
/// sector.
fn otadata_with(seq: u32, state: State) -> Vec<u8> {
    let mut otadata = vec![0xFF; OTADATA.size as usize];
    otadata[..32].copy_from_slice(&SelectEntry::new(seq, state).to_bytes());
    otadata
}

/// The checks every `Storage` passes, whatever the flash: the tier agrees
/// with the handles, and every handle lies inside the flash.
fn consistent(storage: &Storage<'_>, capacity: u32) {
    let report = &storage.report;
    let tier = report.tier();
    assert_eq!(storage.data.is_some(), tier >= Tier::Data, "{report:?}");
    assert_eq!(
        storage.data.as_ref().is_some_and(|data| data.ota.is_some()),
        tier == Tier::Full
    );
    assert_eq!(report.running.is_some(), tier >= Tier::Data);
    assert_eq!(report.boot_state.is_some(), tier >= Tier::Data);
    assert_eq!(
        report.table_offset().is_some(),
        storage.boot.table.is_some()
    );
    let inside = |(offset, len): (u32, u32)| {
        assert!(u64::from(offset) + u64::from(len) <= u64::from(capacity));
    };
    inside(span(&storage.boot.bootloader));
    if let Some(table) = &storage.boot.table {
        inside(span(table));
    }
    if let Some(app) = &storage.boot.running_app {
        inside(span(app));
        assert_eq!(app.offset(), report.booted);
    }
    if let Some(data) = &storage.data {
        assert!(storage.boot.table.is_some());
        let running = report.running.unwrap();
        assert_eq!(
            storage.boot.running_app.as_ref().map(span),
            Some(entry_span(running.partition()))
        );
        let writable = [
            span(&data.config),
            span(&data.other_config),
            span(&data.diag),
        ];
        for (offset, len) in writable {
            inside((offset, len));
            assert!(!is_protected(offset, offset + len, None));
        }
        inside(span(&data.other_app));
        inside(span(&data.otadata));
    }
    if tier < Tier::Full {
        assert!(report.limit.is_some());
    }
}

fn split_on(flash: &RamFlash, booted: u32) -> Storage<'_> {
    let storage = run(split(lend(flash), booted));
    consistent(&storage, flash.bytes().len() as u32);
    assert_eq!(flash.ops(), [], "split never writes");
    storage
}

// The full tier.

#[test]
fn a_board_flashed_by_nix_reaches_the_full_tier() {
    let flash = nix_board();
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.tier(), Tier::Full);
    assert_eq!(report.limit, None);
    assert!(report.table.matches());
    assert_eq!(report.found_table, None);
    assert_eq!(report.table_offset(), Some(TABLE_OFFSET));
    let bootloader = report.bootloader.unwrap();
    assert_eq!(bootloader.name, Some(NIX_NAME));
    assert_eq!(
        known::Hex(&bootloader.digest).to_string(),
        "6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421"
    );
    assert_eq!(report.booted, OTA_0.offset);
    assert_eq!(report.running, Some(Slot::Ota0));
    let app = report.app.unwrap();
    assert_eq!(app.project_name.to_str(), Some("eveningstar"));
    assert_eq!(app.version.to_str(), Some("0.1.0"));
    // The otadata the bootloader writes on its first boot.
    assert_eq!(
        report.boot_state,
        Some(BootState {
            running: Slot::Ota0,
            selected: Some(Slot::Ota0),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        })
    );
    assert!(!report.cannot_confirm());

    let boot = storage.boot;
    assert_eq!(span(&boot.bootloader), (0, TABLE_OFFSET));
    assert_eq!(span(&boot.table.unwrap()), (TABLE_OFFSET, table::MAX_LEN));
    assert_eq!(span(&boot.running_app.unwrap()), entry_span(OTA_0));

    let data = storage.data.unwrap();
    assert_eq!(span(&data.config), entry_span(CONFIG_0));
    assert_eq!(span(&data.other_config), entry_span(CONFIG_1));
    assert_eq!(span(&data.diag), entry_span(DIAG));
    assert_eq!(span(&data.other_app), entry_span(OTA_1));
    assert_eq!(span(&data.otadata), entry_span(OTADATA));
    let ota = data.ota.unwrap();
    assert_eq!(Some(ota.boot_state()), report.boot_state);
    let debug = format!("{ota:?}");
    assert!(debug.contains("\"eveningstar\""), "{debug}");
    assert!(
        debug.contains(&format!("offset: {}", OTA_1.offset)),
        "{debug}"
    );
}

#[test]
fn a_board_running_from_ota_1_hands_out_the_other_slot_s_partitions() {
    let otadata = otadata_with(2, State::Valid);
    let flash = patched(&nix_board(), &[(OTADATA_AT, &otadata), (OTA_1.offset, APP)]);
    let storage = split_on(&flash, OTA_1.offset);
    let report = storage.report;
    assert_eq!(report.tier(), Tier::Full);
    assert_eq!(report.running, Some(Slot::Ota1));
    assert_eq!(
        report.boot_state.unwrap(),
        BootState {
            running: Slot::Ota1,
            selected: Some(Slot::Ota1),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        }
    );
    assert_eq!(span(&storage.boot.running_app.unwrap()), entry_span(OTA_1));
    let data = storage.data.unwrap();
    assert_eq!(span(&data.config), entry_span(CONFIG_1));
    assert_eq!(span(&data.other_config), entry_span(CONFIG_0));
    assert_eq!(span(&data.other_app), entry_span(OTA_0));
    assert_eq!(data.ota.unwrap().boot_state(), report.boot_state.unwrap());
}

#[test]
fn the_bootloader_is_hashed_with_yields() {
    let flash = nix_board();
    let done = block_on(split(lend(&flash), OTA_0.offset));
    // 22 KiB hashed, a yield every 4 KiB.
    assert_eq!(done.wakes, BOOTLOADER.len() / 4096);
    assert_eq!(done.output.report.tier(), Tier::Full);
}

// The data tier.

#[test]
fn an_unknown_bootloader_gives_the_data_tier() {
    let flash = patched(&nix_board(), &[(0, ESPFLASH_BOOTLOADER)]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.tier(), Tier::Data);
    assert_eq!(report.limit, Some(Limit::BootloaderUnknown));
    assert_eq!(report.bootloader.unwrap().name, None);
    assert!(report.app.is_ok());
    let data = storage.data.unwrap();
    assert!(data.ota.is_none());
    assert_eq!(span(&data.config), entry_span(CONFIG_0));
    assert!(!report.cannot_confirm());
}

#[test]
fn a_corrupt_bootloader_gives_the_data_tier() {
    let mut bootloader = BOOTLOADER.to_vec();
    bootloader[0x100] ^= 0x01;
    let flash = patched(&nix_board(), &[(0, &bootloader)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::BootloaderInvalid));
    assert!(matches!(
        storage.report.bootloader,
        Err(ImageError::Checksum { .. } | ImageError::DigestMismatch)
    ));
    assert_eq!(storage.report.tier(), Tier::Data);

    let erased = vec![0xFF; BOOTLOADER.len()];
    let flash = patched(&nix_board(), &[(0, &erased)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.bootloader, Err(ImageError::Magic(0xFF)));
    assert_eq!(storage.report.limit, Some(Limit::BootloaderInvalid));
}

#[test]
fn an_unreadable_app_descriptor_gives_the_data_tier() {
    // The slot holds no image.
    let flash = patched(&nix_board(), &[(OTA_0.offset, &[0xFF; 64])]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::AppDescriptor));
    assert_eq!(storage.report.app, Err(ImageError::Magic(0xFF)));
    assert!(storage.data.unwrap().ota.is_none());
    // The running slot is still the running app's handle.
    assert_eq!(span(&storage.boot.running_app.unwrap()), entry_span(OTA_0));

    // The descriptor can't be read from the flash.
    let flash = nix_board();
    flash.fail_reads(OTA_0.offset + 32..OTA_0.offset + 33);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::AppDescriptor));
    assert_eq!(storage.report.app, Err(ImageError::Read(ReadError::Device)));

    // Booted from ota_1, which holds nothing.
    let flash = nix_board();
    let storage = split_on(&flash, OTA_1.offset);
    assert_eq!(storage.report.limit, Some(Limit::AppDescriptor));
    assert_eq!(storage.report.running, Some(Slot::Ota1));
}

#[test]
fn the_bootloader_is_checked_before_the_app() {
    let flash = patched(
        &nix_board(),
        &[(0, ESPFLASH_BOOTLOADER), (OTA_0.offset, &[0; 64])],
    );
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::BootloaderUnknown));
    assert!(storage.report.app.is_err());
}

// The untrusted tier.

#[test]
fn a_board_flashed_by_cargo_run_is_untrusted_with_espflash_s_table() {
    let flash = espflash_board();
    let storage = split_on(&flash, 0x1_0000);
    let report = storage.report;
    assert_eq!(report.tier(), Tier::Untrusted);
    assert_eq!(report.limit, Some(Limit::Table));
    assert!(!report.table.matches());
    assert_eq!(report.found_table, Some(Found::One(0x8000)));
    assert_eq!(report.table_offset(), Some(0x8000));
    let bootloader = report.bootloader.unwrap();
    assert_eq!(bootloader.name, None);
    assert_eq!(report.running, None);
    assert_eq!(report.boot_state, None);
    assert_eq!(
        report.app.unwrap().project_name.to_str(),
        Some("eveningstar")
    );
    assert!(!report.cannot_confirm());
    assert!(storage.data.is_none());

    assert_eq!(span(&storage.boot.bootloader), (0, 0x8000));
    let table = storage.boot.table.unwrap();
    assert_eq!(span(&table), (0x8000, table::MAX_LEN));
    let mut bytes = vec![0; ESPFLASH_TABLE.len()];
    Source::read(&table, 0, &mut bytes).unwrap();
    assert_eq!(bytes, ESPFLASH_TABLE);
    // The image, inside espflash's factory app partition.
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (0x1_0000, APP.len() as u32)
    );
}

#[test]
fn a_moved_table_is_found_and_untrusted() {
    let flash = RamFlash::with(
        FLASH_LEN,
        &[(0, BOOTLOADER), (0xC000, TABLE), (OTA_0.offset, APP)],
    );
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.limit, Some(Limit::Table));
    assert_eq!(report.table.partitions, [Match::Missing; 8]);
    assert_eq!(report.found_table, Some(Found::One(0xC000)));
    // The bootloader up to the table found.
    assert_eq!(span(&storage.boot.bootloader), (0, 0xC000));
    assert_eq!(report.bootloader.unwrap().name, Some(NIX_NAME));
    assert_eq!(span(&storage.boot.table.unwrap()), (0xC000, table::MAX_LEN));
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (OTA_0.offset, APP.len() as u32)
    );
    assert!(storage.data.is_none());
}

#[test]
fn several_moved_tables_are_no_table() {
    // A table left at 0x8000 by an earlier layout, and the bootloader's at
    // 0xC000: which one it reads is unknown.
    let flash = RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (0x9000, ESPFLASH_TABLE),
            (0xC000, TABLE),
            (OTA_0.offset, APP),
        ],
    );
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.limit, Some(Limit::Table));
    assert_eq!(report.found_table, Some(Found::Several(0x9000, 0xC000)));
    assert_eq!(report.table_offset(), None);
    assert!(storage.boot.table.is_none());
    assert_eq!(span(&storage.boot.bootloader), (0, TABLE_OFFSET));
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (OTA_0.offset, APP.len() as u32)
    );
    assert!(storage.data.is_none());
}

#[test]
fn an_unknown_bootloader_with_another_table_is_untrusted() {
    // The layout's table at 0x8000, and another below the running app: an
    // unknown bootloader could read either.
    for (at, other) in [(0xC000, TABLE), (0x9000, ESPFLASH_TABLE)] {
        let flash = patched(&nix_board(), &[(0, ESPFLASH_BOOTLOADER), (at, other)]);
        let storage = split_on(&flash, OTA_0.offset);
        let report = storage.report;
        assert!(report.table.matches());
        assert_eq!(report.limit, Some(Limit::OtherTable), "{at:#x}");
        assert_eq!(report.tier(), Tier::Untrusted);
        assert_eq!(report.found_table, Some(Found::Several(TABLE_OFFSET, at)));
        assert_eq!(report.table_offset(), None);
        assert_eq!(report.running, None);
        assert_eq!(report.boot_state, None);
        assert!(storage.boot.table.is_none());
        assert!(storage.data.is_none());
        assert!(!report.cannot_confirm());
    }
    // Booted from ota_1, a table inside ota_0 counts.
    let flash = patched(
        &nix_board(),
        &[
            (0, ESPFLASH_BOOTLOADER),
            (OTA_1.offset, APP),
            (OTA_0.offset + 0x1000, TABLE),
        ],
    );
    let storage = split_on(&flash, OTA_1.offset);
    assert_eq!(storage.report.limit, Some(Limit::OtherTable));
    // A corrupt bootloader isn't known either.
    let mut bootloader = BOOTLOADER.to_vec();
    bootloader[0x100] ^= 0x01;
    let flash = patched(&nix_board(), &[(0, &bootloader), (0xC000, TABLE)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::OtherTable));
}

#[test]
fn an_unknown_bootloader_with_only_the_compiled_table_keeps_the_data_tier() {
    // A table without an app at the booted offset can't be the bootloader's.
    let elsewhere = table_with_md5(&[Entry {
        offset: 0x2_0000,
        ..OTA_0
    }]);
    let flash = patched(
        &nix_board(),
        &[(0, ESPFLASH_BOOTLOADER), (0xC000, &elsewhere)],
    );
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.limit, Some(Limit::BootloaderUnknown));
    assert_eq!(report.found_table, Some(Found::One(TABLE_OFFSET)));
    assert_eq!(report.table_offset(), Some(TABLE_OFFSET));
    assert!(storage.data.is_some());
}

#[test]
fn a_known_bootloader_ignores_other_tables() {
    // A known bootloader reads the compiled offset, so a leftover table
    // elsewhere doesn't matter, and the board isn't searched.
    let flash = patched(&nix_board(), &[(0xC000, TABLE)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.tier(), Tier::Full);
    assert_eq!(storage.report.found_table, None);
    // Nor when the full tier is missed for the app's descriptor.
    let flash = patched(
        &nix_board(),
        &[(0xC000, TABLE), (OTA_0.offset, &[0xFF; 64])],
    );
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::AppDescriptor));
    assert_eq!(storage.report.found_table, None);
}

#[test]
fn the_untrusted_running_app_is_cut_to_its_entry() {
    // An entry shorter than the image.
    let short = Entry {
        size: 0x1000,
        ..OTA_0
    };
    let table = table_with_md5(&[short]);
    let flash = RamFlash::with(
        FLASH_LEN,
        &[(0, BOOTLOADER), (0xC000, &table), (OTA_0.offset, APP)],
    );
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.found_table, Some(Found::One(0xC000)));
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (OTA_0.offset, 0x1000)
    );
    // No image: the entry.
    let flash = RamFlash::with(FLASH_LEN, &[(0, BOOTLOADER), (0xC000, TABLE)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(span(&storage.boot.running_app.unwrap()), entry_span(OTA_0));
}

#[test]
fn without_a_table_the_running_app_is_its_image() {
    let flash = RamFlash::with(FLASH_LEN, &[(0, BOOTLOADER), (OTA_0.offset, APP)]);
    let storage = split_on(&flash, OTA_0.offset);
    let report = storage.report;
    assert_eq!(report.limit, Some(Limit::Table));
    assert_eq!(report.found_table, Some(Found::Nothing));
    assert_eq!(report.table_offset(), None);
    assert!(storage.boot.table.is_none());
    assert_eq!(span(&storage.boot.bootloader), (0, TABLE_OFFSET));
    assert!(report.bootloader.is_ok());
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (OTA_0.offset, APP.len() as u32)
    );

    // No table and no image: nothing is known of the running app.
    let flash = RamFlash::with(FLASH_LEN, &[(0, BOOTLOADER)]);
    let storage = split_on(&flash, OTA_0.offset);
    assert!(storage.boot.running_app.is_none());
    assert_eq!(storage.report.app, Err(ImageError::Magic(0xFF)));
}

#[test]
fn a_booted_offset_that_isnt_a_slot_is_untrusted() {
    let flash = patched(&nix_board(), &[(0x2_0000, APP)]);
    let storage = split_on(&flash, 0x2_0000);
    let report = storage.report;
    assert_eq!(report.limit, Some(Limit::NotASlot));
    assert!(report.table.matches());
    assert_eq!(report.found_table, None);
    assert_eq!(report.running, None);
    assert_eq!(report.boot_state, None);
    // The table matches but has no entry there: the image's length.
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (0x2_0000, APP.len() as u32)
    );
    assert!(storage.data.is_none());

    for booted in [
        0,
        0x1_0001,
        0x8000,
        OTA_0.offset - 0x1_0000,
        FLASH_LEN as u32,
        u32::MAX,
    ] {
        let storage = split_on(&flash, booted);
        assert_eq!(storage.report.limit, Some(Limit::NotASlot), "{booted:#x}");
        assert_eq!(storage.report.booted, booted);
    }
    let storage = split_on(&flash, u32::MAX);
    assert!(storage.boot.running_app.is_none());
    assert_eq!(
        storage.report.app,
        Err(ImageError::Read(ReadError::OutOfRange))
    );
}

#[test]
fn a_flash_smaller_than_the_layout_is_untrusted() {
    let flash = RamFlash::with(
        0x40_0000,
        &[
            (0, BOOTLOADER),
            (TABLE_OFFSET, TABLE),
            (OTADATA_AT, OTADATA_FIRST_BOOT),
            (OTA_0.offset, APP),
        ],
    );
    let storage = split_on(&flash, OTA_0.offset);
    assert_eq!(storage.report.limit, Some(Limit::FlashSize));
    assert_eq!(storage.report.tier(), Tier::Untrusted);
    assert!(storage.data.is_none());
    // The image, inside ota_0's entry.
    assert_eq!(
        span(&storage.boot.running_app.unwrap()),
        (OTA_0.offset, APP.len() as u32)
    );
}

#[test]
fn the_limits_name_their_tiers() {
    assert_eq!(Limit::Table.tier(), Tier::Untrusted);
    assert_eq!(Limit::FlashSize.tier(), Tier::Untrusted);
    assert_eq!(Limit::NotASlot.tier(), Tier::Untrusted);
    assert_eq!(Limit::OtherTable.tier(), Tier::Untrusted);
    assert_eq!(Limit::BootloaderInvalid.tier(), Tier::Data);
    assert_eq!(Limit::BootloaderUnknown.tier(), Tier::Data);
    assert_eq!(Limit::AppDescriptor.tier(), Tier::Data);
    assert!(Tier::Untrusted < Tier::Data && Tier::Data < Tier::Full);
}

// T7.

#[test]
fn a_pending_image_can_be_confirmed_only_in_the_full_tier() {
    let pending = otadata_with(1, State::PendingVerify);

    let flash = patched(&nix_board(), &[(OTADATA_AT, &pending)]);
    let report = split_on(&flash, OTA_0.offset).report;
    assert_eq!(report.tier(), Tier::Full);
    assert_eq!(report.boot_state.unwrap().state, Some(State::PendingVerify));
    assert!(!report.cannot_confirm());

    // An unknown bootloader.
    let flash = patched(
        &nix_board(),
        &[(0, ESPFLASH_BOOTLOADER), (OTADATA_AT, &pending)],
    );
    let report = split_on(&flash, OTA_0.offset).report;
    assert_eq!(report.tier(), Tier::Data);
    assert!(report.cannot_confirm());

    // A corrupt bootloader.
    let mut bootloader = BOOTLOADER.to_vec();
    bootloader[0x40] ^= 0x80;
    let flash = patched(&nix_board(), &[(0, &bootloader), (OTADATA_AT, &pending)]);
    let report = split_on(&flash, OTA_0.offset).report;
    assert_eq!(report.limit, Some(Limit::BootloaderInvalid));
    assert!(report.cannot_confirm());

    // The running app's descriptor doesn't read: pending in ota_1, which is
    // empty.
    let pending_1 = otadata_with(2, State::PendingVerify);
    let flash = patched(&nix_board(), &[(OTADATA_AT, &pending_1)]);
    let report = split_on(&flash, OTA_1.offset).report;
    assert_eq!(report.limit, Some(Limit::AppDescriptor));
    assert!(report.cannot_confirm());

    // The pending entry is for the other slot: the running image isn't
    // pending.
    let flash = patched(
        &nix_board(),
        &[(0, ESPFLASH_BOOTLOADER), (OTADATA_AT, &pending_1)],
    );
    let report = split_on(&flash, OTA_0.offset).report;
    assert_eq!(report.tier(), Tier::Data);
    assert_ne!(report.boot_state.unwrap().state, Some(State::PendingVerify));
    assert!(!report.cannot_confirm());

    // The untrusted tier reads no boot state.
    let flash = patched(&espflash_board(), &[(OTADATA_AT, &pending)]);
    let report = split_on(&flash, 0x1_0000).report;
    assert_eq!(report.tier(), Tier::Untrusted);
    assert_eq!(report.boot_state, None);
    assert!(!report.cannot_confirm());
}

#[test]
fn every_state_of_the_running_image_is_reported() {
    for state in [
        State::New,
        State::Valid,
        State::Undefined,
        State::Unknown(9),
    ] {
        let otadata = otadata_with(1, state);
        let flash = patched(
            &nix_board(),
            &[(0, ESPFLASH_BOOTLOADER), (OTADATA_AT, &otadata)],
        );
        let report = split_on(&flash, OTA_0.offset).report;
        assert_eq!(report.boot_state.unwrap().state, Some(state));
        assert!(!report.cannot_confirm(), "{state:?}");
    }
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
        assert!(storage.report.tier() == Tier::Untrusted || len >= FLASH_LEN);
    }
}

#[test]
fn mutated_boards_never_panic() {
    let mut rng = Rng::new(0xB0A7D);
    let flashes = [nix_board(), espflash_board()];
    let images = flashes.map(|flash| flash.bytes());
    // Where the checks read: the bootloader, the table, the otadata and the
    // app's first segment.
    let hot = [
        0..BOOTLOADER.len(),
        0x8000..0x8C00,
        0xA000..0xA020,
        0x1_0000..0x1_0200,
    ];
    for round in 0..120 {
        let mut bytes = images[round % 2].clone();
        for _ in 0..=rng.below(8) {
            let range = &hot[rng.below(hot.len())];
            let at = range.start + rng.below(range.len());
            bytes[at] = rng.byte();
        }
        let flash = RamFlash::new(bytes);
        let booted = [OTA_0.offset, OTA_1.offset, 0x2_0000, u32::MAX][rng.below(4)];
        split_on(&flash, booted);
    }
}
