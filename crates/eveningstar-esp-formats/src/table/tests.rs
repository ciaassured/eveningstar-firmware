#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and indices are small"
)]

use super::*;
use crate::test_support::{
    ESPFLASH_TABLE, PARTITIONS, Rng, TABLE, TABLE_OFFSET, entry_bytes, flash_with, md5_entry,
    md5_entry_for, table_of, table_with_md5,
};
use crate::{FLAG_READONLY, TYPE_DATA};

const N: usize = PARTITIONS.len();

/// The chip size the Nix bootloader's header gives: 8 MiB.
const CHIP: u32 = 0x80_0000;

/// The table's entries, which have to be well formed.
fn read_all(source: &[u8], offset: u32) -> Vec<RawEntry> {
    entries(source, offset).map(Result::unwrap).collect()
}

/// A comparison of `table` with the layout that matches except at `index`.
fn only_differs(table: &[u8], index: usize, fields: Fields) {
    let comparison = compare(table, 0, &PARTITIONS);
    let mut expected = [Match::Matches; N];
    expected[index] = Match::Differs(fields);
    assert_eq!(comparison.partitions, expected, "entry {index}, {fields:?}");
    assert_eq!(comparison.extra, 0);
    assert!(matches!(comparison.end, Ok(End::Md5 { index: N, .. })));
    assert!(!comparison.matches());
}

// The fixtures.

#[test]
fn the_nix_table_holds_the_layout() {
    let entries = read_all(TABLE, 0);
    assert_eq!(entries.len(), N);
    for (entry, expected) in entries.iter().zip(PARTITIONS) {
        assert!(
            entry.differences(&expected).is_empty(),
            "{}",
            expected.label
        );
        assert_eq!(entry.label.to_str(), Some(expected.label));
    }
}

#[test]
fn the_nix_table_matches_the_layout() {
    let comparison = compare(TABLE, 0, &PARTITIONS);
    assert_eq!(comparison.partitions, [Match::Matches; N]);
    assert_eq!(comparison.extra, 0);
    let Ok(End::Md5 {
        index,
        digest,
        matches,
    }) = comparison.end
    else {
        panic!("{:?}", comparison.end);
    };
    assert_eq!(index, N);
    assert_eq!(&digest[..], &TABLE[N * 32 + 16..N * 32 + 32]);
    assert!(matches, "ESP-IDF's MD5 checks");
    assert!(comparison.matches());
}

#[test]
fn the_espflash_table_is_read_as_stored() {
    let entries = read_all(ESPFLASH_TABLE, 0);
    let summary: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.label.to_str().unwrap(),
                e.kind,
                e.subtype,
                e.offset,
                e.size,
                e.flags,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("nvs", 0x01, 0x02, 0x9000, 0x6000, 0),
            ("phy_init", 0x01, 0x01, 0xF000, 0x1000, 0),
            ("factory", 0x00, 0x00, 0x1_0000, 0x7F_0000, 0),
        ]
    );
}

#[test]
fn the_espflash_table_differs_from_the_layout() {
    let comparison = compare(ESPFLASH_TABLE, 0, &PARTITIONS);
    // Written out by hand from the bytes: nvs against factory, phy_init
    // against otadata, the factory app against ota_0.
    assert_eq!(
        comparison.partitions,
        [
            Match::Differs(Fields::LABEL | Fields::SUBTYPE | Fields::SIZE | Fields::FLAGS),
            Match::Differs(Fields::LABEL | Fields::SUBTYPE | Fields::OFFSET | Fields::SIZE),
            Match::Differs(Fields::LABEL | Fields::SUBTYPE | Fields::SIZE),
            Match::Missing,
            Match::Missing,
            Match::Missing,
            Match::Missing,
            Match::Missing,
        ]
    );
    assert_eq!(comparison.extra, 0);
    assert!(matches!(
        comparison.end,
        Ok(End::Md5 {
            index: 3,
            matches: true,
            ..
        })
    ));
    assert!(!comparison.matches());
}

// Each field of each entry.

#[test]
fn each_changed_field_is_reported_as_that_field() {
    // The field, where it starts in the entry, and a change to it.
    type Change = (Fields, usize, fn(&mut [u8]));
    let changes: [Change; 6] = [
        (Fields::KIND, 2, |b| b[0] ^= 0x01),
        (Fields::SUBTYPE, 3, |b| b[0] ^= 0x80),
        (Fields::OFFSET, 4, |b| b[1] ^= 0x10),
        (Fields::SIZE, 8, |b| b[3] ^= 0x01),
        (Fields::LABEL, 12, |b| b[0] ^= 0x20),
        (Fields::FLAGS, 28, |b| b[0] ^= 0x01),
    ];
    for index in 0..N {
        for (field, at, change) in changes {
            let mut table = TABLE.to_vec();
            change(&mut table[index * 32 + at..]);
            only_differs(&table, index, field);
        }
    }
}

#[test]
fn every_bit_of_every_field_counts() {
    let field_of = |byte: usize| match byte {
        2 => Fields::KIND,
        3 => Fields::SUBTYPE,
        4..8 => Fields::OFFSET,
        8..12 => Fields::SIZE,
        12..28 => Fields::LABEL,
        _ => Fields::FLAGS,
    };
    for index in 0..N {
        for byte in 2..32 {
            for bit in 0..8 {
                let mut table = TABLE.to_vec();
                table[index * 32 + byte] ^= 1 << bit;
                only_differs(&table, index, field_of(byte));
            }
        }
    }
}

#[test]
fn several_fields_are_reported_together() {
    let mut table = TABLE.to_vec();
    let mut entry = PARTITIONS[4];
    entry.label = "config_x";
    entry.size = 0x1000;
    entry.flags = FLAG_READONLY;
    table[4 * 32..5 * 32].copy_from_slice(&entry_bytes(&entry));
    only_differs(&table, 4, Fields::LABEL | Fields::SIZE | Fields::FLAGS);

    let mut table = TABLE.to_vec();
    table[2 * 32 + 2..3 * 32].fill(0x5A);
    only_differs(&table, 2, Fields::ALL);
}

#[test]
fn a_label_with_bytes_after_its_nul_differs() {
    let mut table = TABLE.to_vec();
    // "ota_0", NUL, then a stray byte.
    table[2 * 32 + 12 + 10] = b'x';
    only_differs(&table, 2, Fields::LABEL);
    let entry = read_all(&table, 0)[2];
    assert_eq!(entry.label.to_str(), Some("ota_0"));
}

// Missing, extra and reordered entries.

#[test]
fn a_missing_last_entry_is_missing() {
    let table = table_with_md5(&PARTITIONS[..N - 1]);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    let mut expected = [Match::Matches; N];
    expected[N - 1] = Match::Missing;
    assert_eq!(comparison.partitions, expected);
    assert_eq!(comparison.extra, 0);
    assert!(matches!(comparison.end, Ok(End::Md5 { index: 7, .. })));
    assert!(!comparison.matches());
}

