//! Fixtures and builders shared by the tests of the chain: the formats
//! crate's, a flash in RAM that logs its writes, and the boards the flash
//! commands leave.

#![allow(
    clippy::cast_possible_truncation,
    reason = "the flash's addresses and lengths fit"
)]

extern crate alloc;

use alloc::boxed::Box;
use std::sync::Mutex;

pub(crate) use eveningstar_esp_formats::test_support::*;

/// Size of the board's flash in bytes.
pub(crate) const FLASH_LEN: usize = crate::layout::FLASH_SIZE as usize;

/// A flash in RAM with NOR semantics: an erase sets a sector to `0xFF`, a
/// program ANDs the bytes in. A call that breaks the [`NorDevice`] contract
/// (misaligned, past the end, more than a sector programmed at once) panics,
/// as it is a bug in the crate. Every erase and program is logged.
///
/// [`NorDevice`]: crate::nor::NorDevice
#[derive(Debug)]
pub(crate) struct RamFlash {
    bytes: Mutex<Vec<u8>>,
    /// Erases and programs carried out.
    ops: Mutex<Vec<Op>>,
}

/// An erase or program a [`RamFlash`] carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    Erase(u32),
    Program(u32, u32),
}

impl RamFlash {
    /// A flash holding `bytes`.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        assert!(u32::try_from(bytes.len()).is_ok());
        Self {
            bytes: Mutex::new(bytes),
            ops: Mutex::default(),
        }
    }

    /// An erased flash of `len` bytes with `parts` written at their offsets.
    pub(crate) fn with(len: usize, parts: &[(u32, &[u8])]) -> Self {
        Self::new(flash_with(len, parts))
    }

    /// The erases and programs carried out so far.
    pub(crate) fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    /// A copy of the bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }

    fn range(&self, address: u32, len: usize) -> core::ops::Range<usize> {
        let start = address as usize;
        let end = start.checked_add(len).expect("range overflows");
        assert!(
            end <= self.bytes.lock().unwrap().len(),
            "{start:#x}..{end:#x} is past the end of the flash"
        );
        start..end
    }
}

/// A [`RamFlash`] lent to [`crate::chain::split`], which takes its flash mutably, while
/// the test keeps reading it: the box is leaked, so the borrow lasts as long as
/// `flash` does.
pub(crate) fn lend(flash: &RamFlash) -> &mut dyn crate::nor::NorDevice {
    Box::leak(Box::new(Lent(flash)))
}

/// A shared [`RamFlash`] as a [`crate::nor::NorDevice`], for [`lend`].
struct Lent<'a>(&'a RamFlash);

impl crate::nor::NorDevice for Lent<'_> {
    fn capacity(&self) -> u32 {
        crate::nor::NorDevice::capacity(self.0)
    }
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), crate::DeviceError> {
        crate::nor::NorDevice::read(self.0, address, buf)
    }
    fn erase_sector(&self, address: u32) -> Result<(), crate::DeviceError> {
        crate::nor::NorDevice::erase_sector(self.0, address)
    }
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), crate::DeviceError> {
        crate::nor::NorDevice::program(self.0, address, bytes)
    }
}

impl crate::nor::NorDevice for RamFlash {
    fn capacity(&self) -> u32 {
        self.bytes.lock().unwrap().len() as u32
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), crate::DeviceError> {
        let range = self.range(address, buf.len());
        buf.copy_from_slice(&self.bytes.lock().unwrap()[range]);
        Ok(())
    }

    fn erase_sector(&self, address: u32) -> Result<(), crate::DeviceError> {
        assert!(
            address.is_multiple_of(crate::SECTOR_SIZE),
            "erase at {address:#x}"
        );
        let range = self.range(address, crate::SECTOR_SIZE as usize);
        self.ops.lock().unwrap().push(Op::Erase(address));
        self.bytes.lock().unwrap()[range].fill(0xFF);
        Ok(())
    }

    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), crate::DeviceError> {
        assert!(
            address.is_multiple_of(crate::WORD_SIZE) && bytes.len().is_multiple_of(4),
            "program of {} bytes at {address:#x}",
            bytes.len()
        );
        assert!(
            bytes.len() <= crate::SECTOR_SIZE as usize,
            "program of {} bytes at once",
            bytes.len()
        );
        let range = self.range(address, bytes.len());
        self.ops
            .lock()
            .unwrap()
            .push(Op::Program(address, bytes.len() as u32));
        for (old, new) in self.bytes.lock().unwrap()[range].iter_mut().zip(bytes) {
            *old &= new;
        }
        Ok(())
    }
}

/// A board flashed by `flash-all`: the Nix bootloader and table, the otadata
/// the bootloader writes on its first boot, and the app in `ota_0`.
pub(crate) fn nix_board() -> RamFlash {
    RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (crate::layout::TABLE_OFFSET, TABLE),
            (crate::layout::OTADATA.offset, OTADATA_FIRST_BOOT),
            (crate::layout::OTA_0.offset, APP),
        ],
    )
}

/// A board flashed by `cargo run`: espflash's bootloader and default table,
/// and the app in its `factory` partition at 0x10000.
pub(crate) fn espflash_board() -> RamFlash {
    RamFlash::with(
        FLASH_LEN,
        &[
            (0, ESPFLASH_BOOTLOADER),
            (0x8000, ESPFLASH_TABLE),
            (0x1_0000, APP),
        ],
    )
}
