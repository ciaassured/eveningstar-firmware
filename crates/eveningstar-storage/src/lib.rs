//! Storage on the internal flash: the partition layout, the checks of the
//! board's table and bootloader, the typed handles to the partitions, and the
//! OTA mechanism.
//!
//! [`split`] reads the board through a [`Flash`] once per boot and hands out
//! the handles of the trust tier it reaches ([`Storage`]); the
//! [`partitions`] module documents the handles.
//!
//! In the full tier, [`Ota`] alone writes the other app slot and the otadata.
//! An update is written and verified ([`Ota::begin_update`],
//! [`Update::write`], [`Update::finish`]), the [`VerifiedImage`] it gives is
//! selected for the next boot ([`Ota::activate`]), and the image that boots
//! is confirmed ([`Ota::confirm`]) or rolled back ([`Ota::roll_back`]). Any
//! booted image that isn't `Valid` needs a confirmation
//! ([`otadata::BootState::needs_confirmation`]), and updates begin only from
//! a `Valid` one. A power cut at any step leaves a selection that starts a
//! whole image: the one running, or an activated one with its rollback
//! armed. Nothing in the crate reboots.
//!
//! The formats on the flash (the partition table, ESP images and the otadata)
//! are read by `eveningstar-esp-formats`, whose [`image`], [`otadata`] and
//! [`table`] modules are re-exported. The crate is `no_std`, allocation-free,
//! and also builds on the host.

#![cfg_attr(not(test), no_std)]

mod board_table;
mod flash;
pub mod known;
pub mod layout;
mod ota;
pub mod partitions;
mod region;
mod split;
#[cfg(test)]
mod test_support;

pub use board_table::{LayoutComparison, compare_layout};
pub use eveningstar_esp_formats::{Entry, ReadError, Slot, Source, Text, image, otadata, table};
pub use flash::{DeviceError, Flash, SECTOR_SIZE, WORD_SIZE};
pub use ota::{Ota, OtaError, Update, VerifiedImage};
pub use region::AccessError;
pub use split::{AppId, BootChain, BootloaderId, Data, Limit, Report, Storage, Tier, split};

// The otadata partition holds the otadata's two sectors.
const _: () = assert!(layout::OTADATA.size == 2 * otadata::SECTOR_SIZE);

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
}