#[test]
fn a_missing_entry_shifts_the_ones_after_it() {
    // Without otadata, every later entry is compared with the partition
    // before it in the layout.
    let mut entries = PARTITIONS.to_vec();
    entries.remove(1);
    let table = table_with_md5(&entries);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert_eq!(comparison.partitions[0], Match::Matches);
    for (index, result) in comparison.partitions.iter().enumerate().take(N - 1).skip(1) {
        let fields = entry_of(&entries[index]).differences(&PARTITIONS[index]);
        assert!(fields.contains(Fields::LABEL | Fields::OFFSET), "{index}");
        assert_eq!(*result, Match::Differs(fields));
    }
    assert_eq!(comparison.partitions[N - 1], Match::Missing);
    assert_eq!(comparison.extra, 0);
}

#[test]
fn extra_entries_are_counted() {
    let extra = Entry {
        label: "spare",
        kind: TYPE_DATA,
        subtype: 0x06,
        offset: 0x7F_0000,
        size: 0x1_0000,
        flags: 0,
    };
    let mut entries = PARTITIONS.to_vec();
    entries.push(extra);
    entries.push(extra);
    let table = table_with_md5(&entries);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert_eq!(comparison.partitions, [Match::Matches; N]);
    assert_eq!(comparison.extra, 2);
    assert!(matches!(comparison.end, Ok(End::Md5 { index: 10, .. })));
    assert!(!comparison.matches());
    // The first extra entry is entry N.
    let first = super::entries(&table[..], 0).nth(N).unwrap().unwrap();
    assert!(first.label.is_padded("spare"));
}

#[test]
fn an_extra_entry_in_the_middle_shifts_the_ones_after_it() {
    let mut entries = PARTITIONS.to_vec();
    entries.insert(0, PARTITIONS[1]);
    let table = table_with_md5(&entries);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert!(
        comparison
            .partitions
            .iter()
            .all(|m| matches!(m, Match::Differs(_)))
    );
    assert_eq!(comparison.extra, 1);
}

#[test]
fn swapped_entries_differ_at_both_positions() {
    let mut entries = PARTITIONS.to_vec();
    entries.swap(2, 3);
    let table = table_with_md5(&entries);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    let mut expected = [Match::Matches; N];
    expected[2] = Match::Differs(Fields::LABEL | Fields::SUBTYPE | Fields::OFFSET);
    expected[3] = expected[2];
    assert_eq!(comparison.partitions, expected);
    assert_eq!(comparison.extra, 0);
}

// The end of the list, and what follows it.

#[test]
fn bytes_after_the_md5_entry_are_ignored() {
    // Past the MD5 entry (0x100..0x120): the entry after it, then the rest.
    for at in [0x120, 0x123, 0x124, 0x13F, 0x200, 0xBFF] {
        for bit in [0x01, 0x80] {
            let mut table = TABLE.to_vec();
            table[at] ^= bit;
            assert!(compare(&table[..], 0, &PARTITIONS).matches(), "{at:#x}");
        }
    }
    let mut table = TABLE.to_vec();
    table[0x120..].fill(0);
    assert!(compare(&table[..], 0, &PARTITIONS).matches());
}

/// Whether the table ends at an MD5 entry whose digest matches.
fn md5_matches(table: &[u8]) -> bool {
    match entries(table, 0).finish() {
        Ok(End::Md5 { matches, .. }) => matches,
        end => panic!("{end:?}"),
    }
}

#[test]
fn the_md5_padding_is_not_checked() {
    let mut table = TABLE.to_vec();
    table[0x102] = 0;
    table[0x10F] = 0;
    assert!(compare(&table[..], 0, &PARTITIONS).matches());
    assert!(md5_matches(&table));
}

#[test]
fn a_wrong_md5_is_reported_and_the_table_still_matches() {
    for at in [0x110, 0x11F] {
        let mut table = TABLE.to_vec();
        table[at] ^= 0x01;
        let comparison = compare(&table[..], 0, &PARTITIONS);
        assert!(comparison.matches(), "{at:#x}");
        let Ok(End::Md5 {
            digest, matches, ..
        }) = comparison.end
        else {
            panic!("{:?}", comparison.end);
        };
        assert!(!matches, "{at:#x}");
        assert_eq!(&digest[..], &table[0x110..0x120]);
    }
}

#[test]
fn the_md5_covers_every_byte_of_every_entry() {
    // Type, label, a label byte past its NUL that no field shows, flags, the
    // next entry's offset, and the last entry's last byte.
    for at in [0x02, 0x0C, 0x14, 0x1F, 0x24, 0xFF] {
        let mut table = TABLE.to_vec();
        table[at] ^= 0x01;
        assert!(!md5_matches(&table), "{at:#x}");
    }
}

#[test]
fn an_md5_entry_of_the_entries_before_it_matches() {
    for count in [0, 1, 2, 3, N, 40, MAX_ENTRIES - 1] {
        let mut slots: Vec<[u8; 32]> = (0..count)
            .map(|i| {
                entry_bytes(&Entry {
                    offset: 0x1_0000 * (i as u32 + 1),
                    ..PARTITIONS[i % N]
                })
            })
            .collect();
        slots.push(md5_entry_for(&slots));
        assert!(md5_matches(&table_of(&slots)), "{count}");
    }
    // The MD5 of nothing, for a table of only an MD5 entry.
    let empty = md5_entry_for(&[]);
    assert_eq!(
        empty[16..],
        [
            0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
            0x42, 0x7e
        ]
    );
    assert!(md5_matches(&table_of(&[empty])));
    assert!(!md5_matches(&table_of(&[md5_entry()])));
}

#[test]
fn a_table_ending_in_erased_bytes_matches() {
    let table = table_of(&PARTITIONS.map(|e| entry_bytes(&e)));
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert!(comparison.matches());
    assert_eq!(comparison.end, Ok(End::Erased { index: N }));
    assert_eq!(comparison.end.unwrap().partitions(), N);
}

#[test]
fn an_erased_magic_needs_an_erased_type_and_subtype() {
    let mut table = TABLE.to_vec();
    table[0x100..].fill(0xFF);
    table[0x102] = 0x01;
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert_eq!(
        comparison.end,
        Err(TableError::Magic {
            index: N,
            magic: 0xFFFF
        })
    );
    assert_eq!(comparison.partitions, [Match::Matches; N]);
    assert!(!comparison.matches());

    let mut table = TABLE.to_vec();
    table[0x100..].fill(0xFF);
    table[0x103] = 0x00;
    assert_eq!(
        compare(&table[..], 0, &PARTITIONS).end,
        Err(TableError::Magic {
            index: N,
            magic: 0xFFFF
        })
    );
}

#[test]
fn an_erased_table_is_empty() {
    let erased = [0xFF; MAX_LEN as usize];
    let comparison = compare(&erased[..], 0, &PARTITIONS);
    assert_eq!(comparison.end, Err(TableError::Empty));
    assert_eq!(comparison.partitions, [Match::Missing; N]);
    assert!(!comparison.matches());
    assert_eq!(entries(&erased[..], 0).next(), Some(Err(TableError::Empty)));
}

#[test]
fn a_table_of_only_an_md5_entry_has_no_partitions() {
    let table = table_of(&[md5_entry()]);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert!(matches!(comparison.end, Ok(End::Md5 { index: 0, .. })));
    assert_eq!(comparison.partitions, [Match::Missing; N]);
}

