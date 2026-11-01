#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets, indices and counts are small"
)]

use super::*;
use crate::Slot::{Ota0, Ota1};
use crate::test_support::{
    Cut, IdfEntry, OTADATA_FIRST_BOOT, OTADATA_LEN, Rng, Tear, apply, apply_cut, boot, every_cut,
    idf_active, idf_slot, otadata_of, random_cut,
};

// Entries from the tests of esp-bootloader-esp-idf 0.6.0 (`src/ota.rs`),
// licensed MIT OR Apache-2.0: what that crate and ESP-IDF write.

const SLOT_INITIAL: [u8; 32] = [0xFF; 32];

const SLOT_COUNT_1_UNDEFINED: [u8; 32] = [
    1, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 154, 152, 67, 71,
];

const SLOT_COUNT_1_VALID: [u8; 32] = [
    1, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 2, 0, 0, 0, 154, 152, 67, 71,
];

const SLOT_COUNT_2_NEW: [u8; 32] = [
    2, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 0, 0, 0, 0, 116, 55, 246, 85,
];

const SLOT_COUNT_3_PENDING: [u8; 32] = [
    3, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 1, 0, 0, 0, 17, 80, 74, 237,
];

const ERASED: [u8; 32] = [0xFF; 32];

/// What the bootloader loads: both slots, or only one.
const LOADABLE: [[bool; 2]; 3] = [[true, true], [true, false], [false, true]];

/// The bytes of a consistent entry.
fn entry(seq: u32, state: State) -> [u8; 32] {
    SelectEntry::new(seq, state).to_bytes()
}

/// The bytes of an entry whose CRC is off by one bit.
fn bad_crc(seq: u32, state: State) -> [u8; 32] {
    let mut entry = SelectEntry::new(seq, state);
    entry.crc ^= 1;
    entry.to_bytes()
}

/// Sequence number 0 followed by erased bytes, which is a valid entry: the
/// CRC of 0 is `0xFFFFFFFF`.
fn seq_zero_then_erased() -> [u8; 32] {
    let mut bytes = ERASED;
    bytes[..4].fill(0);
    bytes
}

fn pair(first: &[u8; 32], second: &[u8; 32]) -> Otadata {
    read(&otadata_of(first, second)[..])
}

/// The entries the selection and power-cut tests combine: erased, garbage,
/// each state at a few sequence numbers, a bad CRC, and the edges of the
/// sequence numbers.
fn kinds() -> Vec<(String, [u8; 32])> {
    let mut garbage = [0; 32];
    Rng::new(0x007A_DA7A).fill(&mut garbage);
    let mut kinds = vec![
        ("erased".to_owned(), ERASED),
        ("zeros".to_owned(), [0; 32]),
        ("garbage".to_owned(), garbage),
        ("seq 0, erased".to_owned(), seq_zero_then_erased()),
        ("seq 0 Valid".to_owned(), entry(0, State::Valid)),
        ("seq MAX Valid".to_owned(), entry(BLANK_SEQ, State::Valid)),
        (
            "seq MAX-1 Valid".to_owned(),
            entry(BLANK_SEQ - 1, State::Valid),
        ),
        ("seq MAX-1 New".to_owned(), entry(BLANK_SEQ - 1, State::New)),
    ];
    let states = [
        State::New,
        State::PendingVerify,
        State::Valid,
        State::Invalid,
        State::Aborted,
        State::Undefined,
        State::Unknown(0x1234_5678),
    ];
    for seq in [1, 2, 3, 6] {
        for state in states {
            kinds.push((format!("seq {seq} {state:?}"), entry(seq, state)));
        }
        kinds.push((
            format!("seq {seq} Valid, bad CRC"),
            bad_crc(seq, State::Valid),
        ));
    }
    kinds
}

// The CRC.

