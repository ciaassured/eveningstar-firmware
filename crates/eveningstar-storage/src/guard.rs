//! The guard every erase and program passes, and the errors of both.
//!
//! Every erase and program of a write session checks its absolute range
//! against the protected spans before it reaches the device: everything below
//! the first partition (the bootloader and the table's sector),
//! [`layout::FACTORY`], [`layout::RESERVED`] and the slot that booted, all
//! from the compiled layout. A range that touches one is refused whole.

use embedded_storage_async::nor_flash::{NorFlashError, NorFlashErrorKind};

use crate::{Entry, ReadError, Slot, SlotPartitions as _, layout};

/// The most bytes one step reads or programs.
pub(crate) const CHUNK_LEN: u32 = 4096;

/// Why an erase or program was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessError {
    /// Part of the range lies past the end of the region, or the range ends
    /// before it starts.
    OutOfRange,
    /// An erase that doesn't start and end on sector boundaries, or a program
    /// whose offset or length isn't a multiple of
    /// [`WORD_SIZE`](crate::WORD_SIZE).
    NotAligned,
    /// The range touches a protected span: the bootloader, the table's
    /// sector, `factory`, `reserved` or the slot that booted. Nothing was
    /// erased or programmed.
    Protected,
    /// The flash failed the operation; the sectors or bytes before the one
    /// that failed were erased or programmed.
    Device,
}

impl From<ReadError> for AccessError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::OutOfRange => Self::OutOfRange,
            // A read of a region written under it returned bytes that can't
            // be used, as a failed read does.
            ReadError::Device | ReadError::Stale => Self::Device,
        }
    }
}

impl NorFlashError for AccessError {
    fn kind(&self) -> NorFlashErrorKind {
        match self {
            Self::OutOfRange => NorFlashErrorKind::OutOfBounds,
            Self::NotAligned => NorFlashErrorKind::NotAligned,
            Self::Protected | Self::Device => NorFlashErrorKind::Other,
        }
    }
}

/// Offset of the first partition: everything below it is the bootloader and
/// the table's sector.
pub(crate) const BOOT_END: u32 = first_offset(&layout::PARTITIONS);

const fn first_offset(partitions: &[Entry]) -> u32 {
    let mut first = u32::MAX;
    let mut i = 0;
    while i < partitions.len() {
        if partitions[i].offset < first {
            first = partitions[i].offset;
        }
        i += 1;
    }
    first
}

const _: () = assert!(layout::TABLE_OFFSET < BOOT_END);

/// Whether `start..end`, absolute, overlaps a protected span while the app
/// runs from `running`: below the first partition, `factory`, `reserved`
/// and the running slot.
pub(crate) fn is_protected(start: u32, end: u32, running: Slot) -> bool {
    let overlaps = |from: u32, to: u32| start < to && from < end;
    let entry = |entry: Entry| overlaps(entry.offset, entry.end());
    overlaps(0, BOOT_END)
        || entry(layout::FACTORY)
        || entry(layout::RESERVED)
        || entry(running.partition())
}

#[cfg(test)]
mod tests;