#[test]
fn a_bad_magic_ends_the_table_as_malformed() {
    for index in 0..=N {
        for magic in [0x0000_u16, 0x50AB, 0xAA50, 0xEBEA, 0x1234] {
            let mut table = TABLE.to_vec();
            table[index * 32..index * 32 + 2].copy_from_slice(&magic.to_le_bytes());
            let comparison = compare(&table[..], 0, &PARTITIONS);
            assert_eq!(comparison.end, Err(TableError::Magic { index, magic }));
            for (i, result) in comparison.partitions.iter().enumerate() {
                let expected = if i < index {
                    Match::Matches
                } else {
                    Match::Missing
                };
                assert_eq!(*result, expected, "{index} {magic:#x}");
            }
            assert!(!comparison.matches());

            let mut iter = entries(&table[..], 0);
            assert_eq!(iter.by_ref().take_while(Result::is_ok).count(), index);
            assert_eq!(iter.next(), None, "the iterator stops after an error");
            assert_eq!(iter.end(), Some(Err(TableError::Magic { index, magic })));
        }
    }
}

#[test]
fn a_full_table_without_an_end_is_unterminated() {
    let slots = [entry_bytes(&PARTITIONS[0]); MAX_ENTRIES];
    let table = table_of(&slots);
    let comparison = compare(&table[..], 0, &PARTITIONS);
    assert_eq!(comparison.end, Err(TableError::Unterminated));
    assert_eq!(comparison.extra, MAX_ENTRIES - N);
    assert_eq!(entries(&table[..], 0).count(), MAX_ENTRIES + 1);

    // With room for the end in the last slot, it is well formed.
    let mut slots = slots.to_vec();
    slots[MAX_ENTRIES - 1] = md5_entry();
    let table = table_of(&slots);
    assert!(matches!(
        compare(&table[..], 0, &PARTITIONS).end,
        Ok(End::Md5 { index: 95, .. })
    ));
}

#[test]
fn a_truncated_table_is_unreadable_where_it_stops() {
    let complete = (N + 1) * 32;
    for len in 0..complete {
        let table = &TABLE[..len];
        let comparison = compare(table, 0, &PARTITIONS);
        let index = len / 32;
        assert_eq!(
            comparison.end,
            Err(TableError::Read {
                index,
                error: ReadError::OutOfRange
            }),
            "{len}"
        );
        assert_eq!(
            comparison
                .partitions
                .iter()
                .filter(|m| **m == Match::Matches)
                .count(),
            index.min(N)
        );
    }
    assert!(compare(&TABLE[..complete], 0, &PARTITIONS).matches());
}

#[test]
fn a_table_past_the_end_of_the_offsets_is_unreadable() {
    let comparison = compare(TABLE, u32::MAX - 16, &PARTITIONS);
    assert_eq!(
        comparison.end,
        Err(TableError::Read {
            index: 0,
            error: ReadError::OutOfRange
        })
    );
    let comparison = compare(TABLE, u32::MAX - 31, &PARTITIONS);
    assert_eq!(
        comparison.end,
        Err(TableError::Read {
            index: 0,
            error: ReadError::OutOfRange
        })
    );
}

/// A source whose reads fail from `from` on.
struct Failing<'a> {
    bytes: &'a [u8],
    from: u32,
}

impl Source for Failing<'_> {
    fn size(&self) -> u32 {
        self.bytes.size()
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        if offset + buf.len() as u32 > self.from {
            return Err(ReadError::Device);
        }
        self.bytes.read(offset, buf)
    }
}

#[test]
fn a_failing_read_is_reported_with_its_entry() {
    let source = Failing {
        bytes: TABLE,
        from: 3 * 32 + 1,
    };
    let comparison = compare(&source, 0, &PARTITIONS);
    assert_eq!(
        comparison.end,
        Err(TableError::Read {
            index: 3,
            error: ReadError::Device
        })
    );
    assert_eq!(comparison.partitions[2], Match::Matches);
    assert_eq!(comparison.partitions[3], Match::Missing);
}

#[test]
fn entries_reports_its_end_only_once_done() {
    let mut iter = entries(TABLE, 0);
    assert_eq!(iter.end(), None);
    assert!(iter.next().unwrap().is_ok());
    assert_eq!(iter.end(), None);
    assert!(matches!(iter.finish(), Ok(End::Md5 { index: N, .. })));
    assert_eq!(iter.next(), None);
    assert!(matches!(iter.end(), Some(Ok(End::Md5 { index: N, .. }))));
    assert!(std::format!("{iter:?}").contains("index: 8"));
}

// Labels.

#[test]
fn a_label_of_16_bytes_has_no_nul() {
    let entry = Entry {
        label: "abcdefghijklmnop",
        ..PARTITIONS[0]
    };
    let table = table_with_md5(&[entry]);
    let read = read_all(&table, 0)[0];
    assert_eq!(read.label.as_bytes(), b"abcdefghijklmnop");
    assert_eq!(read.label.to_str(), Some("abcdefghijklmnop"));
    assert!(read.differences(&entry).is_empty());
    let shorter = Entry {
        label: "abcdefghijklmno",
        ..entry
    };
    assert_eq!(read.differences(&shorter), Fields::LABEL);
}

#[test]
fn a_label_that_isnt_utf8_is_kept_as_bytes() {
    let mut table = TABLE.to_vec();
    table[12..16].copy_from_slice(&[0xFF, 0xFE, b'a', 0x80]);
    let read = read_all(&table, 0)[0];
    assert_eq!(
        read.label.as_bytes(),
        &[0xFF, 0xFE, b'a', 0x80, b'o', b'r', b'y']
    );
    assert_eq!(read.label.to_str(), None);
    assert!(std::format!("{read:?}").contains(r"\xff\xfea\x80ory"));
    only_differs(&table, 0, Fields::LABEL);
}

#[test]
fn an_empty_label_is_kept() {
    let mut table = TABLE.to_vec();
    table[12..28].fill(0);
    assert_eq!(read_all(&table, 0)[0].label.as_bytes(), b"");
    only_differs(&table, 0, Fields::LABEL);
}

// Fields.

#[test]
fn fields_combine_and_print() {
    let fields = Fields::LABEL | Fields::SIZE;
    assert!(fields.contains(Fields::LABEL));
    assert!(fields.contains(Fields::SIZE));
    assert!(!fields.contains(Fields::KIND));
    assert!(!fields.contains(Fields::LABEL | Fields::KIND));
    assert!(fields.contains(Fields::NONE));
    assert!(!fields.is_empty());
    assert!(Fields::NONE.is_empty());
    assert_eq!(Fields::default(), Fields::NONE);
    assert_eq!(fields.bits(), 0b1_0001);
    assert_eq!(std::format!("{fields:?}"), "Fields(LABEL | SIZE)");
    assert_eq!(std::format!("{:?}", Fields::NONE), "Fields()");
    let all = Fields::LABEL
        | Fields::KIND
        | Fields::SUBTYPE
        | Fields::OFFSET
        | Fields::SIZE
        | Fields::FLAGS;
    assert_eq!(all, Fields::ALL);
}

// Judging the table as the bootloader does.

/// The partitions the bootloader loads from `table` at 0, which has to be
/// valid for [`CHIP`].
fn loaded_entries(table: &[u8]) -> Vec<(usize, RawEntry)> {
    let verified = verify(table, 0, CHIP).unwrap();
    partitions(table, 0, &verified)
        .map(Result::unwrap)
        .collect()
}

