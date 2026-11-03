use eveningstar_esp_formats::TYPE_DATA;
use eveningstar_esp_formats::otadata::{SelectEntry, crc};
use eveningstar_esp_formats::test_support::{entry_bytes, md5_entry_for, otadata_of, table_of};
use eveningstar_storage::chain::Handles;
use eveningstar_storage::test_support::{RamNor, TestFlash, run};
use eveningstar_storage::{Entry, layout};

use super::*;
use crate::test_support::{nix_board, split_board};

/// A label no field given may hold.
const SECRET: [u8; 20] = *b"secret-label-0123456";

/// An entry with `seq`, `state` and a matching CRC, and [`SECRET`] as its
/// label.
fn entry(seq: u32, state: State) -> [u8; 32] {
    SelectEntry {
        seq,
        label: SECRET,
        state,
        crc: crc(seq),
    }
    .to_bytes()
}

/// A module over an otadata partition holding `first` and `second`, and the
/// flash.
fn module_of(first: &[u8; 32], second: &[u8; 32]) -> (Otadata<'static>, TestFlash) {
    let device = RamNor::with(
        layout::FLASH_SIZE,
        &[(layout::OTADATA.offset, &otadata_of(first, second))],
    );
    let flash = TestFlash::new(device);
    let reader = flash.reader::<roles::Otadata>(Span::from(layout::OTADATA));
    (Otadata::compiled(reader), flash)
}

fn fields(first: &[u8; 32], second: &[u8; 32], running: Option<Slot>) -> OtadataFields {
    let (module, _) = module_of(first, second);
    module.parse().unwrap().fields(running)
}

#[test]
fn the_nix_board_selects_its_first_slot() {
    let (storage, _) = split_board(nix_board(), layout::OTA_0.offset);
    let Handles::Boot(handles, _) = storage.handles else {
        panic!("{:?}", storage.report.tier);
    };
    let module = Otadata::compiled(handles.otadata);
    assert!(!module.is_found());
    assert_eq!(module.span(), Span::from(layout::OTADATA));
    let model = module.parse().unwrap();
    assert_eq!(model.active(), Some(Sector::First));
    assert_eq!(model.selected(), Some(Slot::Ota0));
    assert!(!model.boot_state(Slot::Ota0).needs_confirmation);

    let fields = model.fields(storage.report.slot);
    let first = fields.entry(Sector::First).unwrap();
    assert_eq!(
        (first.validity(), first.seq(), first.slot(), first.state()),
        (
            Validity::Valid,
            Some(1),
            Some(Slot::Ota0),
            Some(PublicState::Valid)
        )
    );
    let second = fields.entry(Sector::Second).unwrap();
    assert_eq!(
        (
            second.validity(),
            second.seq(),
            second.slot(),
            second.state()
        ),
        (Validity::Blank, None, None, None)
    );
    assert_eq!(fields.active(), Some(Sector::First));
    let boot = fields.boot().unwrap();
    assert_eq!(
        (
            boot.running(),
            boot.selected(),
            boot.state(),
            boot.rollback_target(),
            boot.needs_confirmation()
        ),
        (
            Slot::Ota0,
            Some(Slot::Ota0),
            Some(PublicState::Valid),
            false,
            false
        )
    );
}

#[test]
fn the_fields_never_hold_the_label() {
    let fields = fields(
        &entry(3, State::Valid),
        &entry(4, State::New),
        Some(Slot::Ota0),
    );
    let shown = format!("{fields:?}");
    assert!(!shown.contains("secret"), "{shown}");
    // Nor as the bytes' values.
    assert!(!shown.contains("115, 101, 99"), "{shown}");
    // The model keeps it, for the firmware.
    let (module, _) = module_of(&entry(3, State::Valid), &entry(4, State::New));
    let model = module.parse().unwrap();
    assert_eq!(model.otadata().entries[0].unwrap().label, SECRET);
}

#[test]
fn an_unknown_state_value_is_never_given() {
    let unknown = State::Unknown(0x00C0_FFEE);
    let fields = fields(&entry(5, unknown), &[0xFF; 32], Some(Slot::Ota0));
    let first = fields.entry(Sector::First).unwrap();
    // The entry is valid, as ESP-IDF treats an unknown state, but its state
    // isn't given.
    assert_eq!(
        (first.validity(), first.seq(), first.slot(), first.state()),
        (Validity::Valid, Some(5), Some(Slot::Ota0), None)
    );
    let boot = fields.boot().unwrap();
    assert_eq!(boot.state(), None);
    assert!(boot.needs_confirmation());
    let shown = format!("{fields:?}");
    assert!(!shown.contains(&format!("{}", 0x00C0_FFEE_u32)), "{shown}");
    assert!(!shown.contains("Unknown"), "{shown}");
}

#[test]
fn a_corrupt_entry_gives_its_validity_alone() {
    let mut corrupt = entry(9, State::Valid);
    corrupt[28] ^= 1;
    let fields = fields(&corrupt, &entry(2, State::Valid), Some(Slot::Ota1));
    let first = fields.entry(Sector::First).unwrap();
    assert_eq!(
        (first.validity(), first.seq(), first.slot(), first.state()),
        (Validity::Corrupt, None, None, None)
    );
    assert!(!format!("{fields:?}").contains('9'));
    assert_eq!(fields.active(), Some(Sector::Second));
}

#[test]
fn a_blank_entry_gives_its_validity_alone() {
    // Blank, whatever its other bytes hold.
    let mut blank = entry(u32::MAX, State::Valid);
    blank[24..28].copy_from_slice(&7_u32.to_le_bytes());
    let fields = fields(&blank, &[0xFF; 32], None);
    let first = fields.entry(Sector::First).unwrap();
    assert_eq!(
        (first.validity(), first.seq(), first.slot(), first.state()),
        (Validity::Blank, None, None, None)
    );
    assert_eq!(fields.boot(), None);
    assert_eq!(fields.active(), None);
}

