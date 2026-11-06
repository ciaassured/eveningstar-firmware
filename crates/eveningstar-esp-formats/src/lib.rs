//! The formats ESP-IDF keeps on the flash: the partition table, ESP images
//! (the bootloader and the apps) and the otadata that selects the app to
//! boot.
//!
//! Every parser reads through a [`Source`], with buffers of at most 256 bytes
//! on the stack, and returns an error rather than panicking on any input.
//! Nothing here touches a flash: the otadata planners ([`otadata`]) compute
//! the erases and programs ESP-IDF's rules call for, and the caller carries
//! them out.
//!
//! An image's verification hashes through a [`hash::Sha256Hasher`] and hands
//! control back between its steps through a [`pace::Pace`]: SHA-256 in
//! software and a yield, unless the caller gives others
//! ([`image::verify_app_with`]).
//!
//! The crate is `no_std` and allocation-free. The `test-support` feature adds
//! [`test_support`], the fixtures and builders the crate's tests use, which
//! allocate.

#![cfg_attr(not(test), no_std)]

mod future;
pub mod hash;
pub mod image;
pub mod otadata;
pub mod pace;
mod source;
pub mod table;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod text;

pub use source::{ReadError, Source};
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

/// One of the two app slots, `ota_0` and `ota_1`, which ESP-IDF's otadata
/// numbers 0 and 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Slot 0, `ota_0`.
    Ota0,
    /// Slot 1, `ota_1`.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_number_and_pair_up() {
        assert_eq!(Slot::ALL.map(Slot::number), [0, 1]);
        for slot in Slot::ALL {
            assert_ne!(slot.other(), slot);
            assert_eq!(slot.other().other(), slot);
        }
    }

    #[test]
    fn end_is_offset_plus_size() {
        let entry = Entry {
            label: "ota_0",
            kind: TYPE_APP,
            subtype: 0x10,
            offset: 0x1_0000,
            size: 0x30_0000,
            flags: 0,
        };
        assert_eq!(entry.end(), 0x31_0000);
    }
}
