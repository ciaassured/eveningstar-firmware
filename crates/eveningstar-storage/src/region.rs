//! Spans of the flash, which every handle wraps, and the guard every erase and
//! program passes.
//!
//! A region is a flash and a span of it that lies inside its capacity.
//! Offsets are relative to the start of the span and checked against its
//! length with checked arithmetic. Long operations run in steps of one sector
//! erased or at most [`CHUNK_LEN`] bytes programmed, read or hashed, with a
//! yield to the executor between them, and no step keeps more than 256 bytes
//! on the stack.
//!
//! Every erase and program also checks its absolute range against the
//! protected regions: everything below the first partition (the bootloader
//! and the table's sector), [`layout::FACTORY`], [`layout::RESERVED`] and the
//! running app's slot, derived from the generated layout and the slot each
//! write is given as running. A write given no running slot, as every
//! handle's is, treats both app slots as protected, so only a write that
//! knows which slot booted reaches the other one.

use core::fmt;

use embedded_storage_async::nor_flash::{NorFlashError, NorFlashErrorKind};
use sha2::{Digest as _, Sha256};

use eveningstar_esp_formats::pace::{Pace as _, Yield};

use crate::flash::{Flash, SECTOR_SIZE, WORD_SIZE};
use crate::{Entry, ReadError, Slot, SlotPartitions as _, Source, layout};

/// The most bytes one step reads, hashes or programs.
pub(crate) const CHUNK_LEN: u32 = 4096;

/// Length of the buffer hashing reads through.
const HASH_BUF_LEN: usize = 256;

/// Why an erase or program was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessError {
    /// Part of the range lies past the end of the partition, or the range
    /// ends before it starts.
    OutOfRange,
    /// An erase that doesn't start and end on sector boundaries, or a program
    /// whose offset or length isn't a multiple of
    /// [`WORD_SIZE`](crate::WORD_SIZE).
    NotAligned,
    /// The range touches a protected region: the bootloader, the table's
    /// sector, `factory`, `reserved` or an app slot. Nothing was erased or
    /// programmed.
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

/// Whether `start..end` overlaps a protected region while the app runs from
/// `running`: below the first partition, `factory`, `reserved` and the
/// running slot, or both slots when `running` is `None`.
pub(crate) fn is_protected(start: u32, end: u32, running: Option<Slot>) -> bool {
    let overlaps = |from: u32, to: u32| start < to && from < end;
    let entry = |entry: Entry| overlaps(entry.offset, entry.end());
    let slot =
        |slot: Slot| running.is_none_or(|running| running == slot) && entry(slot.partition());
    overlaps(0, BOOT_END)
        || entry(layout::FACTORY)
        || entry(layout::RESERVED)
        || slot(Slot::Ota0)
        || slot(Slot::Ota1)
}

/// A span of a flash.
#[derive(Clone, Copy)]
pub(crate) struct Region<'f> {
    flash: &'f dyn Flash,
    offset: u32,
    len: u32,
}

impl<'f> Region<'f> {
    /// The `len` bytes of `flash` at `offset`, or `None` when they run past
    /// its capacity.
    pub(crate) fn new(flash: &'f dyn Flash, offset: u32, len: u32) -> Option<Self> {
        let end = offset.checked_add(len)?;
        (end <= flash.capacity()).then_some(Self { flash, offset, len })
    }

    /// Up to `len` bytes of `flash` at `offset`: as many as lie inside its
    /// capacity, which may be none.
    pub(crate) fn clamped(flash: &'f dyn Flash, offset: u32, len: u32) -> Self {
        let capacity = flash.capacity();
        let offset = offset.min(capacity);
        Self {
            flash,
            offset,
            len: len.min(capacity - offset),
        }
    }

    /// Offset of the span from the start of the flash.
    pub(crate) const fn offset(&self) -> u32 {
        self.offset
    }

    /// Length of the span in bytes.
    pub(crate) const fn len(&self) -> u32 {
        self.len
    }

    /// The absolute address of the `len` bytes at `offset` in the span, or
    /// `None` when they don't lie inside it.
    fn address(&self, offset: u32, len: u32) -> Option<u32> {
        let end = offset.checked_add(len)?;
        if end > self.len {
            return None;
        }
        self.offset.checked_add(offset)
    }

    /// Refuses `start..end`, absolute, when it touches a protected region.
    fn guard(start: u32, end: u32, running: Option<Slot>) -> Result<(), AccessError> {
        if is_protected(start, end, running) {
            Err(AccessError::Protected)
        } else {
            Ok(())
        }
    }

    /// Fills `buf` with the bytes at `offset`, in steps of at most
    /// [`CHUNK_LEN`] bytes, yielding after each full step. Nothing is read
    /// when the range doesn't lie inside the span.
    pub(crate) async fn read_async(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        let len = u32::try_from(buf.len()).map_err(|_| ReadError::OutOfRange)?;
        self.address(offset, len).ok_or(ReadError::OutOfRange)?;
        let mut at = offset;
        for chunk in buf.chunks_mut(CHUNK_LEN as usize) {
            // At most CHUNK_LEN.
            let chunk_len = u32::try_from(chunk.len()).map_err(|_| ReadError::OutOfRange)?;
            self.read(at, chunk)?;
            at = at.checked_add(chunk_len).ok_or(ReadError::OutOfRange)?;
            if chunk_len == CHUNK_LEN {
                Yield.pause().await;
            }
        }
        Ok(())
    }

