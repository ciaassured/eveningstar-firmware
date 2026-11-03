//! Fixtures shared by the crate's tests: boards in RAM split by the storage
//! crate, and a pace that counts.

extern crate alloc;

use alloc::boxed::Box;
use core::future::Future;

use eveningstar_esp_formats::pace::Pace;
use eveningstar_esp_formats::test_support::{APP, BOOTLOADER, OTADATA_FIRST_BOOT, TABLE};
use eveningstar_storage::nor::{NorDevice, State};
use eveningstar_storage::test_support::{RamNor, run};
use eveningstar_storage::{DeviceError, Storage, layout, split};
use sha2::{Digest as _, Sha256};

/// A pace that counts its pauses and never hands control back.
#[derive(Debug, Default)]
pub(crate) struct Counting {
    pub(crate) pauses: usize,
}

impl Pace for Counting {
    fn pause(&mut self) -> impl Future<Output = ()> {
        self.pauses += 1;
        core::future::ready(())
    }
}

/// The SHA-256 of `bytes`, from `sha2` directly.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A shared [`RamNor`] as a device `split` can take mutably, while the test
/// keeps reading and hooking it.
struct Lent(&'static RamNor);

impl NorDevice for Lent {
    fn capacity(&self) -> u32 {
        self.0.capacity()
    }
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        self.0.read(address, buf)
    }
    fn erase_sector(&self, address: u32) -> Result<(), DeviceError> {
        self.0.erase_sector(address)
    }
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError> {
        self.0.program(address, bytes)
    }
}

/// `device`, split for the app booted at `booted`, with the device kept for
/// the test. Both are leaked, so the handles are `'static`.
pub(crate) fn split_board(device: RamNor, booted: u32) -> (Storage<'static>, &'static RamNor) {
    let device: &'static RamNor = Box::leak(Box::new(device));
    let lent: &'static mut dyn NorDevice = Box::leak(Box::new(Lent(device)));
    let state = Box::leak(Box::new(State::new()));
    (run(split(lent, state, booted)), device)
}

/// A board flashed by `flash-all`: the Nix bootloader and table, the otadata
/// the bootloader writes on its first boot, and the app in `ota_0`.
pub(crate) fn nix_board() -> RamNor {
    RamNor::with(
        layout::FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (layout::TABLE_OFFSET, TABLE),
            (layout::OTADATA.offset, OTADATA_FIRST_BOOT),
            (layout::OTA_0.offset, APP),
        ],
    )
}