/// The comparison by label of the partitions the bootloader loads from
/// `table` with the layout.
fn by_label(table: &[u8]) -> LabelComparison<N> {
    compare_labels(
        loaded_entries(table).into_iter().map(|(_, e)| e),
        &PARTITIONS,
    )
}

/// The bytes of `entries`, each a partition entry.
fn slots_of(entries: &[Entry]) -> Vec<[u8; 32]> {
    entries.iter().map(entry_bytes).collect()
}

/// What the bootloader keeps from the table at `offset` in `source`, which
/// has to read.
fn load_all(source: &[u8], offset: u32, verified: &Verified) -> Loaded {
    let mut loaded = Loaded::new();
    load(&mut loaded, source, offset, verified).unwrap();
    loaded
}

/// A field, and a change to it.
type FieldChange = (Fields, fn(&mut Entry));

#[test]
fn the_nix_table_is_valid_as_the_bootloader_judges_it() {
    let verified = verify(TABLE, 0, CHIP).unwrap();
    let mut digest = [0; 16];
    digest.copy_from_slice(&TABLE[N * 32 + 16..N * 32 + 32]);
    assert_eq!(
        verified,
        Verified {
            end: N + 1,
            md5: Some(Md5Entry { index: N, digest }),
        }
    );
    assert_eq!(verified.partitions(), N);
    assert!(!verified.md5_read_as_partition());
    let loaded = loaded_entries(TABLE);
    assert_eq!(loaded.len(), N);
    for ((index, entry), (i, expected)) in loaded.iter().zip(PARTITIONS.iter().enumerate()) {
        assert_eq!(*index, i);
        assert!(entry.differences(expected).is_empty(), "{}", expected.label);
    }
    assert!(by_label(TABLE).matches());
    assert_eq!(by_label(TABLE).expected, [Match::Matches; N]);
}

#[test]
fn the_espflash_table_is_valid_and_differs_from_the_layout_by_label() {
    let verified = verify(ESPFLASH_TABLE, 0, CHIP).unwrap();
    assert_eq!(verified.partitions(), 3);
    assert_eq!(verified.end, 4);
    let comparison = by_label(ESPFLASH_TABLE);
    // Its `factory` is an app at 0x10000; `nvs` and `phy_init` are extra.
    let mut expected = [Match::Missing; N];
    expected[0] = Match::Differs(
        Fields::KIND | Fields::SUBTYPE | Fields::OFFSET | Fields::SIZE | Fields::FLAGS,
    );
    assert_eq!(comparison.expected, expected);
    assert_eq!(comparison.extra, 2);
    assert!(!comparison.matches());
    let loaded = load_all(ESPFLASH_TABLE, 0, &verified);
    assert_eq!(
        loaded.factory,
        Some(Kept {
            index: 2,
            offset: 0x1_0000,
            size: 0x7F_0000
        })
    );
    assert_eq!(loaded.apps().count(), 1);
    assert_eq!(loaded.otadata, None);
}

#[test]
fn a_table_without_an_md5_entry_is_valid() {
    let table = table_of(&slots_of(&PARTITIONS));
    let verified = verify(&table[..], 0, CHIP).unwrap();
    assert_eq!(verified, Verified { end: N, md5: None });
    assert_eq!(verified.partitions(), N);
    assert!(!verified.md5_read_as_partition());
    assert!(by_label(&table).matches());
}

#[test]
fn a_bad_md5_makes_the_table_invalid() {
    let computed: [u8; 16] = TABLE[0x110..0x120].try_into().unwrap();
    for at in [0x110, 0x115, 0x11F] {
        let mut table = TABLE.to_vec();
        table[at] ^= 0x01;
        let stored: [u8; 16] = table[0x110..0x120].try_into().unwrap();
        assert_eq!(
            verify(&table[..], 0, CHIP),
            Err(TableError::Md5 {
                index: N,
                stored,
                computed
            }),
            "{at:#x}"
        );
    }
    // A change to any byte of an entry before it, a label byte past its NUL
    // that no field shows included.
    for at in [0x02, 0x0C, 0x14, 0x1F, 0x24, 0xFF] {
        let mut table = TABLE.to_vec();
        table[at] ^= 0x01;
        assert!(
            matches!(
                verify(&table[..], 0, CHIP),
                Err(TableError::Md5 { index: N, .. })
            ),
            "{at:#x}"
        );
    }
}

#[test]
fn the_md5_entry_s_padding_is_never_read() {
    let mut table = TABLE.to_vec();
    table[0x102..0x110].fill(0x00);
    assert!(verify(&table[..], 0, CHIP).is_ok());
    assert!(by_label(&table).matches());
}

#[test]
fn a_second_md5_entry_makes_the_table_invalid() {
    let mut slots = slots_of(&PARTITIONS);
    slots.push(md5_entry_for(&slots));
    // Even a second one whose digest would match.
    let again = md5_entry_for(&slots[..N]);
    slots.push(again);
    assert_eq!(
        verify(&table_of(&slots)[..], 0, CHIP),
        Err(TableError::SecondMd5 { index: N + 1 })
    );
}

#[test]
fn entries_between_the_md5_entry_and_the_end_entry_shift_the_partitions() {
    // Seven partitions, their MD5 entry, then the last partition.
    let mut slots = slots_of(&PARTITIONS[..N - 1]);
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&PARTITIONS[N - 1]));
    let table = table_of(&slots);
    let verified = verify(&table[..], 0, CHIP).unwrap();
    assert_eq!(verified.end, N + 1);
    assert_eq!(verified.md5.map(|md5| md5.index), Some(N - 1));
    // The bootloader loads the seven and the MD5 entry, read as a partition,
    // and leaves out the last entry.
    assert_eq!(verified.partitions(), N);
    assert!(verified.md5_read_as_partition());
    let loaded = loaded_entries(&table);
    assert_eq!(loaded.len(), N);
    let (index, md5_as_partition) = loaded[N - 1];
    assert_eq!(index, N - 1);
    assert_eq!(md5_as_partition, RawEntry::from_bytes(&slots[N - 1]));
    assert_eq!(
        (md5_as_partition.kind, md5_as_partition.subtype),
        (0xFF, 0xFF)
    );
    // So the table doesn't match the layout: its last partition is missing,
    // and the MD5 entry is extra.
    let comparison = by_label(&table);
    let mut expected = [Match::Matches; N];
    expected[N - 1] = Match::Missing;
    assert_eq!(comparison.expected, expected);
    assert_eq!(comparison.extra, 1);
    assert!(!comparison.matches());
}