    /// The SHA-256 digest of the `len` bytes at `offset`, in steps of at
    /// most [`CHUNK_LEN`] bytes, yielding after each full step.
    pub(crate) async fn sha256(&self, offset: u32, len: u32) -> Result<[u8; 32], ReadError> {
        let end = self
            .address(offset, len)
            .and(offset.checked_add(len))
            .ok_or(ReadError::OutOfRange)?;
        let mut sha = Sha256::new();
        let mut at = offset;
        while at < end {
            let step = (end - at).min(CHUNK_LEN);
            self.hash(&mut sha, at, step)?;
            at += step;
            if step == CHUNK_LEN {
                Yield.pause().await;
            }
        }
        Ok(sha.finalize().into())
    }

    /// Hashes the `len` bytes at `offset`, which lie inside the span, through
    /// a 256-byte buffer.
    fn hash(&self, sha: &mut Sha256, offset: u32, len: u32) -> Result<(), ReadError> {
        let mut buf = [0; HASH_BUF_LEN];
        let end = offset.checked_add(len).ok_or(ReadError::OutOfRange)?;
        let mut at = offset;
        while at < end {
            // At most HASH_BUF_LEN.
            let step = usize::try_from(end - at)
                .unwrap_or(HASH_BUF_LEN)
                .min(HASH_BUF_LEN);
            let chunk = buf.get_mut(..step).unwrap_or_default();
            self.read(at, chunk)?;
            sha.update(&*chunk);
            at += u32::try_from(step).map_err(|_| ReadError::OutOfRange)?;
        }
        Ok(())
    }

    /// Erases the sectors from `from` up to `to`, both sector boundaries, one
    /// sector at a time, yielding after each. Nothing is erased when the range
    /// doesn't lie inside the span, isn't aligned or touches a protected
    /// region while the app runs from `running`.
    pub(crate) async fn erase(
        &self,
        from: u32,
        to: u32,
        running: Option<Slot>,
    ) -> Result<(), AccessError> {
        if from > to || to > self.len {
            return Err(AccessError::OutOfRange);
        }
        let start = self
            .offset
            .checked_add(from)
            .ok_or(AccessError::OutOfRange)?;
        let end = self.offset.checked_add(to).ok_or(AccessError::OutOfRange)?;
        if !start.is_multiple_of(SECTOR_SIZE) || !end.is_multiple_of(SECTOR_SIZE) {
            return Err(AccessError::NotAligned);
        }
        Self::guard(start, end, running)?;
        let mut address = start;
        while address < end {
            let next = address
                .checked_add(SECTOR_SIZE)
                .ok_or(AccessError::OutOfRange)?;
            Self::guard(address, next, running)?;
            self.flash
                .erase_sector(address)
                .map_err(|_| AccessError::Device)?;
            address = next;
            Yield.pause().await;
        }
        Ok(())
    }

    /// Programs `bytes` at `offset`, in steps of at most [`CHUNK_LEN`] bytes,
    /// yielding after each. Nothing is programmed when the offset or the
    /// length isn't a multiple of [`WORD_SIZE`], the range doesn't lie inside
    /// the span or it touches a protected region while the app runs from
    /// `running`.
    pub(crate) async fn program(
        &self,
        offset: u32,
        bytes: &[u8],
        running: Option<Slot>,
    ) -> Result<(), AccessError> {
        let len = u32::try_from(bytes.len()).map_err(|_| AccessError::OutOfRange)?;
        if !offset.is_multiple_of(WORD_SIZE) || !len.is_multiple_of(WORD_SIZE) {
            return Err(AccessError::NotAligned);
        }
        let start = self.address(offset, len).ok_or(AccessError::OutOfRange)?;
        if !start.is_multiple_of(WORD_SIZE) {
            return Err(AccessError::NotAligned);
        }
        let end = start.checked_add(len).ok_or(AccessError::OutOfRange)?;
        Self::guard(start, end, running)?;
        let mut address = start;
        for chunk in bytes.chunks(CHUNK_LEN as usize) {
            // At most CHUNK_LEN.
            let chunk_len = u32::try_from(chunk.len()).map_err(|_| AccessError::OutOfRange)?;
            let next = address
                .checked_add(chunk_len)
                .ok_or(AccessError::OutOfRange)?;
            Self::guard(address, next, running)?;
            self.flash
                .program(address, chunk)
                .map_err(|_| AccessError::Device)?;
            address = next;
            Yield.pause().await;
        }
        Ok(())
    }
}

impl Source for Region<'_> {
    fn size(&self) -> u32 {
        self.len
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        let len = u32::try_from(buf.len()).map_err(|_| ReadError::OutOfRange)?;
        let address = self.address(offset, len).ok_or(ReadError::OutOfRange)?;
        self.flash.read(address, buf).map_err(|_| ReadError::Device)
    }
}

impl fmt::Debug for Region<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Region")
            .field("offset", &self.offset)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
