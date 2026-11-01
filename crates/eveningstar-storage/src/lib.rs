//! Storage on the internal flash: the partition layout, the formats on the
//! flash (the partition table, ESP images and the otadata), the checks of the
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
//! The crate is `no_std`, allocation-free, and also builds on the host. Every
//! parser reads through [`Source`], with buffers of at most 256 bytes on the
//! stack, and returns an error rather than panicking on any input.

#![cfg_attr(not(test), no_std)]

mod flash;
mod future;
pub mod image;
pub mod known;
pub mod layout;
mod ota;
pub mod otadata;
pub mod partitions;
mod region;
mod source;
mod split;
pub mod table;
#[cfg(test)]
mod test_support;
mod text;

pub use flash::{DeviceError, Flash, SECTOR_SIZE, WORD_SIZE};
pub use ota::{Ota, OtaError, Update, VerifiedImage};
pub use region::AccessError;
pub use source::{ReadError, Source};
pub use split::{AppId, BootChain, BootloaderId, Data, Limit, Report, Storage, Tier, split};
pub use text::Text;

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

/// One of the two app slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// [`layout::OTA_0`].
    Ota0,
    /// [`layout::OTA_1`].
    Ota1,
}

impl Slot {
    /// Both slots, in order.
    pub const ALL: [Self; 2] = [Self::Ota0, Self::Ota1];

    /// The slot's number: 0 for `ota_0`, 1 for `ota_1`.
    #[must_use]
    pub const fn number(self) -> u32 {
        match self {
            Self::Ota0 => 0,
            Self::Ota1 => 1,
        }
    }

    /// The other slot.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Ota0 => Self::Ota1,
            Self::Ota1 => Self::Ota0,
        }
    }

    /// The slot's partition.
    #[must_use]
    pub const fn partition(self) -> Entry {
        match self {
            Self::Ota0 => layout::OTA_0,
            Self::Ota1 => layout::OTA_1,
        }
    }

    /// The configuration partition paired with the slot.
    #[must_use]
    pub const fn config(self) -> Entry {
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
