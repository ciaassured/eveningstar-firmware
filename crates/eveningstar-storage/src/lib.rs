//! Storage on the internal flash: the partition layout, the verification
//! chain that decides how far the board is trusted, and the handles of its
//! regions.
//!
//! [`split`] runs the verification chain once per boot through a [`NorDevice`]
//! and hands out the handles of the trust tier it reaches ([`chain::Storage`]);
//! the [`chain`] module documents the steps and the tiers. A handle is a reader
//! or a writer of a region with a role ([`roles`]), over a state table that
//! gives each region a [`WriteState`] ([`nor`]). Readers read through
//! snapshots, which fail once their region is written, and readers of
//! publishable roles publish views ([`PublicRead`]), the only bytes meant to
//! leave the device. Writers write only inside a session, whose erases and
//! programs pass the guard of the protected spans. The boot tier alone hands
//! out the writers of the boot selection, the alternate slot's and the
//! otadata's ([`chain::BootWriters`]). Nothing in the crate reboots.
//!
//! The formats on the flash (the partition table, ESP images and the otadata)
//! are read by `eveningstar-esp-formats`, whose [`image`], [`otadata`] and
//! [`table`] modules are re-exported. [`LAYOUT_RECORD`] is the compiled
//! layout as a layout record, the form in which an app image embeds the
//! layout it is built for. The crate is `no_std`, allocation-free,
//! and also builds on the host. The `test-support` feature adds the
//! `test_support` module: a NOR flash in RAM that takes power cuts, the
//! simulated ESP-IDF bootloader run on it, and handles over it.
//!
//! [`split`]: chain::split
//! [`NorDevice`]: nor::NorDevice

#![cfg_attr(not(test), no_std)]

pub mod chain;
mod guard;
pub mod known;
pub mod layout;
pub mod nor;
pub mod roles;
mod span;
mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod view;

pub use chain::{Report, Storage, Tier, split};
pub use eveningstar_esp_formats::{Entry, ReadError, Slot, Source, Text, image, otadata, table};
pub use guard::AccessError;
pub use nor::{DeviceError, SECTOR_SIZE, WORD_SIZE};
pub use span::Span;
pub use state::{Epoch, WriteState};
pub use view::{MAX_SANITIZED, PublicRead, PublishError, SnapshotError, ViewError};

// The otadata partition holds the otadata's two sectors.
const _: () = assert!(layout::OTADATA.size == 2 * otadata::SECTOR_SIZE);

/// The layout record of the compiled layout, with one entry per partition.
pub type CompiledLayoutRecord = image::LayoutRecord<{ layout::PARTITIONS.len() }>;

/// The compiled layout as a layout record ([`image::LayoutRecord`]): the
/// table's offset, the flash size and every partition, in table order.
pub const LAYOUT_RECORD: CompiledLayoutRecord = match image::LayoutRecord::new(
    layout::TABLE_OFFSET,
    layout::FLASH_SIZE,
    &layout::PARTITIONS,
) {
    Some(record) => record,
    None => panic!("the compiled layout has a label longer than 16 bytes or too many partitions"),
};

/// The partitions the layout assigns to an app slot.
pub trait SlotPartitions {
    /// The slot's app partition.
    fn partition(self) -> Entry;

    /// The configuration partition paired with the slot.
    fn config(self) -> Entry;
}

impl SlotPartitions for Slot {
    fn partition(self) -> Entry {
        match self {
            Self::Ota0 => layout::OTA_0,
            Self::Ota1 => layout::OTA_1,
        }
    }

    fn config(self) -> Entry {
        match self {
            Self::Ota0 => layout::CONFIG_0,
            Self::Ota1 => layout::CONFIG_1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::layout::{
        CONFIG_0, CONFIG_1, FLASH_SIZE, OTA_0, OTA_1, OTADATA, PARTITIONS, TABLE_OFFSET,
    };
    use eveningstar_esp_formats::{TYPE_APP, TYPE_DATA};

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
    fn slots_name_the_app_partitions() {
        assert_eq!(Slot::Ota0.partition(), OTA_0);
        assert_eq!(Slot::Ota1.partition(), OTA_1);
        assert_eq!(Slot::Ota0.config(), CONFIG_0);
        assert_eq!(Slot::Ota1.config(), CONFIG_1);
        assert_eq!(Slot::ALL.map(Slot::number), [0, 1]);
        for slot in Slot::ALL {
            assert_ne!(slot.other(), slot);
            assert_eq!(slot.other().other(), slot);
        }
    }

    #[test]
    fn end_is_offset_plus_size() {
        assert_eq!(OTA_0.end(), 0x31_0000);
    }

    #[test]
    fn the_layout_record_holds_the_table_nix_builds() {
        use eveningstar_esp_formats::test_support::{
            FLASH_SIZE as BOARD_FLASH_SIZE, TABLE, TABLE_OFFSET as BOARD_TABLE_OFFSET, app_image,
            board_layout_record, with_layout,
        };

        assert_eq!(LAYOUT_RECORD.table_offset(), TABLE_OFFSET);
        assert_eq!(LAYOUT_RECORD.flash_size(), FLASH_SIZE);
        assert_eq!(LAYOUT_RECORD.entries().len(), PARTITIONS.len());
        for (i, entry) in LAYOUT_RECORD.entries().iter().enumerate() {
            assert_eq!(&entry[..], &TABLE[i * 32..i * 32 + 32], "entry {i}");
        }
        // The board the formats crate's fixtures describe is this layout.
        assert_eq!(
            (TABLE_OFFSET, FLASH_SIZE),
            (BOARD_TABLE_OFFSET, BOARD_FLASH_SIZE)
        );
        let image = with_layout(&app_image().build(), &board_layout_record());
        let embedded = image::app_layout(&image[..]).unwrap().unwrap();
        assert_eq!(embedded.matches(&image[..], &LAYOUT_RECORD), Ok(true));
    }
}