/// The CRC computed a byte at a time, as `esp_rom_crc32_le` does.
fn crc_bytewise(seq: u32) -> u32 {
    let mut crc = !u32::MAX;
    for byte in seq.to_le_bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[test]
fn the_crc_matches_the_vectors() {
    assert_eq!(crc(1).to_le_bytes(), [154, 152, 67, 71]);
    assert_eq!(crc(1), 0x4743_989A);
    assert_eq!(crc(2).to_le_bytes(), [116, 55, 246, 85]);
    assert_eq!(crc(3).to_le_bytes(), [17, 80, 74, 237]);
}

#[test]
fn the_crc_of_0_is_the_erased_value() {
    assert_eq!(crc(0), 0xFFFF_FFFF);
    assert_eq!(crc(BLANK_SEQ), 0x2144_DF1C);
}

#[test]
fn the_crc_equals_the_bytewise_crc() {
    let mut rng = Rng::new(0xC4C);
    for seq in (0..1000).chain([u32::MAX - 1, u32::MAX, 0x8000_0000]) {
        assert_eq!(crc(seq), crc_bytewise(seq), "{seq:#x}");
    }
    for _ in 0..10_000 {
        let seq = rng.next_u32();
        assert_eq!(crc(seq), crc_bytewise(seq), "{seq:#x}");
    }
}

#[test]
fn only_seq_0_has_an_erased_crc() {
    // The CRC of a 32-bit word is a bijection, so the preimage of the erased
    // value is unique.
    let mut rng = Rng::new(0xE2A5);
    for _ in 0..100_000 {
        let seq = rng.next_u32();
        assert!(seq == 0 || crc(seq) != 0xFFFF_FFFF);
    }
}

// Entries.

#[test]
fn every_vector_parses_to_its_seq_and_state() {
    let vectors = [
        (SLOT_COUNT_1_UNDEFINED, 1, State::Undefined, Ota0),
        (SLOT_COUNT_1_VALID, 1, State::Valid, Ota0),
        (SLOT_COUNT_2_NEW, 2, State::New, Ota1),
        (SLOT_COUNT_3_PENDING, 3, State::PendingVerify, Ota0),
    ];
    for (bytes, seq, state, slot) in vectors {
        let parsed = SelectEntry::from_bytes(&bytes);
        assert_eq!(parsed.seq, seq);
        assert_eq!(parsed.state, state);
        assert_eq!(parsed.label, [0xFF; 20]);
        assert_eq!(parsed.crc, crc(seq));
        assert_eq!(parsed.validity(), Validity::Valid);
        assert_eq!(parsed.slot(), slot);
        assert_eq!(parsed.to_bytes(), bytes);
        // What the plans write is what ESP-IDF writes.
        assert_eq!(SelectEntry::new(seq, state).to_bytes(), bytes);
    }

    let initial = SelectEntry::from_bytes(&SLOT_INITIAL);
    assert_eq!(initial.seq, BLANK_SEQ);
    assert_eq!(initial.state, State::Undefined);
    assert_eq!(initial.crc, 0xFFFF_FFFF);
    assert_eq!(initial.validity(), Validity::Blank);
}

#[test]
fn any_32_bytes_parse_and_store_back_unchanged() {
    let mut rng = Rng::new(0x005E_1EC7);
    for _ in 0..10_000 {
        let mut bytes = [0; 32];
        rng.fill(&mut bytes);
        assert_eq!(SelectEntry::from_bytes(&bytes).to_bytes(), bytes);
    }
}

#[test]
fn states_keep_their_raw_values() {
    let named = [
        (0, State::New),
        (1, State::PendingVerify),
        (2, State::Valid),
        (3, State::Invalid),
        (4, State::Aborted),
        (u32::MAX, State::Undefined),
    ];
    for (raw, state) in named {
        assert_eq!(State::from_raw(raw), state);
        assert_eq!(state.raw(), raw);
    }
    for raw in [5, 0x1234_5678, u32::MAX - 1] {
        assert_eq!(State::from_raw(raw), State::Unknown(raw));
        assert_eq!(State::from_raw(raw).raw(), raw);
    }
}

#[test]
fn validity_is_judged_in_order() {
    let judged = |bytes: [u8; 32]| SelectEntry::from_bytes(&bytes).validity();
    // A blank sequence number wins over everything else.
    assert_eq!(judged(ERASED), Validity::Blank);
    assert_eq!(judged(entry(BLANK_SEQ, State::Valid)), Validity::Blank);
    assert_eq!(judged(bad_crc(BLANK_SEQ, State::Aborted)), Validity::Blank);
    // Then the CRC, then the state.
    assert_eq!(judged(bad_crc(5, State::Aborted)), Validity::Corrupt);
    assert_eq!(judged(entry(5, State::Invalid)), Validity::Invalid);
    assert_eq!(judged(entry(5, State::Aborted)), Validity::Aborted);
    for state in [
        State::New,
        State::PendingVerify,
        State::Valid,
        State::Undefined,
        State::Unknown(9),
    ] {
        assert_eq!(judged(entry(5, state)), Validity::Valid, "{state:?}");
    }
    assert_eq!(judged([0; 32]), Validity::Corrupt);
}

#[test]
fn seq_selects_a_slot_in_wrapping_arithmetic() {
    let slot = |seq| SelectEntry::new(seq, State::Valid).slot();
    assert_eq!(slot(1), Ota0);
    assert_eq!(slot(2), Ota1);
    assert_eq!(slot(3), Ota0);
    assert_eq!(slot(0), Ota1);
    assert_eq!(slot(BLANK_SEQ - 1), Ota1);
    assert_eq!(slot(BLANK_SEQ - 2), Ota0);
    let mut rng = Rng::new(0x5107);
    for _ in 0..1000 {
        let seq = rng.next_u32();
        assert_eq!(slot(seq), idf_slot(seq));
    }
}

#[test]
fn seq_0_followed_by_erased_bytes_is_a_valid_undefined_entry() {
    for (first, second) in [
        (seq_zero_then_erased(), ERASED),
        (ERASED, seq_zero_then_erased()),
    ] {
        let otadata = pair(&first, &second);
        let active = otadata.active().unwrap();
        let entry = otadata.valid(active).unwrap();
        assert_eq!(entry.seq, 0);
        assert_eq!(entry.state, State::Undefined);
        assert_eq!(otadata.selected(), Some(Ota1));
    }
    // Any other sequence number with an erased CRC is corrupt.
    let mut bytes = ERASED;
    bytes[..4].copy_from_slice(&1_u32.to_le_bytes());
    assert_eq!(
        SelectEntry::from_bytes(&bytes).validity(),
        Validity::Corrupt
    );
}

// Reading.

/// The otadata partition with one sector that can't be read.
struct Failing {
    bytes: Vec<u8>,
    sector: Sector,
}

impl Source for Failing {
    fn size(&self) -> u32 {
        self.bytes.len() as u32
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        if offset / SECTOR_SIZE == self.sector.index() as u32 {
            return Err(ReadError::Device);
        }
        self.bytes[..].read(offset, buf)
    }
}

#[test]
fn each_entry_is_read_from_the_start_of_its_sector() {
    let flash = otadata_of(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    let otadata = read(&flash[..]);
    assert_eq!(otadata.entry(Sector::First).unwrap().seq, 1);
    assert_eq!(otadata.entry(Sector::Second).unwrap().seq, 2);
    assert_eq!(Sector::First.offset(), 0);
    assert_eq!(Sector::Second.offset(), 0x1000);
    assert_eq!(Sector::ALL.map(Sector::index), [0, 1]);
    assert_eq!(Sector::First.other(), Sector::Second);
    assert_eq!(Sector::Second.other(), Sector::First);
}

#[test]
fn a_read_error_is_kept_for_its_entry_only() {
    let bytes = otadata_of(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    let first = read(&Failing {
        bytes: bytes.clone(),
        sector: Sector::First,
    });
    assert_eq!(first.entry(Sector::First), Err(ReadError::Device));
    assert_eq!(first.active(), Some(Sector::Second));
    assert_eq!(first.selected(), Some(Ota1));

    let second = read(&Failing {
        bytes,
        sector: Sector::Second,
    });
    assert_eq!(second.entry(Sector::Second), Err(ReadError::Device));
    assert_eq!(second.active(), Some(Sector::First));
    assert_eq!(second.selected(), Some(Ota0));
}

#[test]
fn a_short_source_leaves_the_second_entry_out_of_range() {
    let bytes = otadata_of(&SLOT_COUNT_1_VALID, &ERASED);
    let otadata = read(&bytes[..0x1010]);
    assert_eq!(otadata.entry(Sector::Second), Err(ReadError::OutOfRange));
    assert_eq!(otadata.selected(), Some(Ota0));
    let empty = read(&[][..]);
    assert_eq!(empty.entries, [Err(ReadError::OutOfRange); 2]);
    assert_eq!(empty.active(), None);
}

// Selection.

/// The active sector of `first` and `second`, which have to give the mirror
/// result when swapped unless their sequence numbers tie.
fn active_of(first: [u8; 32], second: [u8; 32]) -> Option<Sector> {
    pair(&first, &second).active()
}

#[test]
fn the_active_entry_is_esp_idfs_for_every_pair() {
    let kinds = kinds();
    for (first_name, first) in &kinds {
        for (second_name, second) in &kinds {
            let flash = otadata_of(first, second);
            let ours = read(&flash[..]);
            let idf = [IdfEntry::read(&flash, 0), IdfEntry::read(&flash, 1)];
            let expected = idf_active(&idf);
            let case = format!("{first_name} / {second_name}");
            assert_eq!(ours.active().map(Sector::index), expected, "{case}");
            assert_eq!(
                ours.selected(),
                expected.map(|i| idf_slot(idf[i].seq)),
                "{case}"
            );
            for sector in Sector::ALL {
                assert_eq!(
                    ours.valid(sector).is_some(),
                    idf[sector.index()].valid(),
                    "{case}, {sector:?}"
                );
            }
        }
    }
}

#[test]
fn the_higher_valid_seq_is_active_in_either_sector() {
    let low = entry(1, State::Valid);
    let high = entry(2, State::New);
    assert_eq!(active_of(low, high), Some(Sector::Second));
    assert_eq!(active_of(high, low), Some(Sector::First));
    assert_eq!(pair(&low, &high).selected(), Some(Ota1));

    let top = entry(BLANK_SEQ - 1, State::Valid);
    assert_eq!(active_of(low, top), Some(Sector::Second));
    assert_eq!(active_of(top, low), Some(Sector::First));
}

#[test]
fn equal_seqs_tie_to_the_first_sector() {
    let first = entry(4, State::Valid);
    let second = entry(4, State::New);
    let otadata = pair(&first, &second);
    assert_eq!(otadata.active(), Some(Sector::First));
    assert_eq!(otadata.active_entry().unwrap().state, State::Valid);
    assert_eq!(active_of(second, first), Some(Sector::First));
}

#[test]
fn an_entry_that_isnt_valid_is_never_active() {
    let valid = entry(1, State::Valid);
    let not_valid = [
        ERASED,
        [0; 32],
        entry(5, State::Invalid),
        entry(5, State::Aborted),
        bad_crc(5, State::Valid),
        entry(BLANK_SEQ, State::Valid),
    ];
    for bytes in not_valid {
        assert_eq!(active_of(valid, bytes), Some(Sector::First));
        assert_eq!(active_of(bytes, valid), Some(Sector::Second));
        assert_eq!(active_of(bytes, bytes), None);
        assert_eq!(active_of(bytes, ERASED), None);
    }
}

#[test]
fn an_unknown_state_is_valid_and_can_be_active() {
    let unknown = entry(7, State::Unknown(0x1234_5678));
    let otadata = pair(&entry(6, State::Valid), &unknown);
    assert_eq!(otadata.active(), Some(Sector::Second));
    assert_eq!(
        otadata.boot_state(Ota0).state,
        Some(State::Unknown(0x1234_5678))
    );
}

#[test]
fn erased_otadata_selects_nothing() {
    let otadata = pair(&ERASED, &ERASED);
    assert_eq!(otadata.active(), None);
    assert_eq!(otadata.selected(), None);
    for sector in Sector::ALL {
        assert_eq!(otadata.entry(sector).unwrap().validity(), Validity::Blank);
    }
}

// Boot state.

#[test]
fn the_running_image_takes_the_state_of_the_active_entry_selecting_it() {
    // Activated, booted once, not yet confirmed; the old image is the target.
    let otadata = pair(&SLOT_COUNT_1_VALID, &entry(2, State::PendingVerify));
    assert_eq!(
        otadata.boot_state(Ota1),
        BootState {
            running: Ota1,
            selected: Some(Ota1),
            state: Some(State::PendingVerify),
            rollback_target: true,
            needs_confirmation: true,
        }
    );
}

#[test]
fn an_activation_not_yet_booted_leaves_the_running_image_its_own_state() {
    let otadata = pair(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    assert_eq!(
        otadata.boot_state(Ota0),
        BootState {
            running: Ota0,
            selected: Some(Ota1),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        }
    );
}

#[test]
fn after_a_fallback_the_running_image_may_be_aborted_or_have_no_entry() {
    // The bootloader aborted ota_1 and, with nothing valid, started ota_0,
    // whose entry is aborted too.
    let otadata = pair(&entry(1, State::Aborted), &entry(2, State::Aborted));
    let state = otadata.boot_state(Ota0);
    assert_eq!(state.selected, None);
    assert_eq!(state.state, Some(State::Aborted));
    assert!(!state.rollback_target);
    assert!(state.needs_confirmation);

    // No entry names ota_1 with a matching CRC.
    let state = pair(&bad_crc(2, State::Valid), &ERASED).boot_state(Ota1);
    assert_eq!(state.selected, None);
    assert_eq!(state.state, None);
    assert!(state.needs_confirmation);

    // Of two entries naming the running slot, the higher one's state.
    let state = pair(&entry(3, State::Invalid), &entry(1, State::Aborted)).boot_state(Ota0);
    assert_eq!(state.state, Some(State::Invalid));
}

#[test]
fn only_a_valid_image_is_confirmed() {
    let needs = |first: [u8; 32], second: [u8; 32], running: Slot| {
        pair(&first, &second).boot_state(running).needs_confirmation
    };
    // Selected and Valid; Valid beside an activation not yet booted.
    assert!(!needs(SLOT_COUNT_1_VALID, ERASED, Ota0));
    assert!(!needs(SLOT_COUNT_1_VALID, SLOT_COUNT_2_NEW, Ota0));
    // Selected in any other state.
    for state in [
        State::New,
        State::PendingVerify,
        State::Undefined,
        State::Unknown(9),
    ] {
        assert!(needs(entry(1, state), ERASED, Ota0), "{state:?}");
    }
    // A fallback: the selected slot was tried and didn't load, or was
    // confirmed and decayed, whatever the running slot's own entry says.
    assert!(needs(
        SLOT_COUNT_1_VALID,
        entry(2, State::PendingVerify),
        Ota0
    ));
    assert!(needs(SLOT_COUNT_1_VALID, entry(2, State::Valid), Ota0));
    assert!(needs(
        entry(1, State::Aborted),
        entry(2, State::Valid),
        Ota0
    ));
    assert!(needs(ERASED, entry(2, State::Valid), Ota0));
    // An activation not yet booted beside an unconfirmed image.
    assert!(needs(entry(1, State::Undefined), SLOT_COUNT_2_NEW, Ota0));
    // No valid entry.
    assert!(needs(
        entry(1, State::Aborted),
        entry(2, State::Aborted),
        Ota1
    ));
    assert!(needs(ERASED, ERASED, Ota0));
}

#[test]
fn the_other_slot_is_a_rollback_target_only_through_a_valid_entry_naming_it() {
    let running = entry(2, State::Valid);
    let targets = [
        (entry(1, State::Valid), true),
        (entry(1, State::New), true),
        (entry(1, State::Undefined), true),
        (entry(1, State::Invalid), false),
        (entry(1, State::Aborted), false),
        (bad_crc(1, State::Valid), false),
        (ERASED, false),
        // Naming the running slot, not the other.
        (entry(0, State::Valid), false),
    ];
    for (other, target) in targets {
        assert_eq!(
            pair(&other, &running).boot_state(Ota1).rollback_target,
            target,
            "{:?}",
            SelectEntry::from_bytes(&other)
        );
    }
    // The active entry has to name the running slot.
    assert!(
        !pair(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW)
            .boot_state(Ota0)
            .rollback_target
    );
}

// Sequence numbers.

/// `esp_rewrite_ota_data`'s number for `slot` above `seq`: the smallest
/// `(slot + 1) % 2 + 2i` that isn't below `seq`, which its loop over `i`
/// reaches for every `seq` below [`BLANK_SEQ`].
fn idf_seq(seq: u32, slot: Slot) -> u32 {
    let base = u64::from((slot.number() + 1) % 2);
    let seq = u64::from(seq);
    let i = if seq > base {
        (seq - base).div_ceil(2)
    } else {
        0
    };
    u32::try_from(base + 2 * i).unwrap()
}

#[test]
fn the_next_seq_is_esp_idfs_or_two_more_when_the_slot_is_selected() {
    let mut rng = Rng::new(0x5E9);
    let seqs = (1..2000)
        .chain(BLANK_SEQ - 2000..BLANK_SEQ)
        .chain((0..2000).map(|_| rng.next_u32() >> 1));
    for seq in seqs {
        for slot in Slot::ALL {
            let ours = seq_after(seq, slot);
            let theirs = idf_seq(seq, slot);
            if slot_of(seq) == slot {
                assert_eq!(theirs, seq, "{seq:#x} {slot:?}");
                assert_eq!(ours, seq.checked_add(2).filter(|&s| s != BLANK_SEQ));
            } else if theirs == BLANK_SEQ {
                assert_eq!(ours, None, "{seq:#x} {slot:?}");
            } else {
                assert_eq!(ours, Some(theirs), "{seq:#x} {slot:?}");
            }
            if let Some(next) = ours {
                assert!(next > seq);
                assert_eq!(slot_of(next), slot);
            }
        }
    }
}

#[test]
fn the_next_seq_never_reaches_the_blank_value() {
    assert_eq!(seq_after(BLANK_SEQ - 1, Ota0), None);
    assert_eq!(seq_after(BLANK_SEQ - 1, Ota1), None);
    assert_eq!(seq_after(BLANK_SEQ - 2, Ota1), Some(BLANK_SEQ - 1));
    assert_eq!(seq_after(BLANK_SEQ - 2, Ota0), None);
    assert_eq!(seq_after(0, Ota0), Some(1));
    assert_eq!(seq_after(0, Ota1), Some(2));
}

// Plans.

#[test]
fn activation_writes_new_to_the_sector_that_isnt_active() {
    let otadata = read(OTADATA_FIRST_BOOT);
    let write = otadata.plan_activate(Ota0).unwrap();
    assert_eq!(write.sector(), Sector::Second);
    assert_eq!(write.offset(), 0x1000);
    assert_eq!(write.entry(), SelectEntry::new(2, State::New));
    // The entry esp-bootloader-esp-idf reaches in two writes, in one.
    assert_eq!(write.bytes(), SLOT_COUNT_2_NEW);

    // Seq + 1 from either sector.
    let otadata = pair(&entry(4, State::Valid), &entry(3, State::Valid));
    let write = otadata.plan_activate(Ota1).unwrap();
    assert_eq!(write.sector(), Sector::Second);
    assert_eq!(write.entry(), SelectEntry::new(5, State::New));
}

#[test]
fn activation_refuses_unless_the_running_slot_is_selected_and_confirmed() {
    let pending = pair(&SLOT_COUNT_1_VALID, &entry(2, State::PendingVerify));
    assert_eq!(pending.plan_activate(Ota1), Err(PlanError::Unconfirmed));
    let undefined = pair(&SLOT_COUNT_1_UNDEFINED, &ERASED);
    assert_eq!(undefined.plan_activate(Ota0), Err(PlanError::Unconfirmed));
    let elsewhere = pair(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    assert_eq!(elsewhere.plan_activate(Ota0), Err(PlanError::NotSelected));
    let none = pair(&ERASED, &entry(1, State::Aborted));
    assert_eq!(none.plan_activate(Ota0), Err(PlanError::NotSelected));
    assert_eq!(none.plan_activate(Ota1), Err(PlanError::NotSelected));
    // 0xFFFFFFFE selects ota_1; 0xFFFFFFFF would be next for ota_0.
    let exhausted = pair(&entry(BLANK_SEQ - 1, State::Valid), &ERASED);
    assert_eq!(exhausted.plan_activate(Ota1), Err(PlanError::SeqExhausted));
    assert_eq!(
        exhausted.plan_begin_update(Ota1),
        Err(PlanError::SeqExhausted)
    );
    let last = pair(&entry(BLANK_SEQ - 2, State::Valid), &ERASED);
    assert_eq!(last.plan_activate(Ota0).unwrap().entry().seq, BLANK_SEQ - 1);
}

#[test]
fn beginning_an_update_settles_both_entries_on_the_running_slot() {
    // From the first boot: the second sector gets a newer entry for ota_0.
    let otadata = read(OTADATA_FIRST_BOOT);
    let write = otadata.plan_begin_update(Ota0).unwrap().unwrap();
    assert_eq!(write.sector(), Sector::Second);
    assert_eq!(write.entry(), SelectEntry::new(3, State::Valid));
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    apply(&mut flash, &write);
    assert_eq!(read(&flash[..]).plan_begin_update(Ota0), Ok(None));

    // A confirmed update with its rollback target: the target is replaced.
    let otadata = pair(&SLOT_COUNT_1_VALID, &entry(2, State::Valid));
    let write = otadata.plan_begin_update(Ota1).unwrap().unwrap();
    assert_eq!(write.sector(), Sector::First);
    assert_eq!(write.entry(), SelectEntry::new(4, State::Valid));

    // An activation not yet booted is cancelled in place, then replaced.
    let mut flash = otadata_of(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    let first = read(&flash[..]).plan_begin_update(Ota0).unwrap().unwrap();
    assert_eq!(first.sector(), Sector::Second);
    assert_eq!(first.entry(), SelectEntry::new(2, State::Invalid));
    apply(&mut flash, &first);
    assert_eq!(read(&flash[..]).selected(), Some(Ota0));
    let second = read(&flash[..]).plan_begin_update(Ota0).unwrap().unwrap();
    assert_eq!(second.sector(), Sector::Second);
    assert_eq!(second.entry(), SelectEntry::new(3, State::Valid));
    apply(&mut flash, &second);
    assert_eq!(read(&flash[..]).plan_begin_update(Ota0), Ok(None));

    // With nothing valid, nothing is confirmed.
    let none = pair(&entry(1, State::Aborted), &entry(2, State::Aborted));
    assert_eq!(none.plan_begin_update(Ota1), Err(PlanError::Unconfirmed));
}

#[test]
fn beginning_an_update_refuses_until_the_running_image_is_confirmed() {
    let unconfirmed = [
        // Booted once, pending.
        (SLOT_COUNT_1_VALID, entry(2, State::PendingVerify), Ota1),
        // No state.
        (SLOT_COUNT_1_UNDEFINED, ERASED, Ota0),
        // Fallbacks from ota_1: its own entry valid, aborted, none.
        (SLOT_COUNT_1_VALID, entry(2, State::PendingVerify), Ota0),
        (entry(1, State::Aborted), entry(2, State::Valid), Ota0),
        (ERASED, entry(2, State::PendingVerify), Ota0),
        // No valid entry.
        (entry(3, State::Aborted), bad_crc(4, State::Valid), Ota1),
    ];
    for (first, second, running) in unconfirmed {
        let mut flash = otadata_of(&first, &second);
        let otadata = read(&flash[..]);
        assert!(otadata.boot_state(running).needs_confirmation);
        assert_eq!(
            otadata.plan_begin_update(running),
            Err(PlanError::Unconfirmed)
        );
        // Confirmed, it settles without another Valid entry for an image
        // that wasn't.
        run_plan(&mut flash, |otadata| otadata.plan_confirm(running));
        let state = read(&flash[..]).boot_state(running);
        assert_eq!(state.selected, Some(running));
        assert_eq!(state.state, Some(State::Valid));
        assert!(!state.needs_confirmation);
        while let Some(write) = read(&flash[..]).plan_begin_update(running).unwrap() {
            assert_eq!(write.entry().slot(), running);
            apply(&mut flash, &write);
        }
        for sector in Sector::ALL {
            let entry = read(&flash[..]).valid(sector).unwrap();
            assert_eq!((entry.slot(), entry.state), (running, State::Valid));
        }
    }
}

#[test]
fn confirming_rewrites_the_active_entry_selecting_the_running_slot_in_place() {
    let mut pending = SelectEntry::new(6, State::PendingVerify);
    pending.label = *b"label kept as stored";
    let otadata = pair(&entry(5, State::Valid), &pending.to_bytes());
    let write = otadata.plan_confirm(Ota1).unwrap().unwrap();
    assert_eq!(write.sector(), Sector::Second);
    assert_eq!(write.entry().seq, 6);
    assert_eq!(write.entry().state, State::Valid);
    assert_eq!(write.entry().label, *b"label kept as stored");
    assert_eq!(write.entry().crc, crc(6));

    // Already valid: nothing to write.
    assert_eq!(read(OTADATA_FIRST_BOOT).plan_confirm(Ota0), Ok(None));
    // Undefined, unknown and new states become valid too.
    let undefined = pair(&SLOT_COUNT_1_UNDEFINED, &ERASED);
    assert_eq!(
        undefined.plan_confirm(Ota0).unwrap().unwrap().bytes(),
        SLOT_COUNT_1_VALID
    );
    for state in [State::Unknown(9), State::New] {
        let write = pair(&entry(1, state), &ERASED).plan_confirm(Ota0).unwrap();
        assert_eq!(write.unwrap().entry(), SelectEntry::new(1, State::Valid));
    }
}

#[test]
fn confirming_after_a_fallback_writes_an_entry_for_the_running_slot_beside_the_active_one() {
    // ota_1 didn't load: tried once, confirmed and decayed, with ota_0's own
    // entry valid, aborted or missing.
    let fallbacks = [
        (SLOT_COUNT_1_VALID, entry(2, State::PendingVerify)),
        (entry(1, State::Aborted), entry(2, State::Valid)),
        (ERASED, entry(2, State::Valid)),
        (entry(5, State::Invalid), entry(8, State::Valid)),
    ];
    for (first, second) in fallbacks {
        let otadata = pair(&first, &second);
        let write = otadata.plan_confirm(Ota0).unwrap().unwrap();
        let active = otadata.active_entry().unwrap();
        assert_eq!(write.sector(), Sector::First);
        assert_eq!(write.entry().slot(), Ota0);
        assert_eq!(write.entry().state, State::Valid);
        assert_eq!(write.entry().seq, active.seq + 1);
        let mut flash = otadata_of(&first, &second);
        apply(&mut flash, &write);
        let state = read(&flash[..]).boot_state(Ota0);
        assert_eq!(state.selected, Some(Ota0));
        assert!(!state.needs_confirmation);
        // The other slot no longer loads, so it stays selected.
        assert_eq!(boot(&mut flash, [true, false]), Some(Ota0));
        assert_eq!(read(&flash[..]).boot_state(Ota0).state, Some(State::Valid));
    }
    // An activation not yet booted beside a confirmed image stays.
    let activation = pair(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    assert_eq!(activation.plan_confirm(Ota0), Ok(None));
}

#[test]
fn confirming_with_no_valid_entry_writes_the_bootloader_s_first_entry() {
    for (first, second) in [
        (entry(1, State::Aborted), entry(2, State::Aborted)),
        (entry(3, State::Aborted), bad_crc(4, State::Valid)),
        (ERASED, entry(2, State::Invalid)),
    ] {
        for running in [Ota0, Ota1] {
            let write = pair(&first, &second)
                .plan_confirm(running)
                .unwrap()
                .unwrap();
            assert_eq!(write.sector(), Sector::First);
            assert_eq!(
                write.entry(),
                SelectEntry::new(running.number() + 1, State::Valid)
            );
        }
    }
    // What the bootloader writes on blank otadata after starting ota_0.
    let write = pair(&ERASED, &ERASED).plan_confirm(Ota0).unwrap().unwrap();
    let mut flash = otadata_of(&ERASED, &ERASED);
    apply(&mut flash, &write);
    assert_eq!(flash, OTADATA_FIRST_BOOT);
}

#[test]
fn rolling_back_marks_the_running_image_invalid_in_place() {
    let otadata = pair(&SLOT_COUNT_1_VALID, &entry(2, State::PendingVerify));
    let write = otadata.plan_roll_back(Ota1).unwrap();
    assert_eq!(write.sector(), Sector::Second);
    assert_eq!(write.entry(), SelectEntry::new(2, State::Invalid));
    let mut flash = otadata_of(&SLOT_COUNT_1_VALID, &entry(2, State::PendingVerify));
    apply(&mut flash, &write);
    assert_eq!(read(&flash[..]).selected(), Some(Ota0));
}

#[test]
fn rolling_back_refuses_without_a_target() {
    // The first boot: nothing to go back to.
    let first_boot = read(OTADATA_FIRST_BOOT);
    assert_eq!(
        first_boot.plan_roll_back(Ota0),
        Err(PlanError::NoRollbackTarget)
    );
    // The target was aborted.
    let aborted = pair(&entry(1, State::Aborted), &entry(2, State::Valid));
    assert_eq!(
        aborted.plan_roll_back(Ota1),
        Err(PlanError::NoRollbackTarget)
    );
    // Both entries name the running slot.
    let same = pair(&entry(1, State::Valid), &entry(3, State::Valid));
    assert_eq!(same.plan_roll_back(Ota0), Err(PlanError::NoRollbackTarget));
    // The running slot isn't the selected one.
    let elsewhere = pair(&SLOT_COUNT_1_VALID, &SLOT_COUNT_2_NEW);
    assert_eq!(elsewhere.plan_roll_back(Ota0), Err(PlanError::NotSelected));
}

#[test]
fn a_read_error_blocks_a_plan_only_in_the_sector_it_keeps() {
    let failing = |first: [u8; 32], second: [u8; 32], sector| {
        read(&Failing {
            bytes: otadata_of(&first, &second),
            sector,
        })
    };
    let read_error = |sector| PlanError::Read {
        sector,
        error: ReadError::Device,
    };

    // The sector overwritten can't be read: no matter.
    let otadata = failing(SLOT_COUNT_1_VALID, ERASED, Sector::Second);
    assert_eq!(
        otadata.plan_activate(Ota0).unwrap().sector(),
        Sector::Second
    );
    assert_eq!(
        otadata.plan_begin_update(Ota0).unwrap().unwrap().sector(),
        Sector::Second
    );
    let otadata = failing(ERASED, ERASED, Sector::First);
    assert_eq!(
        otadata.plan_confirm(Ota1).unwrap().unwrap().sector(),
        Sector::First
    );

    // The sector kept can't be read: refused.
    let otadata = failing(ERASED, ERASED, Sector::Second);
    assert_eq!(otadata.plan_confirm(Ota0), Err(read_error(Sector::Second)));
    let otadata = failing(SLOT_COUNT_1_UNDEFINED, ERASED, Sector::Second);
    assert_eq!(otadata.plan_confirm(Ota0), Err(read_error(Sector::Second)));
    assert_eq!(
        otadata.plan_roll_back(Ota0),
        Err(read_error(Sector::Second))
    );
}

// The four faults of esp-bootloader-esp-idf 0.6.0.

#[test]
fn fault_1_a_corrupt_entry_stops_neither_reading_nor_writing() {
    let mut corrupt = SLOT_COUNT_1_UNDEFINED;
    corrupt[31] ^= 0xFF;
    let mut garbage = [0; 32];
    Rng::new(0xBAD).fill(&mut garbage);
    for bad in [corrupt, garbage, [0; 32]] {
        // That crate fails every operation here; this reads the other entry.
        let otadata = pair(&bad, &SLOT_COUNT_2_NEW);
        assert_eq!(otadata.active(), Some(Sector::Second));
        assert_eq!(otadata.selected(), Some(Ota1));

        // And plans writes: the bootloader's step, then confirming.
        let mut flash = otadata_of(&bad, &SLOT_COUNT_2_NEW);
        assert_eq!(boot(&mut flash, [true, true]), Some(Ota1));
        let otadata = read(&flash[..]);
        assert_eq!(otadata.boot_state(Ota1).state, Some(State::PendingVerify));
        let write = otadata.plan_confirm(Ota1).unwrap().unwrap();
        assert_eq!(write.sector(), Sector::Second);
        assert_eq!(otadata.plan_begin_update(Ota1), Err(PlanError::Unconfirmed));
        apply(&mut flash, &write);
        let write = read(&flash[..]).plan_begin_update(Ota1).unwrap().unwrap();
        assert_eq!(write.sector(), Sector::First);
    }
}

#[test]
fn fault_2_an_aborted_or_invalid_higher_entry_is_never_the_current_one() {
    for bad in [State::Aborted, State::Invalid] {
        let otadata = pair(&SLOT_COUNT_1_VALID, &entry(2, bad));
        // That crate takes the highest sequence number, ota_1.
        assert_eq!(otadata.selected(), Some(Ota0));
        assert_eq!(otadata.boot_state(Ota0).state, Some(State::Valid));
        // The running image is confirmed already: nothing is written.
        assert_eq!(otadata.plan_confirm(Ota0), Ok(None));
    }
}

#[test]
fn fault_2_marking_valid_after_a_rollback_doesnt_revive_the_bad_image() {
    // Activate ota_1, boot it once, reboot without confirming it.
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota1));
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
    let otadata = read(&flash[..]);
    assert_eq!(otadata.entry(Sector::Second).unwrap().state, State::Aborted);

    // Confirming the running image changes nothing, and ota_0 stays.
    assert_eq!(otadata.plan_confirm(Ota0), Ok(None));
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
}

#[test]
fn fault_3_activation_writes_new_with_the_seq() {
    let flash = OTADATA_FIRST_BOOT.to_vec();
    let write = read(&flash[..]).plan_activate(Ota0).unwrap();
    // One write holds sequence number, state and CRC.
    assert_eq!(write.entry().state, State::New);
    // Every cut of it leaves no valid entry, so the bootloader never sees
    // the new sequence number with another state.
    for cut in every_cut(&write).into_iter().map(Some).chain([None]) {
        let mut flash = flash.clone();
        apply_cut(&mut flash, &write, cut);
        let otadata = read(&flash[..]);
        match otadata.valid(Sector::Second) {
            Some(entry) => assert_eq!((cut, entry), (None, write.entry())),
            None => assert!(cut.is_some()),
        }
        let booted = boot(&mut flash, [true, true]);
        if cut.is_some() {
            assert_eq!(booted, Some(Ota0));
        } else {
            assert_eq!(booted, Some(Ota1));
            let state = read(&flash[..]).boot_state(Ota1).state;
            assert_eq!(state, Some(State::PendingVerify));
        }
    }
}

#[test]
fn fault_4_a_corrupt_target_sector_is_overwritten() {
    let mut garbage = [0; 32];
    Rng::new(0xF4).fill(&mut garbage);
    for target in [
        garbage,
        bad_crc(9, State::Valid),
        [0; 32],
        entry(9, State::Aborted),
    ] {
        let mut flash = otadata_of(&SLOT_COUNT_1_VALID, &target);
        let write = read(&flash[..]).plan_activate(Ota0).unwrap();
        assert_eq!(write.sector(), Sector::Second);
        apply(&mut flash, &write);
        assert_eq!(read(&flash[..]).selected(), Some(Ota1));
    }
}

// The fixture.

#[test]
fn the_first_boot_fixture_is_entry_0_valid_and_entry_1_erased() {
    assert_eq!(OTADATA_FIRST_BOOT.len(), OTADATA_LEN);
    assert_eq!(OTADATA_FIRST_BOOT[..32], SLOT_COUNT_1_VALID);
    assert!(OTADATA_FIRST_BOOT[32..].iter().all(|&b| b == 0xFF));

    let otadata = read(OTADATA_FIRST_BOOT);
    let first = otadata.entry(Sector::First).unwrap();
    assert_eq!(
        (first.seq, first.state, first.slot()),
        (1, State::Valid, Ota0)
    );
    assert_eq!(first.validity(), Validity::Valid);
    let second = otadata.entry(Sector::Second).unwrap();
    assert_eq!(second.validity(), Validity::Blank);
    assert_eq!(otadata.active(), Some(Sector::First));
    assert_eq!(
        otadata.boot_state(Ota0),
        BootState {
            running: Ota0,
            selected: Some(Ota0),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        }
    );
}

#[test]
fn the_simulated_bootloader_writes_the_fixture_on_blank_otadata() {
    // flash-all writes ota_0 only, so ota_1 doesn't load.
    for loadable in [[true, false], [true, true]] {
        let mut flash = vec![0xFF; OTADATA_LEN];
        assert_eq!(boot(&mut flash, loadable), Some(Ota0));
        assert_eq!(flash, OTADATA_FIRST_BOOT);
        // The next boot changes nothing.
        assert_eq!(boot(&mut flash, loadable), Some(Ota0));
        assert_eq!(flash, OTADATA_FIRST_BOOT);
    }
}

// Flows on the simulated bootloader.

/// Plans a write on `flash` with `plan` and executes it; the plan has to
/// give one.
fn run_plan(flash: &mut [u8], plan: impl FnOnce(&Otadata) -> Result<Option<Write>, PlanError>) {
    let write = plan(&read(&flash[..])).unwrap().unwrap();
    apply(flash, &write);
}

#[test]
fn an_update_is_booted_once_then_confirmed() {
    let mut flash = vec![0xFF; OTADATA_LEN];
    let mut loadable = [true, false];
    assert_eq!(boot(&mut flash, loadable), Some(Ota0));

    run_plan(&mut flash, |otadata| otadata.plan_begin_update(Ota0));
    assert_eq!(read(&flash[..]).plan_begin_update(Ota0), Ok(None));
    loadable[1] = true;
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(read(&flash[..]).boot_state(Ota0).state, Some(State::Valid));

    assert_eq!(boot(&mut flash, loadable), Some(Ota1));
    let state = read(&flash[..]).boot_state(Ota1);
    assert_eq!(state.state, Some(State::PendingVerify));
    assert!(state.rollback_target);

    run_plan(&mut flash, |otadata| otadata.plan_confirm(Ota1));
    assert_eq!(boot(&mut flash, loadable), Some(Ota1));
    let state = read(&flash[..]).boot_state(Ota1);
    assert_eq!(state.state, Some(State::Valid));
    assert!(state.rollback_target);
    assert_eq!(boot(&mut flash, loadable), Some(Ota1));
}

#[test]
fn an_update_that_isnt_confirmed_is_rolled_back_by_the_bootloader() {
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    run_plan(&mut flash, |otadata| otadata.plan_begin_update(Ota0));
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota1));
    // A reboot without confirming: the bootloader aborts it.
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
    let state = read(&flash[..]).boot_state(Ota0);
    assert_eq!(state.selected, Some(Ota0));
    assert_eq!(state.state, Some(State::Valid));
    assert!(!state.rollback_target);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
}

#[test]
fn an_update_the_bootloader_cant_load_falls_back_to_the_running_image() {
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, false]), Some(Ota0));
    // The pending entry still selects ota_1; the running image is ota_0's.
    let otadata = read(&flash[..]);
    let state = otadata.boot_state(Ota0);
    assert_eq!(state.selected, Some(Ota1));
    assert_eq!(state.state, Some(State::Valid));
    // A fallback: no update until it is confirmed, which selects ota_0.
    assert!(state.needs_confirmation);
    assert_eq!(otadata.plan_begin_update(Ota0), Err(PlanError::Unconfirmed));
    run_plan(&mut flash, |otadata| otadata.plan_confirm(Ota0));
    assert_eq!(read(&flash[..]).selected(), Some(Ota0));
    // A new update then takes ota_1 out of the selection.
    run_plan(&mut flash, |otadata| otadata.plan_begin_update(Ota0));
    assert_eq!(read(&flash[..]).plan_begin_update(Ota0), Ok(None));
    assert_eq!(boot(&mut flash, [true, false]), Some(Ota0));
}