#[test]
fn an_md5_entry_read_as_a_partition_is_loaded_as_its_bytes_say() {
    // An `ota_0` entry, then an MD5 entry whose unread bytes say `ota_0` at
    // 0x20_0000, then another entry before the end.
    let mut slots = slots_of(&PARTITIONS[2..3]);
    let mut md5 = md5_entry_for(&slots);
    md5[2] = TYPE_APP;
    md5[3] = 0x10;
    md5[4..8].copy_from_slice(&0x20_0000_u32.to_le_bytes());
    md5[8..12].copy_from_slice(&0x1_0000_u32.to_le_bytes());
    slots.push(md5);
    slots.push(entry_bytes(&PARTITIONS[1]));
    let table = table_of(&slots);
    let verified = verify(&table[..], 0, CHIP).unwrap();
    assert_eq!(verified.partitions(), 2);
    let loaded = load_all(&table[..], 0, &verified);
    // The last `ota_0` the bootloader reads is the MD5 entry's, and the
    // otadata after it is left out.
    assert_eq!(
        loaded.ota[0],
        Some(Kept {
            index: 1,
            offset: 0x20_0000,
            size: 0x1_0000
        })
    );
    assert_eq!(loaded.otadata, None);
}

#[test]
fn the_end_entry_is_its_magic_type_and_subtype() {
    // Its other 28 bytes aren't read.
    for fill in [0x00, 0x5A] {
        let mut table = TABLE.to_vec();
        table[0x124..0x140].fill(fill);
        assert!(verify(&table[..], 0, CHIP).is_ok(), "{fill:#x}");
        assert!(by_label(&table).matches());
    }
    // Nor is anything after it.
    let mut table = TABLE.to_vec();
    table[0x140..].fill(0x00);
    table[0x140..0x142].copy_from_slice(&0x1234_u16.to_le_bytes());
    assert!(verify(&table[..], 0, CHIP).is_ok());
    // Its type and subtype have to be erased.
    for at in [0x122, 0x123] {
        let mut table = TABLE.to_vec();
        table[at] = 0x00;
        assert_eq!(
            verify(&table[..], 0, CHIP),
            Err(TableError::Magic {
                index: N + 1,
                magic: 0xFFFF
            })
        );
    }
}

#[test]
fn an_md5_entry_and_an_end_entry_hold_no_partitions() {
    let table = table_of(&[md5_entry_for(&[])]);
    let verified = verify(&table[..], 0, CHIP).unwrap();
    assert_eq!(verified.end, 1);
    assert_eq!(verified.partitions(), 0);
    assert!(!verified.md5_read_as_partition());
    assert_eq!(partitions(&table[..], 0, &verified).count(), 0);
    assert_eq!(load_all(&table[..], 0, &verified), Loaded::new());
    let comparison = by_label(&table);
    assert_eq!(comparison.expected, [Match::Missing; N]);
    assert_eq!(comparison.extra, 0);
    // With a digest of something else, it is invalid.
    assert!(matches!(
        verify(&table_of(&[md5_entry()])[..], 0, CHIP),
        Err(TableError::Md5 { index: 0, .. })
    ));
}

#[test]
fn an_end_entry_first_is_no_table() {
    let erased = [0xFF; MAX_LEN as usize];
    assert_eq!(verify(&erased[..], 0, CHIP), Err(TableError::Empty));
    // Whatever follows it.
    let mut table = erased;
    table[32..64].copy_from_slice(&entry_bytes(&PARTITIONS[0]));
    assert_eq!(verify(&table[..], 0, CHIP), Err(TableError::Empty));
}

#[test]
fn any_other_magic_makes_the_table_invalid() {
    for index in 0..=N {
        for magic in [0x0000_u16, 0x50AB, 0xAA50, 0xEBEA, 0x1234] {
            let mut table = TABLE.to_vec();
            table[index * 32..index * 32 + 2].copy_from_slice(&magic.to_le_bytes());
            assert_eq!(
                verify(&table[..], 0, CHIP),
                Err(TableError::Magic { index, magic }),
                "{index} {magic:#x}"
            );
        }
    }
}

#[test]
fn a_table_needs_an_end_entry_within_its_length() {
    let slots = slots_of(&[PARTITIONS[0]; MAX_ENTRIES]);
    assert_eq!(
        verify(&table_of(&slots)[..], 0, CHIP),
        Err(TableError::Unterminated)
    );
    // An MD5 entry in the last slot doesn't end it.
    let mut with_md5 = slots[..MAX_ENTRIES - 1].to_vec();
    with_md5.push(md5_entry_for(&with_md5));
    assert_eq!(
        verify(&table_of(&with_md5)[..], 0, CHIP),
        Err(TableError::Unterminated)
    );
    // An end entry in the last slot does.
    let verified = verify(&table_of(&slots[..MAX_ENTRIES - 1])[..], 0, CHIP).unwrap();
    assert_eq!(verified.end, MAX_ENTRIES - 1);
    assert_eq!(verified.partitions(), MAX_ENTRIES - 1);
}

/// A table of `PARTITIONS` with `entry` in place of the one at `index`.
fn with_entry(index: usize, entry: Entry) -> Vec<u8> {
    let mut entries = PARTITIONS.to_vec();
    entries[index] = entry;
    table_with_md5(&entries)
}

#[test]
fn every_partition_entry_lies_inside_the_chip_size() {
    let reserved = PARTITIONS[N - 1];
    let at = |offset, size| {
        verify(
            &with_entry(
                N - 1,
                Entry {
                    offset,
                    size,
                    ..reserved
                },
            )[..],
            0,
            CHIP,
        )
    };
    let past = |offset, size| {
        Err(TableError::PastChip {
            index: N - 1,
            offset,
            size,
        })
    };
    // Up to the chip size, and an empty entry at its end.
    assert!(at(0x69_0000, 0x17_0000).is_ok());
    assert!(at(CHIP, 0).is_ok());
    assert_eq!(at(0x69_0000, 0x17_0001), past(0x69_0000, 0x17_0001));
    assert_eq!(at(CHIP + 1, 0), past(CHIP + 1, 0));
    assert_eq!(at(u32::MAX, 0), past(u32::MAX, 0));
    // The end wraps at 2^32 as the bootloader computes it. A wrapped end lies
    // below the offset, so an entry starting inside the chip whose end wraps
    // passes, whatever its size.
    assert!(at(0x69_0000, 0u32.wrapping_sub(0x69_0000)).is_ok());
    assert!(at(0x69_0000, u32::MAX - 0x68_0000).is_ok());
    assert!(at(0x69_0000, u32::MAX).is_ok());
    assert!(at(CHIP, u32::MAX).is_ok());
}

#[test]
fn a_partition_past_the_header_s_flash_size_is_invalid_inside_a_larger_chip() {
    // The header names 4 MiB: `ota_1` ends at 0x61_0000, past it, though the
    // chip holds 8 MiB.
    assert_eq!(
        verify(TABLE, 0, crate::image::flash_size(2)),
        Err(TableError::PastChip {
            index: 3,
            offset: 0x31_0000,
            size: 0x30_0000
        })
    );
    // An unknown code is 2 MiB, which `ota_0` passes the end of.
    assert_eq!(
        verify(TABLE, 0, crate::image::flash_size(0xF)),
        Err(TableError::PastChip {
            index: 2,
            offset: 0x1_0000,
            size: 0x30_0000
        })
    );
    assert!(verify(TABLE, 0, crate::image::flash_size(3)).is_ok());
    assert!(verify(TABLE, 0, crate::image::flash_size(4)).is_ok());
}

