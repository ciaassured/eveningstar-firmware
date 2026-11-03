//! Fixtures and builders shared by the tests of `split`, the partitions and
//! `Ota`: the formats crate's, a flash in RAM that fails and tears its writes
//! on request, and the boards the flash commands leave.

#![allow(
    clippy::cast_possible_truncation,
    reason = "the flash's addresses and lengths fit"
)]

extern crate alloc;

use alloc::boxed::Box;
use std::sync::Mutex;

pub(crate) use eveningstar_esp_formats::test_support::*;

use crate::SlotPartitions as _;

/// Size of the board's flash in bytes.
pub(crate) const FLASH_LEN: usize = crate::layout::FLASH_SIZE as usize;

/// A flash in RAM with NOR semantics: an erase sets a sector to `0xFF`, a
/// program ANDs the bytes in. A call that breaks the [`NorDevice`] contract
/// (misaligned, past the end, more than a sector programmed at once) panics,
/// as it is a bug in the crate. Every erase and program is logged, and they
/// can be made to fail after a number of them, the first that fails landing
/// in part as a power cut leaves it.
///
/// [`NorDevice`]: crate::nor::NorDevice
#[derive(Debug)]
pub(crate) struct RamFlash {
    bytes: Mutex<Vec<u8>>,
    /// Erases and programs that succeeded, as (address, length, is erase).
    ops: Mutex<Vec<Op>>,
    /// The number of erases and programs that succeed before every later one
    /// fails.
    fail_after: Mutex<Option<usize>>,
    /// How the first program that fails lands.
    tear: Mutex<Option<Tear>>,
    /// Reads that touch this range fail.
    fail_reads: Mutex<Option<core::ops::Range<u32>>>,
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
            fail_after: Mutex::default(),
            tear: Mutex::default(),
            fail_reads: Mutex::default(),
        }
    }

    /// An erased flash of `len` bytes with `parts` written at their offsets.
    pub(crate) fn with(len: usize, parts: &[(u32, &[u8])]) -> Self {
        Self::new(flash_with(len, parts))
    }

    /// Makes every erase and program fail once `n` more have succeeded.
    pub(crate) fn fail_after(&self, n: usize) {
        let done = self.ops.lock().unwrap().len();
        *self.fail_after.lock().unwrap() = Some(done + n);
    }

    /// Cuts the power once `n` more erases and programs have succeeded: every
    /// later one fails, and when the first to fail is a program, it lands as
    /// `tear` says all the same.
    pub(crate) fn cut_after(&self, n: usize, tear: Tear) {
        self.fail_after(n);
        *self.tear.lock().unwrap() = Some(tear);
    }

    /// Makes every read that touches `range` fail.
    pub(crate) fn fail_reads(&self, range: core::ops::Range<u32>) {
        *self.fail_reads.lock().unwrap() = Some(range);
    }

    /// Lets every erase, program and read succeed again.
    pub(crate) fn heal(&self) {
        *self.fail_after.lock().unwrap() = None;
        *self.tear.lock().unwrap() = None;
        *self.fail_reads.lock().unwrap() = None;
    }

    /// Writes `bytes` at `address` as they are, without logging them or
    /// failing: what the bootloader writes, or what decays on the flash.
    pub(crate) fn poke(&self, address: u32, bytes: &[u8]) {
        let range = self.range(address, bytes.len());
        self.bytes.lock().unwrap()[range].copy_from_slice(bytes);
    }

    /// What `read` returns given the bytes, without copying them.
    pub(crate) fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        read(&self.bytes.lock().unwrap())
    }

    /// The erases and programs carried out so far.
    pub(crate) fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    /// A copy of the bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }

    /// A copy of the `len` bytes at `address`.
    pub(crate) fn at(&self, address: u32, len: usize) -> Vec<u8> {
        let at = address as usize;
        self.bytes.lock().unwrap()[at..at + len].to_vec()
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

    /// Logs `op`, or fails it when the failure point has been reached.
    fn record(&self, op: Op) -> Result<(), crate::DeviceError> {
        let mut ops = self.ops.lock().unwrap();
        if self
            .fail_after
            .lock()
            .unwrap()
            .is_some_and(|n| ops.len() >= n)
        {
            return Err(crate::DeviceError);
        }
        ops.push(op);
        Ok(())
    }
}

/// A [`RamFlash`] lent to [`crate::split`], which takes its flash mutably, while
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
        let fails = self.fail_reads.lock().unwrap().clone();
        if let Some(fails) = fails
            && (range.start as u32) < fails.end
            && fails.start < range.end as u32
        {
            return Err(crate::DeviceError);
        }
        buf.copy_from_slice(&self.bytes.lock().unwrap()[range]);
        Ok(())
    }

    fn erase_sector(&self, address: u32) -> Result<(), crate::DeviceError> {
        assert!(
            address.is_multiple_of(crate::SECTOR_SIZE),
            "erase at {address:#x}"
        );
        let range = self.range(address, crate::SECTOR_SIZE as usize);
        if let Err(error) = self.record(Op::Erase(address)) {
            // A cut erase lands whole or not at all here.
            *self.tear.lock().unwrap() = None;
            return Err(error);
        }
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
        let recorded = self.record(Op::Program(address, bytes.len() as u32));
        let mut flash = self.bytes.lock().unwrap();
        if recorded.is_err() {
            if let Some(tear) = self.tear.lock().unwrap().take() {
                tear.land(&mut flash[range], bytes);
            }
            return recorded;
        }
        for (old, new) in flash[range].iter_mut().zip(bytes) {
            *old &= new;
        }
        Ok(())
    }
}

/// The otadata partition's offset.
pub(crate) const OTADATA_AT: u32 = crate::layout::OTADATA.offset;

/// A board flashed by `flash-all`: the Nix bootloader and table, the otadata
/// the bootloader writes on its first boot, and the app in `ota_0`.
pub(crate) fn nix_board() -> RamFlash {
    RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (crate::layout::TABLE_OFFSET, TABLE),
            (OTADATA_AT, OTADATA_FIRST_BOOT),
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

/// One boot of `flash` by the simulated ESP-IDF bootloader ([`boot`]), which
/// loads a slot when [`idf_loads`] says so and writes the otadata back
/// unlogged: the slot started, `None` when none loads.
pub(crate) fn boot_flash(flash: &RamFlash) -> Option<crate::Slot> {
    let loadable = crate::Slot::ALL.map(|slot| {
        let entry = slot.partition();
        flash.with_bytes(|bytes| idf_loads(&bytes[entry.offset as usize..entry.end() as usize]))
    });
    let mut otadata = flash.at(OTADATA_AT, OTADATA_LEN);
    let booted = boot(&mut otadata, loadable);
    flash.poke(OTADATA_AT, &otadata);
    booted
}