#[test]
fn a_confirmed_update_can_be_rolled_back_once() {
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota1));
    run_plan(&mut flash, |otadata| otadata.plan_confirm(Ota1));

    let roll_back = read(&flash[..]).plan_roll_back(Ota1).unwrap();
    apply(&mut flash, &roll_back);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
    let otadata = read(&flash[..]);
    assert_eq!(otadata.entry(Sector::Second).unwrap().state, State::Invalid);
    let state = otadata.boot_state(Ota0);
    assert_eq!(state.state, Some(State::Valid));
    assert!(!state.rollback_target);
    assert_eq!(
        otadata.plan_roll_back(Ota0),
        Err(PlanError::NoRollbackTarget)
    );
}

#[test]
fn an_unconfirmed_update_can_be_rolled_back_before_the_reboot() {
    let mut flash = OTADATA_FIRST_BOOT.to_vec();
    let activation = read(&flash[..]).plan_activate(Ota0).unwrap();
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota1));
    let roll_back = read(&flash[..]).plan_roll_back(Ota1).unwrap();
    apply(&mut flash, &roll_back);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
}

#[test]
fn without_a_valid_entry_an_update_follows_a_confirmation() {
    // Running ota_1 with nothing valid: the next boot would try ota_0 first,
    // the slot about to be rewritten, so no update begins and nothing is
    // activated until the running image is confirmed, which selects it.
    let mut flash = otadata_of(&entry(3, State::Aborted), &bad_crc(4, State::Valid));
    let otadata = read(&flash[..]);
    assert_eq!(otadata.plan_activate(Ota1), Err(PlanError::NotSelected));
    assert_eq!(otadata.plan_begin_update(Ota1), Err(PlanError::Unconfirmed));
    let confirm = otadata.plan_confirm(Ota1).unwrap().unwrap();
    assert_eq!(confirm.entry(), SelectEntry::new(2, State::Valid));

    // Cut short, the confirmation leaves the bootloader trying ota_0 first,
    // as before it; ota_0 still holds the image it tried then.
    let mut cut = flash.clone();
    apply_cut(&mut cut, &confirm, Some(JUST_ERASED));
    assert_eq!(boot(&mut cut, [true, true]), Some(Ota0));
    let mut before = flash.clone();
    assert_eq!(boot(&mut before, [true, true]), Some(Ota0));

    apply(&mut flash, &confirm);
    run_plan(&mut flash, |otadata| otadata.plan_begin_update(Ota1));
    assert_eq!(read(&flash[..]).plan_begin_update(Ota1), Ok(None));
    // While ota_0 is rewritten, nothing selects it, cut or not.
    let activation = read(&flash[..]).plan_activate(Ota1).unwrap();
    for cut in every_cut(&activation) {
        let mut torn = flash.clone();
        apply_cut(&mut torn, &activation, Some(cut));
        assert_eq!(boot(&mut torn, [true, true]), Some(Ota1), "{cut:?}");
    }
    apply(&mut flash, &activation);
    assert_eq!(boot(&mut flash, [true, true]), Some(Ota0));
    assert_eq!(
        read(&flash[..]).boot_state(Ota0).state,
        Some(State::PendingVerify)
    );
}