#[test]
fn entries_after_the_md5_entry_are_checked_too() {
    let mut slots = slots_of(&PARTITIONS);
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&Entry {
        offset: CHIP,
        size: 0x1000,
        ..PARTITIONS[N - 1]
    }));
    assert_eq!(
        verify(&table_of(&slots)[..], 0, CHIP),
        Err(TableError::PastChip {
            index: N + 1,
            offset: CHIP,
            size: 0x1000
        })
    );
    // The first reason in table order counts: an entry past the chip before
    // a bad MD5.
    let mut table = with_entry(
        0,
        Entry {
            offset: CHIP,
            ..PARTITIONS[0]
        },
    );
    table[0x110] ^= 1;
    assert!(matches!(
        verify(&table[..], 0, CHIP),
        Err(TableError::PastChip { index: 0, .. })
    ));
}

#[test]
fn a_table_that_can_t_be_read_is_invalid_where_it_stops() {
    let complete = (N + 2) * 32;
    for len in (0..complete).step_by(7) {
        assert_eq!(
            verify(&TABLE[..len], 0, CHIP),
            Err(TableError::Read {
                index: len / 32,
                error: ReadError::OutOfRange
            }),
            "{len}"
        );
    }
    assert!(verify(&TABLE[..complete], 0, CHIP).is_ok());
    assert_eq!(
        verify(TABLE, u32::MAX - 31, CHIP),
        Err(TableError::Read {
            index: 0,
            error: ReadError::OutOfRange
        })
    );
    let source = Failing {
        bytes: TABLE,
        from: 3 * 32 + 1,
    };
    assert_eq!(
        verify(&source, 0, CHIP),
        Err(TableError::Read {
            index: 3,
            error: ReadError::Device
        })
    );
}

#[test]
fn the_bootloader_keeps_the_last_partition_of_each_kind() {
    let otadata = PARTITIONS[1];
    let ota_0 = PARTITIONS[2];
    let entries = [
        Entry {
            offset: 0xD000,
            ..otadata
        },
        otadata,
        ota_0,
        Entry {
            offset: 0x40_0000,
            ..ota_0
        },
        Entry {
            label: "factory",
            subtype: 0x00,
            offset: 0x70_0000,
            size: 0x1_0000,
            ..ota_0
        },
        Entry {
            label: "test",
            subtype: 0x20,
            offset: 0x71_0000,
            size: 0x1_0000,
            ..ota_0
        },
        Entry {
            label: "ota_15",
            subtype: 0x1F,
            offset: 0x72_0000,
            size: 0x1_0000,
            ..ota_0
        },
    ];
    let table = table_with_md5(&entries);
    let verified = verify(&table[..], 0, CHIP).unwrap();
    let loaded = load_all(&table[..], 0, &verified);
    let kept = |index, entry: Entry| {
        Some(Kept {
            index,
            offset: entry.offset,
            size: entry.size,
        })
    };
    assert_eq!(loaded.otadata, kept(1, otadata));
    assert_eq!(loaded.ota[0], kept(3, entries[3]));
    assert_eq!(loaded.factory, kept(4, entries[4]));
    assert_eq!(loaded.test, kept(5, entries[5]));
    assert_eq!(loaded.ota[15], kept(6, entries[6]));
    assert!(loaded.ota[1..15].iter().all(Option::is_none));
    let apps: Vec<_> = loaded.apps().map(|kept| kept.index).collect();
    assert_eq!(apps, [4, 5, 3, 6]);
}

#[test]
fn partitions_of_other_kinds_are_left_out() {
    let ota_0 = PARTITIONS[2];
    let otadata = PARTITIONS[1];
    let mut entries = Vec::new();
    // Apps the bootloader doesn't boot from: other app subtypes, and TEE
    // apps without TEE support.
    for subtype in [0x01, 0x0F, 0x21, 0x2F, 0x30, 0x31, 0x40, 0xFF] {
        entries.push(Entry { subtype, ..ota_0 });
    }
    // Data that isn't the otadata, and other types.
    for (kind, subtype) in [
        (TYPE_DATA, 0x01),
        (TYPE_DATA, 0x90),
        (0x02, 0x00),
        (0x40, 0x10),
    ] {
        entries.push(Entry {
            kind,
            subtype,
            ..otadata
        });
    }
    let table = table_with_md5(&entries);
    let verified = verify(&table[..], 0, CHIP).unwrap();
    assert_eq!(load_all(&table[..], 0, &verified), Loaded::new());
    assert_eq!(Loaded::default(), Loaded::new());
}

#[test]
fn partitions_compare_with_the_layout_by_label_in_any_order() {
    let mut entries = PARTITIONS.to_vec();
    entries.swap(2, 3);
    entries.reverse();
    let table = table_with_md5(&entries);
    assert!(by_label(&table).matches());
}

#[test]
fn a_missing_partition_is_missing_and_the_rest_still_match() {
    for index in 0..N {
        let mut entries = PARTITIONS.to_vec();
        entries.remove(index);
        let comparison = by_label(&table_with_md5(&entries));
        let mut expected = [Match::Matches; N];
        expected[index] = Match::Missing;
        assert_eq!(comparison.expected, expected, "{index}");
        assert_eq!(comparison.extra, 0);
        assert!(!comparison.matches());
    }
}

#[test]
fn extra_and_repeated_labels_are_extra() {
    let mut entries = PARTITIONS.to_vec();
    entries.push(Entry {
        label: "spare",
        ..PARTITIONS[N - 1]
    });
    let comparison = by_label(&table_with_md5(&entries));
    assert_eq!(comparison.expected, [Match::Matches; N]);
    assert_eq!(comparison.extra, 1);
    assert!(!comparison.matches());

    // A second `ota_1` elsewhere: the first is compared, the second is extra.
    let mut entries = PARTITIONS.to_vec();
    entries.push(Entry {
        offset: 0x70_0000,
        size: 0x1_0000,
        ..PARTITIONS[3]
    });
    let comparison = by_label(&table_with_md5(&entries));
    assert_eq!(comparison.expected, [Match::Matches; N]);
    assert_eq!(comparison.extra, 1);
    // In the other order, the first differs.
    entries.swap(3, N);
    let comparison = by_label(&table_with_md5(&entries));
    assert_eq!(
        comparison.expected[3],
        Match::Differs(Fields::OFFSET | Fields::SIZE)
    );
    assert_eq!(comparison.extra, 1);
}

#[test]
fn a_partition_differs_in_the_fields_it_changes_never_its_label() {
    let changes: [FieldChange; 5] = [
        (Fields::KIND, |e| e.kind ^= 1),
        (Fields::SUBTYPE, |e| e.subtype ^= 1),
        (Fields::OFFSET, |e| e.offset -= 0x1000),
        (Fields::SIZE, |e| e.size -= 0x1000),
        (Fields::FLAGS, |e| e.flags ^= FLAG_READONLY),
    ];
    for index in 0..N {
        for (field, change) in changes {
            let mut entry = PARTITIONS[index];
            change(&mut entry);
            let comparison = by_label(&with_entry(index, entry));
            let mut expected = [Match::Matches; N];
            expected[index] = Match::Differs(field);
            assert_eq!(comparison.expected, expected, "{index} {field:?}");
            assert_eq!(comparison.extra, 0);
        }
    }
}

