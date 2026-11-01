//! Storage on the internal flash: the partition layout and, later, the checks
//! of the table and bootloader on the board and the handles to the partitions.
//!
//! The crate is `no_std` and also builds on the host.

#![no_std]

pub mod layout;

/// Partition type of an application image.
pub const TYPE_APP: u8 = 0x00;

/// Partition type of data.
pub const TYPE_DATA: u8 = 0x01;

/// Entry flag: the partition is read-only. The bit is the one the ESP-IDF
/// partition table format uses.
pub const FLAG_READONLY: u32 = 1 << 1;

/// One partition of the table, with the fields the table stores for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Name, at most 16 bytes.
    pub label: &'static str,
    /// Partition type (`kind`, as `type` is a keyword).
    pub kind: u8,
    /// Partition subtype, which depends on the type.
    pub subtype: u8,
    /// Offset from the start of the flash, in bytes.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
    /// Flag bits, such as [`FLAG_READONLY`].
    pub flags: u32,
}

impl Entry {
    /// Offset of the first byte after the partition.
    #[must_use]
    pub const fn end(&self) -> u32 {
        self.offset + self.size
    }
}

#[cfg(test)]
mod tests {
    use super::layout::{
        CONFIG_0, CONFIG_1, FLASH_SIZE, OTA_0, OTA_1, OTADATA, PARTITIONS, TABLE_OFFSET,
    };
    use super::*;

    const SECTOR_SIZE: u32 = 4096;
    const APP_ALIGNMENT: u32 = 64 * 1024;
    const FIRST_OFFSET: u32 = 0x9000;

    fn alignment(entry: &Entry) -> u32 {
        if entry.kind == TYPE_APP {
            APP_ALIGNMENT
        } else {
            SECTOR_SIZE
        }
    }

    #[test]
    fn partitions_are_aligned_for_their_type() {
        for entry in PARTITIONS {
            assert_eq!(entry.offset % alignment(&entry), 0, "{}", entry.label);
            assert_eq!(entry.size % alignment(&entry), 0, "{}", entry.label);
            assert_ne!(entry.size, 0, "{}", entry.label);
        }
    }

    #[test]
    fn partitions_do_not_overlap() {
        for (i, a) in PARTITIONS.iter().enumerate() {
            for b in &PARTITIONS[i + 1..] {
                assert!(
                    a.end() <= b.offset || b.end() <= a.offset,
                    "{} overlaps {}",
                    a.label,
                    b.label
                );
            }
        }
    }

    #[test]
    fn partitions_lie_between_the_table_and_the_end_of_the_flash() {
        const { assert!(TABLE_OFFSET + SECTOR_SIZE <= FIRST_OFFSET) };
        for entry in PARTITIONS {
            assert!(entry.offset >= FIRST_OFFSET, "{}", entry.label);
            assert!(entry.end() <= FLASH_SIZE, "{}", entry.label);
        }
    }

    #[test]
    fn labels_are_unique_and_fit_the_table() {
        for (i, a) in PARTITIONS.iter().enumerate() {
            assert!(a.label.len() <= 16, "{}", a.label);
            for b in &PARTITIONS[i + 1..] {
                assert_ne!(a.label, b.label);
            }
        }
    }

    #[test]
    fn slot_pairs_have_equal_sizes() {
        assert_eq!(CONFIG_0.size, CONFIG_1.size);
        assert_eq!(OTA_0.size, OTA_1.size);
    }

    #[test]
    fn app_slots_are_apps_and_otadata_is_data() {
        assert_eq!(OTA_0.kind, TYPE_APP);
        assert_eq!(OTA_1.kind, TYPE_APP);
        assert_eq!(OTADATA.kind, TYPE_DATA);
    }

    #[test]
    fn end_is_offset_plus_size() {
        assert_eq!(OTA_0.end(), 0x31_0000);
    }
}