// Power cuts.

/// Boots a copy of `flash` twice, as a board that never confirms what it
/// starts: the slots started, and the otadata after the first boot.
fn boot_twice(flash: &[u8], loadable: [bool; 2]) -> ([Option<Slot>; 2], Vec<u8>) {
    let mut flash = flash.to_vec();
    let first = boot(&mut flash, loadable);
    let after_first = flash.clone();
    let second = boot(&mut flash, loadable);
    ([first, second], after_first)
}

/// What a cut write boots.
struct CutBoots {
    /// The cut, `None` for the whole write.
    cut: Option<Cut>,
    /// Whether it landed the whole entry.
    whole: bool,
    /// The slots the two boots start.
    boots: [Option<Slot>; 2],
    /// The otadata the first boot leaves.
    left: Vec<u8>,
}

/// The cut that leaves the written sector erased.
const JUST_ERASED: Cut = Cut {
    done: 0,
    tear: Tear::Prefix(0),
};

/// Cuts `write` on `flash` in each way [`every_cut`] gives, and boots each
/// result twice, and the whole write too. Each has to boot what `flash`
/// would have, or what `flash` with the write would. A write over the active
/// entry may also boot what its erased sector leaves, which is the other
/// entry's slot when that is valid and loads: a rollback. Returns what each
/// cut and the whole write boot.
fn cuts(flash: &[u8], write: &Write, loadable: [bool; 2], case: &str) -> Vec<CutBoots> {
    let in_place = read(flash).active() == Some(write.sector());
    let (before, _) = boot_twice(flash, loadable);
    let mut done = flash.to_vec();
    apply(&mut done, write);
    let (after, _) = boot_twice(&done, loadable);
    let mut erased = flash.to_vec();
    apply_cut(&mut erased, write, Some(JUST_ERASED));
    let (erased_boots, _) = boot_twice(&erased, loadable);

    if in_place {
        let kept = read(flash).valid(write.sector().other());
        if let Some(kept) = kept.filter(|kept| loadable[kept.slot().number() as usize]) {
            assert_eq!(erased_boots[0], Some(kept.slot()), "{case}: rollback");
        }
    }

    every_cut(write)
        .into_iter()
        .map(Some)
        .chain([None])
        .map(|cut| {
            let mut torn = flash.to_vec();
            apply_cut(&mut torn, write, cut);
            let whole = torn == done;
            let (boots, left) = boot_twice(&torn, loadable);
            assert!(boots.iter().all(Option::is_some), "{case}, {cut:?}");
            let allowed = boots == before || boots == after || (in_place && boots == erased_boots);
            assert!(
                allowed,
                "{case}, {cut:?}: booted {boots:?}, before {before:?}, \
                 after {after:?}, erased {erased_boots:?}"
            );
            CutBoots {
                cut,
                whole,
                boots,
                left,
            }
        })
        .collect()
}