#[test]
fn a_label_with_bytes_after_its_nul_is_another_label() {
    let mut table = TABLE.to_vec();
    // "ota_0", NUL, then a stray byte, and the MD5 made again.
    table[2 * 32 + 12 + 10] = b'x';
    let slots: Vec<[u8; 32]> = table[..N * 32]
        .chunks(32)
        .map(|c| c.try_into().unwrap())
        .collect();
    table[N * 32..N * 32 + 32].copy_from_slice(&md5_entry_for(&slots));
    let comparison = by_label(&table);
    assert_eq!(comparison.expected[2], Match::Missing);
    assert_eq!(comparison.extra, 1);
}

#[test]
fn the_search_judges_candidates_as_the_bootloader_does() {
    let booted = 0x1_0000;
    // A table with a bad MD5 isn't one the bootloader read.
    let mut bad_md5 = TABLE.to_vec();
    bad_md5[0x110] ^= 1;
    let flash = flash_with(0x1_0000, &[(0x3000, &bad_md5), (0x8000, TABLE)]);
    assert_eq!(find(&flash[..], booted, CHIP), Found::One(0x8000));
    // Nor is one past the chip size the header gives.
    let flash = flash_with(0x1_0000, &[(0x8000, TABLE)]);
    assert_eq!(
        find(&flash[..], booted, crate::image::flash_size(2)),
        Found::Nothing
    );
    // A table without an MD5 entry is one.
    let plain = table_of(&slots_of(&PARTITIONS));
    let flash = flash_with(0x1_0000, &[(0x3000, &plain), (0x8000, TABLE)]);
    assert_eq!(
        find(&flash[..], booted, CHIP),
        Found::Several(0x3000, 0x8000)
    );
    // An app read from an MD5 entry's unread bytes counts, and one the
    // bootloader leaves out as the last entry doesn't.
    let mut slots = vec![entry_bytes(&PARTITIONS[1])];
    let mut md5 = md5_entry_for(&slots);
    md5[2] = TYPE_APP;
    md5[4..8].copy_from_slice(&booted.to_le_bytes());
    slots.push(md5);
    slots.push(entry_bytes(&PARTITIONS[1]));
    let shifted = table_of(&slots);
    let flash = flash_with(0x1_0000, &[(0x3000, &shifted)]);
    assert_eq!(find(&flash[..], booted, CHIP), Found::One(0x3000));
    let mut slots = slots_of(&PARTITIONS[..2]);
    slots.push(md5_entry_for(&slots));
    slots.push(entry_bytes(&PARTITIONS[2]));
    let left_out = table_of(&slots);
    let flash = flash_with(0x1_0000, &[(0x3000, &left_out)]);
    assert_eq!(find(&flash[..], booted, CHIP), Found::Nothing);
}

// Finding a moved table.

const BOOTLOADER: &[u8] = &[0xE9; 0x100];

#[test]
fn the_nix_table_is_found_where_it_was_moved() {
    for offset in [0x1000, 0x8000, 0xC000, 0xF000] {
        let flash = flash_with(0x1_0000, &[(0, BOOTLOADER), (offset, TABLE)]);
        assert_eq!(
            find(&flash[..], 0x1_0000, CHIP),
            Found::One(offset),
            "{offset:#x}"
        );
    }
    // Booted from ota_1, the same table is found: it has an app entry there.
    let flash = flash_with(0x31_0000, &[(0, BOOTLOADER), (0xC000, TABLE)]);
    assert_eq!(find(&flash[..], 0x31_0000, CHIP), Found::One(0xC000));
    // Booted from ota_1, a table below it but above ota_0 is found too.
    let flash = flash_with(0x31_0000, &[(0, BOOTLOADER), (0x30_0000, TABLE)]);
    assert_eq!(find(&flash[..], 0x31_0000, CHIP), Found::One(0x30_0000));
}

#[test]
fn the_espflash_table_is_found_where_it_was_moved() {
    for offset in [0x8000, 0x9000, 0xE000] {
        let flash = flash_with(0x1_0000, &[(0, BOOTLOADER), (offset, ESPFLASH_TABLE)]);
        assert_eq!(
            find(&flash[..], 0x1_0000, CHIP),
            Found::One(offset),
            "{offset:#x}"
        );
    }
}

#[test]
fn two_tables_with_the_booted_app_are_several() {
    // A table left over from an earlier layout, below or above the current
    // one: either could be the one the bootloader read.
    let flash = flash_with(0x1_0000, &[(0x3000, ESPFLASH_TABLE), (0x8000, TABLE)]);
    assert_eq!(
        find(&flash[..], 0x1_0000, CHIP),
        Found::Several(0x3000, 0x8000)
    );
    let flash = flash_with(0x1_0000, &[(0x8000, TABLE), (0xC000, TABLE)]);
    assert_eq!(
        find(&flash[..], 0x1_0000, CHIP),
        Found::Several(0x8000, 0xC000)
    );
    // The first two of three.
    let flash = flash_with(
        0x1_0000,
        &[(0x2000, TABLE), (0x8000, TABLE), (0xC000, ESPFLASH_TABLE)],
    );
    assert_eq!(
        find(&flash[..], 0x1_0000, CHIP),
        Found::Several(0x2000, 0x8000)
    );
    // Booted from ota_1, a table inside ota_0 counts too.
    let flash = flash_with(0x31_0000, &[(0x8000, TABLE), (0x20_0000, TABLE)]);
    assert_eq!(
        find(&flash[..], 0x31_0000, CHIP),
        Found::Several(0x8000, 0x20_0000)
    );
}

#[test]
fn only_one_found_table_has_an_offset() {
    assert_eq!(Found::One(0x8000).offset(), Some(0x8000));
    assert_eq!(Found::Nothing.offset(), None);
    assert_eq!(Found::Several(0x8000, 0xC000).offset(), None);
}

#[test]
fn tables_without_the_booted_app_are_skipped() {
    // Well formed, with an app elsewhere.
    let elsewhere = table_with_md5(&[Entry {
        offset: 0x2_0000,
        ..PARTITIONS[2]
    }]);
    // Well formed, with a data partition at the booted offset.
    let data_there = table_with_md5(&[Entry {
        kind: TYPE_DATA,
        ..PARTITIONS[2]
    }]);
    // With the app, but malformed after it.
    let mut malformed = table_with_md5(&PARTITIONS);
    malformed[3 * 32] = 0;
    // With the app, but no end within the table's length.
    let unterminated = table_of(&[entry_bytes(&PARTITIONS[2]); MAX_ENTRIES]);
    let flash = flash_with(
        0x10_0000,
        &[
            (0, BOOTLOADER),
            (0x1000, &elsewhere),
            (0x2000, &data_there),
            (0x3000, &malformed),
            (0x4000, &unterminated[..]),
            (0xC000, TABLE),
        ],
    );
    assert_eq!(find(&flash[..], 0x1_0000, CHIP), Found::One(0xC000));
    assert_eq!(find(&flash[..], 0x31_0000, CHIP), Found::One(0xC000));
    // The espflash table has an app at ota_0's offset but not at ota_1's.
    let flash = flash_with(0x10_0000, &[(0x8000, ESPFLASH_TABLE), (0xC000, TABLE)]);
    assert_eq!(
        find(&flash[..], 0x1_0000, CHIP),
        Found::Several(0x8000, 0xC000)
    );
    assert_eq!(find(&flash[..], 0x31_0000, CHIP), Found::One(0xC000));
}

