//! NOR flash: the device trait and its sizes.

/// Size of a NOR flash sector, the unit of erasure, in bytes.
pub const SECTOR_SIZE: u32 = 4096;

/// Alignment of the address and the length of a NOR flash program, in bytes.
pub const WORD_SIZE: u32 = 4;

/// The flash device failed an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceError;

/// A NOR flash, addressed from 0 up to its capacity: reads at any alignment,
/// erases sectors of [`SECTOR_SIZE`] bytes, programs words of [`WORD_SIZE`]
/// bytes, and programming only clears bits.
///
/// Each call is atomic with respect to every other call on the same device,
/// and acts as a compiler and hardware barrier: no memory access moves across
/// it. A power cut during an erase or a program may leave any subset of that
/// call's bits changed, and none of a later call's.
///
/// Every method takes `&self`, so the handles of one flash share it. The
/// crate calls a device only with ranges inside its capacity, aligned as each
/// method describes, and erases at most one sector or programs at most
/// [`SECTOR_SIZE`] bytes per call, so no call runs longer than one sector
/// erase.
///
/// The trait has no associated constants, so it is dyn-compatible: the crate
/// reaches a device as a `&dyn NorDevice`.
pub trait NorDevice: Sync {
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
    /// multiples of [`WORD_SIZE`]: each byte becomes the AND of what it held
    /// and what is programmed.
    ///
    /// # Errors
    ///
    /// [`DeviceError`] when the device fails the program.
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError>;
}