/// The states the planners start from on a board: for every pair of entries
/// and every set of loadable slots, the state a boot leaves, with the slot
/// it started, and the states one completed plan later, before the next
/// boot. After an activation the other slot holds a new image, which loads
/// or not.
fn board_states() -> Vec<(String, Vec<u8>, Slot, [bool; 2])> {
    let kinds = kinds();
    let mut states = Vec::new();
    for (first_name, first) in &kinds {
        for (second_name, second) in &kinds {
            for loadable in LOADABLE {
                let mut flash = otadata_of(first, second);
                let running = boot(&mut flash, loadable).unwrap();
                let case = format!("{first_name} / {second_name}, {loadable:?}, {running:?}");
                let otadata = read(&flash[..]);
                let mut then = |plan: &str, write: Write, loadable: [bool; 2]| {
                    let mut flash = flash.clone();
                    apply(&mut flash, &write);
                    states.push((format!("{case}, then {plan}"), flash, running, loadable));
                };
                if let Ok(Some(write)) = otadata.plan_begin_update(running) {
                    then("begin update", write, loadable);
                }
                if let Ok(write) = otadata.plan_activate(running) {
                    for new_image in [true, false] {
                        let mut loadable = loadable;
                        loadable[running.other().number() as usize] = new_image;
                        then("activate", write, loadable);
                    }
                }
                if let Ok(Some(write)) = otadata.plan_confirm(running) {
                    then("confirm", write, loadable);
                }
                if let Ok(write) = otadata.plan_roll_back(running) {
                    then("roll back", write, loadable);
                }
                states.push((case, flash, running, loadable));
            }
        }
    }
    states
}