#[test]
fn a_table_in_sector_0_or_at_the_booted_offset_isnt_found() {
    let flash = flash_with(0x2_0000, &[(0, TABLE), (0x1_0000, TABLE)]);
    assert_eq!(find(&flash[..], 0x1_0000, CHIP), Found::Nothing);
}

#[test]
fn no_table_is_found_on_an_erased_flash() {
    let flash = vec![0xFF; 0x1_0000];
    assert_eq!(find(&flash[..], 0x1_0000, CHIP), Found::Nothing);
    let flash = vec![0x00; 0x1_0000];
    assert_eq!(find(&flash[..], 0x1_0000, CHIP), Found::Nothing);
}

#[test]
fn a_booted_offset_of_0_or_off_a_sector_finds_nothing() {
    let flash = flash_with(0x2_0000, &[(0x8000, TABLE)]);
    assert_eq!(find(&flash[..], 0, CHIP), Found::Nothing);
    assert_eq!(find(&flash[..], 0x1000, CHIP), Found::Nothing);
    assert_eq!(find(&flash[..], 0x1_0001, CHIP), Found::Nothing);
    assert_eq!(find(&flash[..], 0x1_0800, CHIP), Found::Nothing);
    // An app entry at an offset off a sector, matching it, still isn't taken.
    let odd = table_with_md5(&[Entry {
        offset: 0x1_0800,
        ..PARTITIONS[2]
    }]);
    let flash = flash_with(0x2_0000, &[(0x8000, &odd)]);
    assert_eq!(find(&flash[..], 0x1_0800, CHIP), Found::Nothing);
}

#[test]
fn a_flash_shorter_than_the_booted_offset_is_searched_as_far_as_it_goes() {
    let flash = flash_with(0xD000, &[(0xC000, TABLE)]);
    assert_eq!(find(&flash[..], 0x31_0000, CHIP), Found::One(0xC000));
    // The table needs its entries, the MD5 entry and the end entry.
    assert_eq!(find(&flash[..0xC140], 0x31_0000, CHIP), Found::One(0xC000));
    assert_eq!(find(&flash[..0xC13F], 0x31_0000, CHIP), Found::Nothing);
    assert_eq!(find(&[][..], 0x31_0000, CHIP), Found::Nothing);
}

// Never panicking.

#[test]
fn random_tables_never_panic() {
    let mut rng = Rng::new(0x5EED_7AB1E);
    let mut table = vec![0; MAX_LEN as usize];
    for round in 0..4000 {
        rng.fill(&mut table);
        // Most random magics end the table at once, so most slots get a valid
        // magic, and some an MD5 or erased one.
        for slot in table.as_chunks_mut::<32>().0 {
            let magic: u16 = match rng.below(40) {
                0 => MD5_MAGIC,
                1 => {
                    slot[2] = 0xFF;
                    slot[3] = 0xFF;
                    0xFFFF
                }
                2 => rng.next_u32() as u16,
                _ => MAGIC,
            };
            slot[..2].copy_from_slice(&magic.to_le_bytes());
            if rng.below(4) == 0 {
                slot[2] = TYPE_APP;
                slot[4..8].copy_from_slice(&0x1_0000_u32.to_le_bytes());
            }
        }
        let len = if round % 3 == 0 {
            rng.below(table.len() + 1)
        } else {
            table.len()
        };
        let source = &table[..len];
        exercise(source, 0);
        let comparison = compare(source, 0, &PARTITIONS);
        let mut iter = entries(source, 0);
        let count = iter.by_ref().flatten().count();
        assert!(count <= MAX_ENTRIES);
        assert_eq!(Some(comparison.end), iter.end());
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    let mut rng = Rng::new(0xF1_7E5);
    for fixture in [TABLE, ESPFLASH_TABLE] {
        let interesting = (N + 2) * 32;
        for at in 0..interesting {
            for value in [0x00, 0xFF, 0x50, 0xAA, 0xEB, rng.byte()] {
                let mut table = fixture.to_vec();
                table[at] = value;
                exercise(&table, 0);
            }
        }
        for _ in 0..2000 {
            let mut table = fixture.to_vec();
            for _ in 0..=rng.below(8) {
                let at = rng.below(interesting);
                table[at] = rng.byte();
            }
            exercise(&table, 0);
        }
    }
}

#[test]
fn random_flash_never_panics_discovery() {
    let mut rng = Rng::new(0xD15C0);
    let mut flash = vec![0; 0x1_0000];
    for round in 0..64_u32 {
        rng.fill(&mut flash);
        // Put table-like sectors in: valid magics, an app at the booted offset.
        for sector in 1..16 {
            if rng.below(2) == 0 {
                let at = sector * 0x1000;
                for slot in flash[at..at + 0x200].as_chunks_mut::<32>().0 {
                    slot[..2].copy_from_slice(&MAGIC.to_le_bytes());
                    if rng.below(3) == 0 {
                        slot[2] = TYPE_APP;
                        slot[4..8].copy_from_slice(&0x1_0000_u32.to_le_bytes());
                    }
                }
                if rng.below(2) == 0 {
                    let end = at + 32 * rng.below(16);
                    flash[end..end + 32].copy_from_slice(&md5_entry());
                }
            }
        }
        let booted = match round % 3 {
            0 => 0x1_0000,
            1 => 0x1000 * (round % 17),
            _ => rng.next_u32() & !0xFFF,
        };
        let tables: Vec<u32> = (1..booted / SECTOR_SIZE)
            .map(|sector| sector * SECTOR_SIZE)
            .filter(|&offset| boots(&flash[..], offset, booted, CHIP))
            .collect();
        let expected = match tables[..] {
            [] => Found::Nothing,
            [offset] => Found::One(offset),
            [first, second, ..] => Found::Several(first, second),
        };
        assert_eq!(find(&flash[..], booted, CHIP), expected);
    }
}

/// Runs every table function over `source` at `offset`.
fn exercise(source: &[u8], offset: u32) {
    for chip in [CHIP, 0, 0x1000, u32::MAX] {
        if let Ok(verified) = verify(source, offset, chip) {
            assert!(verified.end < MAX_ENTRIES);
            assert!(verified.partitions() <= verified.end);
            let read: Vec<_> = partitions(source, offset, &verified).collect();
            assert_eq!(read.len(), verified.partitions());
            let loaded = load_all(source, offset, &verified);
            assert!(loaded.apps().all(|kept| kept.index < verified.partitions()));
            let labels = compare_labels(read.into_iter().map(|r| r.unwrap().1), &PARTITIONS);
            let _ = labels.matches();
        }
    }
    let comparison = compare(source, offset, &PARTITIONS);
    let _ = comparison.matches();
    let _ = std::format!("{comparison:?}");
    let _ = compare(source, TABLE_OFFSET, &PARTITIONS);
    for entry in entries(source, offset).flatten() {
        let _ = entry.label.to_str();
        let _ = std::format!("{entry:?}");
    }
    let _ = find(source, 0x1_0000, CHIP);
}

fn entry_of(entry: &Entry) -> RawEntry {
    RawEntry::from_bytes(&entry_bytes(entry))
}
