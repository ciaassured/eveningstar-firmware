use eveningstar_esp_formats::table::{self, End, Fields, Match, TableError};

use super::*;
use crate::SECTOR_SIZE;
use crate::layout::{PARTITIONS, TABLE_OFFSET};
use crate::test_support::{ESPFLASH_TABLE, TABLE, flash_with};

const N: usize = PARTITIONS.len();

#[test]
fn the_nix_table_holds_the_layout() {
    let entries: Vec<_> = table::entries(TABLE, 0).map(Result::unwrap).collect();
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
fn the_nix_table_matches_at_the_compiled_offset() {
    let flash = flash_with(0x1_0000, &[(TABLE_OFFSET, TABLE)]);
    let comparison = compare_layout(&flash[..]);
    assert_eq!(comparison.partitions, [Match::Matches; N]);
    assert_eq!(comparison.extra, 0);
    assert!(matches!(
        comparison.end,
        Ok(End::Md5 {
            index: N,
            matches: true,
            ..
        })
    ));
    assert!(comparison.matches());
    // The same bytes one sector further on aren't at the compiled offset.
    let moved = flash_with(0x1_0000, &[(TABLE_OFFSET + SECTOR_SIZE, TABLE)]);
    let comparison = compare_layout(&moved[..]);
    assert_eq!(comparison.end, Err(TableError::Empty));
    assert_eq!(comparison.partitions, [Match::Missing; N]);
}

#[test]
fn the_espflash_table_differs_from_the_layout() {
    let flash = flash_with(0x1_0000, &[(TABLE_OFFSET, ESPFLASH_TABLE)]);
    let comparison = compare_layout(&flash[..]);
    assert_eq!(comparison, compare(ESPFLASH_TABLE, 0, &PARTITIONS));
    assert!(!comparison.matches());
    // nvs against factory, phy_init against otadata, the factory app against
    // ota_0.
    for m in &comparison.partitions[..3] {
        assert!(matches!(m, Match::Differs(f) if f.contains(Fields::LABEL)));
    }
    assert_eq!(comparison.partitions[3..], [Match::Missing; N - 3]);
}