#[test]
fn a_cut_begin_update_boots_what_it_would_have() {
    let mut planned = 0;
    for (case, flash, running, loadable) in board_states() {
        let mut flash = flash;
        let mut writes = 0;
        loop {
            let otadata = read(&flash[..]);
            let write = match otadata.plan_begin_update(running) {
                Ok(Some(write)) => write,
                Ok(None) => break,
                Err(PlanError::Unconfirmed) => {
                    assert!(otadata.boot_state(running).needs_confirmation, "{case}");
                    break;
                }
                Err(PlanError::SeqExhausted) => {
                    assert!(
                        otadata.active_entry().unwrap().seq >= BLANK_SEQ - 2,
                        "{case}"
                    );
                    break;
                }
                Err(error) => panic!("{case}: {error:?}"),
            };
            // Only from a confirmed image, which stays the only Valid one.
            assert!(!otadata.boot_state(running).needs_confirmation, "{case}");
            if Some(write.sector()) == otadata.active() {
                // Cancelling the active entry, which names the other slot.
                assert_eq!(write.entry().state, State::Invalid, "{case}");
                assert_eq!(write.entry().slot(), running.other(), "{case}");
            } else {
                assert_eq!(write.entry().state, State::Valid, "{case}");
                assert_eq!(write.entry().slot(), running, "{case}");
            }
            cuts(&flash, &write, loadable, &case);
            apply(&mut flash, &write);
            writes += 1;
            planned += 1;
            assert!(writes <= 2, "{case}");
        }
        let otadata = read(&flash[..]);
        if otadata.plan_begin_update(running) == Ok(None) {
            // Settled: both entries valid and naming the running slot.
            for sector in Sector::ALL {
                assert_eq!(
                    otadata.valid(sector).map(|e| e.slot()),
                    Some(running),
                    "{case}"
                );
            }
        }
    }
    assert!(planned > 1000, "{planned}");
}