#[test]
fn invalid_and_aborted_entries_give_their_fields() {
    let fields = fields(
        &entry(6, State::Invalid),
        &entry(7, State::Aborted),
        Some(Slot::Ota1),
    );
    let first = fields.entry(Sector::First).unwrap();
    assert_eq!(
        (first.validity(), first.seq(), first.slot(), first.state()),
        (
            Validity::Invalid,
            Some(6),
            Some(Slot::Ota1),
            Some(PublicState::Invalid)
        )
    );
    let second = fields.entry(Sector::Second).unwrap();
    assert_eq!(
        (
            second.validity(),
            second.seq(),
            second.slot(),
            second.state()
        ),
        (
            Validity::Aborted,
            Some(7),
            Some(Slot::Ota0),
            Some(PublicState::Aborted)
        )
    );
    // The bootloader fell back to the running slot: its own entry's state.
    let boot = fields.boot().unwrap();
    assert_eq!(boot.selected(), None);
    assert_eq!(boot.state(), Some(PublicState::Invalid));
    assert!(boot.needs_confirmation());
}

#[test]
fn every_defined_state_is_given() {
    for (state, public) in [
        (State::New, PublicState::New),
        (State::PendingVerify, PublicState::PendingVerify),
        (State::Valid, PublicState::Valid),
        (State::Invalid, PublicState::Invalid),
        (State::Aborted, PublicState::Aborted),
        (State::Undefined, PublicState::Undefined),
    ] {
        assert_eq!(PublicState::of(state), Some(public));
    }
    for raw in [5, 6, 0x100, u32::MAX - 1] {
        assert_eq!(PublicState::of(State::from_raw(raw)), None);
    }
    let fields = fields(
        &entry(1, State::PendingVerify),
        &[0xFF; 32],
        Some(Slot::Ota0),
    );
    let boot = fields.boot().unwrap();
    assert_eq!(boot.state(), Some(PublicState::PendingVerify));
    assert!(boot.needs_confirmation());
}

#[test]
fn an_entry_that_cant_be_read_is_given_as_such() {
    // A located otadata of one sector: the second entry lies past it.
    let device = RamNor::with(
        layout::FLASH_SIZE,
        &[(layout::OTA_1.offset, &entry(1, State::Valid))],
    );
    let flash = TestFlash::new(device);
    let reader = flash.reader::<FoundOtadata>(Span::new(layout::OTA_1.offset, 4096));
    let module = Otadata::found(reader);
    assert!(module.is_found());
    let fields = module.parse().unwrap().fields(None);
    assert_eq!(fields.entry(Sector::Second), Err(ReadError::OutOfRange));
    assert_eq!(
        fields.entry(Sector::First).unwrap().validity(),
        Validity::Valid
    );
}

#[test]
fn the_unverified_tier_reads_the_located_tables_otadata() {
    // The layout's partitions and one more, with the MD5 entry: the table
    // isn't the layout, so the tier is unverified.
    let mut slots: Vec<[u8; 32]> = layout::PARTITIONS.iter().map(entry_bytes).collect();
    slots.push(entry_bytes(&Entry {
        label: "extra",
        kind: TYPE_DATA,
        subtype: 0x06,
        offset: 0x7F_0000,
        size: 0x1_0000,
        flags: 0,
    }));
    slots.push(md5_entry_for(&slots));
    let device = nix_board();
    device.poke(layout::TABLE_OFFSET, &table_of(&slots));
    let (storage, _) = split_board(device, layout::OTA_0.offset);
    let Handles::Unverified(located) = storage.handles else {
        panic!("{:?}", storage.report.tier);
    };
    let module = Otadata::found(located.otadata.unwrap());
    assert_eq!(module.span(), Span::from(layout::OTADATA));
    let model = module.parse().unwrap();
    assert_eq!(model.selected(), Some(Slot::Ota0));
    assert_eq!(model.fields(None).boot(), None);
}

#[test]
fn a_write_moves_the_epoch_and_a_session_stops_the_parse() {
    let (module, flash) = module_of(&entry(1, State::Valid), &[0xFF; 32]);
    let mut writer = flash.writer::<roles::Otadata>(Span::from(layout::OTADATA), Slot::Ota0);
    let before = module.parse().unwrap();

    let mut session = writer.begin();
    assert_eq!(module.parse(), Err(LoadError::Busy));
    run(session.erase(4096, 8192)).unwrap();
    run(session.program(4096, &entry(2, State::New))).unwrap();
    drop(session);

    let after = module.parse().unwrap();
    assert_ne!(after.epoch(), before.epoch());
    assert_eq!(after.selected(), Some(Slot::Ota1));
    assert_eq!(before.selected(), Some(Slot::Ota0));
}

#[test]
fn a_session_between_the_entries_makes_the_parse_stale() {
    let (module, flash) = module_of(&entry(1, State::Valid), &entry(2, State::Valid));
    let mut writer = flash.writer::<roles::Otadata>(Span::from(layout::OTADATA), Slot::Ota0);
    // Before the read of the second entry.
    flash.device().before_call(1, move || {
        let mut session = writer.begin();
        run(session.erase(0, 4096)).unwrap();
    });
    assert_eq!(module.parse(), Err(LoadError::Stale));
    let model = module.parse().unwrap();
    assert_eq!(
        model.fields(None).entry(Sector::First).unwrap().validity(),
        Validity::Blank
    );
}

#[test]
fn the_module_can_be_shared_from_a_static() {
    fn shared<T: Sync + Send>() {}
    shared::<Otadata<'static>>();
}
