//! The flash chip, as the crate reaches it.

/// Size of a flash sector, the unit of erasure, in bytes.
pub const SECTOR_SIZE: u32 = 4096;

/// Alignment of the address and the length of a program, in bytes.
pub const WORD_SIZE: u32 = 4;

/// The flash device failed an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceError;

/// The internal NOR flash, addressed from 0 up to its capacity.
///
/// Every method takes `&self`, so the handles of one flash share it; an
/// implementation serializes the operations itself. The crate calls
/// [`erase_sector`](Self::erase_sector) and [`program`](Self::program) only
/// with ranges inside the capacity, aligned as each method describes, and
/// programs at most one sector's worth of bytes per call, so no single call
/// runs longer than one sector erase.
pub trait Flash: Sync {
    /// Size of the flash in bytes.
    fn capacity(&self) -> u32;

    /// Fills `buf` with the bytes starting at `address`, at any alignment.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when the device fails the read or the range doesn't lie
    /// inside the flash.
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError>;

    /// Erases the sector starting at `address`, a multiple of
    /// [`SECTOR_SIZE`], setting each of its bytes to `0xFF`.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when the device fails the erase.
    fn erase_sector(&self, address: u32) -> Result<(), DeviceError>;

    /// Programs `bytes` at `address`, both the address and the length
    /// multiples of [`WORD_SIZE`]. NOR flash only clears bits: each byte
    /// becomes the AND of what it held and what is programmed.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when the device fails the program.
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError>;
}