#[test]
fn a_cut_activation_never_boots_the_new_image_without_rollback() {
    let mut planned = 0;
    for (case, flash, running, loadable) in board_states() {
        let Ok(write) = read(&flash[..]).plan_activate(running) else {
            continue;
        };
        planned += 1;
        assert_eq!(write.entry().state, State::New);
        assert_eq!(write.entry().slot(), running.other());
        // The image written to the other slot loads, or the bootloader
        // rejects it.
        for new_image in [true, false] {
            let mut loadable = loadable;
            loadable[running.other().number() as usize] = new_image;
            let boots = cuts(&flash, &write, loadable, &case);
            for CutBoots {
                cut,
                whole,
                boots: [booted, _],
                left,
            } in boots
            {
                if booted == Some(running.other()) {
                    // Only the whole entry starts it, and only once.
                    assert!(whole, "{case}, {cut:?}");
                    let state = read(&left[..]).boot_state(running.other()).state;
                    assert_eq!(state, Some(State::PendingVerify), "{case}");
                } else {
                    assert_eq!(booted, Some(running), "{case}, {cut:?}");
                }
            }
        }
    }
    assert!(planned > 500, "{planned}");
}

#[test]
fn a_cut_confirmation_boots_as_before_or_as_confirmed() {
    let (mut in_place, mut beside) = (0, 0);
    for (case, flash, running, loadable) in board_states() {
        let otadata = read(&flash[..]);
        let Ok(Some(write)) = otadata.plan_confirm(running) else {
            assert!(
                !otadata.boot_state(running).needs_confirmation
                    || otadata.plan_confirm(running).is_err(),
                "{case}"
            );
            continue;
        };
        let (before, _) = boot_twice(&flash, loadable);
        let boots = cuts(&flash, &write, loadable, &case);
        if Some(write.sector()) == otadata.active() {
            in_place += 1;
        } else {
            // Beside the active entry: every cut boots as before.
            beside += 1;
            for CutBoots { cut, boots, .. } in boots {
                if cut.is_some() {
                    assert_eq!(boots, before, "{case}, {cut:?}");
                }
            }
        }
        let mut done = flash.clone();
        apply(&mut done, &write);
        let state = read(&done[..]).boot_state(running);
        assert_eq!(state.selected, Some(running), "{case}");
        assert!(!state.needs_confirmation, "{case}");
    }
    assert!(in_place > 300 && beside > 300, "{in_place} {beside}");
}

#[test]
fn a_cut_roll_back_boots_the_target_or_the_running_image() {
    let mut planned = 0;
    for (case, flash, running, loadable) in board_states() {
        // Rolling back needs an image in the other slot.
        if !loadable[running.other().number() as usize] {
            continue;
        }
        let Ok(write) = read(&flash[..]).plan_roll_back(running) else {
            continue;
        };
        planned += 1;
        // The erase alone already leaves the target selected, whatever the
        // sequence number.
        for CutBoots {
            cut,
            boots: [booted, _],
            ..
        } in cuts(&flash, &write, loadable, &case)
        {
            assert_eq!(booted, Some(running.other()), "{case}, {cut:?}");
        }
    }
    assert!(planned > 100, "{planned}");
}

#[test]
fn an_entry_cut_anywhere_is_valid_only_whole() {
    let mut rng = Rng::new(0xC07);
    let seqs = [0, 1, 2, 3, 0x8000_0000, BLANK_SEQ - 1]
        .into_iter()
        .chain((0..60).map(|_| rng.next_u32()));
    let mut cuts = 0;
    for seq in seqs.filter(|&seq| seq != BLANK_SEQ) {
        for state in [
            State::New,
            State::Valid,
            State::Invalid,
            State::PendingVerify,
        ] {
            let write = Write::new(Sector::First, SelectEntry::new(seq, state));
            for cut in every_cut(&write) {
                let mut flash = vec![0xFF; OTADATA_LEN];
                apply_cut(&mut flash, &write, Some(cut));
                let entry = read(&flash[..]).entry(Sector::First).unwrap();
                // A cut can land all of a program whose last bytes are 0xFF.
                assert!(
                    !entry.is_valid() || entry == write.entry(),
                    "{seq:#x} {state:?} {cut:?}: {entry:?}"
                );
                cuts += 1;
            }
        }
    }
    assert!(cuts > 20_000, "{cuts}");
}

#[test]
fn a_torn_sequence_number_never_matches_the_crc_stored() {
    // Every sequence number with one bit fewer cleared than planned, the
    // tear closest to whole, for a sample of planned ones.
    let mut rng = Rng::new(0x5E9);
    for seq in [0, 1, 2, 4, 0xFFFF_FFFE]
        .into_iter()
        .chain((0..2000).map(|_| rng.next_u32()))
    {
        for bit in (0..32).filter(|bit| seq & (1 << bit) == 0) {
            assert_ne!(crc(seq | 1 << bit), crc(seq), "{seq:#x} bit {bit}");
        }
    }
}

#[test]
fn the_crc_is_one_to_one() {
    // `!crc` is linear over GF(2): the CRC is one to one when the images of
    // the 32 unit vectors are independent.
    assert_eq!(!crc(0), 0);
    let mut rows: Vec<u32> = (0..32).map(|bit| !crc(1 << bit)).collect();
    for (a, b) in [(0x1234_5678_u32, 0x0F0F_00FF_u32), (u32::MAX, 7)] {
        assert_eq!(!crc(a ^ b), !crc(a) ^ !crc(b));
    }
    let mut rank = 0;
    for bit in (0..32).rev() {
        let Some(pivot) = (rank..32).find(|&row| rows[row] & (1 << bit) != 0) else {
            continue;
        };
        rows.swap(rank, pivot);
        let pivot = rows[rank];
        for row in rows.iter_mut().skip(rank + 1) {
            if *row & (1 << bit) != 0 {
                *row ^= pivot;
            }
        }
        rank += 1;
    }
    assert_eq!(rank, 32);
}

#[test]
fn one_program_of_the_whole_entry_can_tear_into_an_activation_without_rollback() {
    // The activation of ota_1 after the first boot.
    let start = OTADATA_FIRST_BOOT.to_vec();
    let write = read(&start[..]).plan_activate(Ota0).unwrap();
    let at = write.offset() as usize;
    let bytes = write.bytes();
    let clears: usize = bytes.iter().map(|b| b.count_zeros() as usize).sum();

    // Programmed in one go, as ESP-IDF does, every bit landing but one.
    let mut unarmed = Vec::new();
    for kept in 0..clears {
        let mut flash = start.clone();
        flash[at..at + SECTOR_SIZE as usize].fill(0xFF);
        Tear::AllBut(kept).land(&mut flash[at..at + ENTRY_LEN], &bytes);
        if boot(&mut flash, [true, true]) == Some(Ota1) {
            let state = read(&flash[..]).boot_state(Ota1).state;
            if state != Some(State::PendingVerify) {
                unarmed.push(state);
            }
        }
    }
    // The state's bits but one: Valid, or an unknown state, which ESP-IDF
    // boots without ever arming a rollback.
    assert!(unarmed.contains(&Some(State::Valid)), "{unarmed:?}");
    assert!(unarmed.len() > 20, "{unarmed:?}");

    // In its three programs, no cut starts it.
    for cut in every_cut(&write) {
        let mut flash = start.clone();
        apply_cut(&mut flash, &write, Some(cut));
        assert_eq!(boot(&mut flash, [true, true]), Some(Ota0), "{cut:?}");
    }
}

#[test]
fn the_tears_land_part_of_a_program() {
    let new = [0x00, 0x0F, 0xFF, 0x12];
    let clears: u32 = new.iter().map(|b: &u8| b.count_zeros()).sum();
    let landed = |tear: Tear| {
        let mut old = [0xFF; 4];
        tear.land(&mut old, &new);
        // Only bits the program clears, and never all of them.
        for (old, new) in old.iter().zip(&new) {
            assert_eq!(old & new, *new, "{tear:?}");
        }
        old.iter().map(|b| b.count_zeros()).sum::<u32>()
    };
    assert_eq!(landed(Tear::Prefix(0)), 0);
    assert_eq!(landed(Tear::Prefix(2)), 12);
    for n in 0..40 {
        assert_eq!(landed(Tear::AllBut(n)), clears - 1);
    }
    let mut counts = Vec::new();
    for seed in 0..60 {
        let count = landed(Tear::Bits(seed));
        assert!(count < clears, "{seed}");
        counts.push(count);
    }
    assert!(counts.iter().any(|&count| count < clears / 2));
    assert!(counts.iter().any(|&count| count == clears - 1));
    // Programming over cleared bits lands nothing more.
    let mut old = [0x00; 4];
    Tear::Bits(1).land(&mut old, &new);
    assert_eq!(old, [0; 4]);
}

// A long run of random operations.

/// A board running the simulated bootloader, with the operations of an OTA
/// service on top of the plans.
struct Board {
    flash: Vec<u8>,
    loadable: [bool; 2],
    /// Whether each slot's image was confirmed: the image `flash-all` wrote,
    /// or one confirmed through `plan_confirm`, and nothing else.
    confirmed: [bool; 2],
    running: Slot,
    /// The other slot is partly written.
    updating: bool,
    /// The other slot holds a finished image not yet activated.
    finished: bool,
    rng: Rng,
    stats: Stats,
    log: Vec<String>,
}

#[derive(Debug, Default)]
struct Stats {
    boots: usize,
    cuts: usize,
    updates_booted: usize,
    confirmed: usize,
    rolled_back: usize,
    aborted: usize,
    fallbacks: usize,
    refused: usize,
}

impl Board {
    fn new(seed: u64) -> Self {
        let mut board = Self {
            flash: vec![0xFF; OTADATA_LEN],
            loadable: [true, false],
            confirmed: [true, false],
            running: Ota0,
            updating: false,
            finished: false,
            rng: Rng::new(seed),
            stats: Stats::default(),
            log: Vec::new(),
        };
        board.reboot();
        assert_eq!(board.flash, OTADATA_FIRST_BOOT);
        board
    }

    fn otadata(&self) -> Otadata {
        read(&self.flash[..])
    }

    fn other(&self) -> usize {
        self.running.other().number() as usize
    }

    /// Makes `write`, or cuts it short and reboots. Whether it completed.
    fn execute(&mut self, write: &Write) -> bool {
        self.log
            .push(format!("  write {:?} {:?}", write.sector(), write.entry()));
        if self.rng.below(5) == 0 {
            let cut = random_cut(&mut self.rng, write);
            self.log.push(format!("  cut {cut:?}"));
            apply_cut(&mut self.flash, write, Some(cut));
            self.stats.cuts += 1;
            self.reboot();
            false
        } else {
            apply(&mut self.flash, write);
            true
        }
    }

    fn reboot(&mut self) {
        let before = self.running;
        let pending = self.otadata().boot_state(before).state == Some(State::PendingVerify);
        let running = boot(&mut self.flash, self.loadable).expect("no slot boots");
        self.stats.boots += 1;
        self.log.push(format!(
            "  boot {running:?} {:?} loadable {:?} confirmed {:?}",
            self.otadata().entries.map(|e| e.map(|e| (e.seq, e.state))),
            self.loadable,
            self.confirmed
        ));
        if pending && running != before {
            self.stats.aborted += 1;
        }
        self.running = running;
        self.updating = false;
        self.finished = false;

        let state = self.otadata().boot_state(running);
        let log = self.log[self.log.len().saturating_sub(40)..].join("\n");
        assert_ne!(state.state, Some(State::New));
        match state.selected {
            Some(selected) if selected != running => {
                // A fallback, only from a slot that doesn't load.
                assert!(!self.loadable[selected.number() as usize], "{log}");
                assert!(state.needs_confirmation, "{log}");
                self.stats.fallbacks += 1;
            }
            _ if !self.confirmed[running.number() as usize] => {
                // An image nobody confirmed runs only with its rollback
                // armed.
                assert_eq!(state.state, Some(State::PendingVerify), "{state:?}\n{log}");
                self.stats.updates_booted += 1;
            }
            _ => {}
        }
    }

    fn begin_update(&mut self) {
        loop {
            match self.otadata().plan_begin_update(self.running) {
                Ok(Some(write)) => {
                    if !self.execute(&write) {
                        return;
                    }
                }
                Ok(None) => break,
                Err(PlanError::Unconfirmed) => {
                    assert!(self.otadata().boot_state(self.running).needs_confirmation);
                    self.stats.refused += 1;
                    return;
                }
                Err(error) => panic!("{error:?}"),
            }
        }
        // The first sector of the slot is erased: it no longer loads.
        let other = self.other();
        self.loadable[other] = false;
        self.confirmed[other] = false;
        self.updating = true;
        self.finished = false;
    }

    fn finish(&mut self) {
        if self.updating {
            let other = self.other();
            // Now and then, an image the bootloader rejects.
            self.loadable[other] = self.rng.below(8) != 0;
            self.updating = false;
            self.finished = true;
        }
    }

    fn activate(&mut self) {
        if !self.finished || !self.loadable[self.other()] {
            return;
        }
        match self.otadata().plan_activate(self.running) {
            Ok(write) => {
                if self.execute(&write) {
                    self.finished = false;
                }
            }
            Err(error) => panic!("{error:?}"),
        }
    }

    fn confirm(&mut self) {
        let state = self.otadata().boot_state(self.running);
        match self.otadata().plan_confirm(self.running) {
            Ok(Some(write)) => {
                assert!(state.needs_confirmation);
                if self.execute(&write) {
                    self.confirmed[self.running.number() as usize] = true;
                    self.stats.confirmed += 1;
                    let state = self.otadata().boot_state(self.running);
                    assert_eq!(state.selected, Some(self.running));
                    assert_eq!(state.state, Some(State::Valid));
                }
            }
            Ok(None) => assert!(!state.needs_confirmation),
            Err(error) => panic!("{error:?}"),
        }
    }

    /// The other slot's image decays, now and then: it no longer loads.
    fn decay(&mut self) {
        let other = self.other();
        if self.loadable[other] && self.rng.below(4) == 0 {
            self.log.push("  decay".to_owned());
            self.loadable[other] = false;
            self.finished = false;
        }
    }

    fn roll_back(&mut self) {
        // The OTA service rolls back only to an image that verifies.
        if self.updating || !self.loadable[self.other()] {
            return;
        }
        let state = self.otadata().boot_state(self.running);
        match self.otadata().plan_roll_back(self.running) {
            Ok(write) => {
                if self.execute(&write) {
                    self.reboot();
                    self.stats.rolled_back += 1;
                }
            }
            Err(PlanError::NoRollbackTarget) => assert!(!state.rollback_target),
            Err(PlanError::NotSelected) => assert_ne!(state.selected, Some(self.running)),
            Err(error) => panic!("{error:?}"),
        }
    }

    fn step(&mut self) {
        let op = self.rng.below(12);
        self.log.push(format!("op {op} running {:?}", self.running));
        match op {
            0 | 1 => {
                self.begin_update();
                self.finish();
                self.activate();
            }
            2 => self.begin_update(),
            3 => {
                self.begin_update();
                self.finish();
            }
            4 => self.activate(),
            5 | 6 => self.confirm(),
            7 => self.roll_back(),
            8 => self.decay(),
            _ => self.reboot(),
        }
        // Only a confirmation makes an image confirmed.
        let state = self.otadata().boot_state(self.running);
        if !state.needs_confirmation {
            let log = self.log[self.log.len().saturating_sub(40)..].join("\n");
            assert!(self.confirmed[self.running.number() as usize], "{log}");
        }
        // Whatever happened, the board boots.
        let mut copy = self.flash.clone();
        assert!(boot(&mut copy, self.loadable).is_some());
    }
}

#[test]
fn random_operations_and_power_cuts_keep_the_board_booting() {
    // Odd seeds: the generator sets the lowest bit.
    for seed in (1..40).step_by(2) {
        let mut board = Board::new(seed);
        for _ in 0..5000 {
            board.step();
        }
        let stats = &board.stats;
        assert!(stats.cuts > 100, "{seed}: {stats:?}");
        assert!(stats.updates_booted > 20, "{seed}: {stats:?}");
        assert!(stats.confirmed > 5, "{seed}: {stats:?}");
        assert!(stats.rolled_back > 5, "{seed}: {stats:?}");
        assert!(stats.aborted > 5, "{seed}: {stats:?}");
        assert!(stats.fallbacks > 5, "{seed}: {stats:?}");
        assert!(stats.refused > 5, "{seed}: {stats:?}");
    }
}
