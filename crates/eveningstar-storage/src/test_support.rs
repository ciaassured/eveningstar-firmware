//! Test support: a NOR flash in RAM with a hook on its calls, and readers and
//! writers over it made without `split`. The module allocates, and is built
//! only for tests and with the `test-support` feature.
//!
//! The RAM device and the state table lock through `critical-section`, so a
//! host test needs an implementation of it, such as `critical-section`'s
//! `std` feature.
//!
//! ```
//! use eveningstar_storage::Span;
//! use eveningstar_storage::roles::AlternateApp;
//! use eveningstar_storage::test_support::{RamNor, TestFlash, run};
//! use eveningstar_storage::{Slot, layout};
//!
//! let flash = TestFlash::new(RamNor::erased(layout::FLASH_SIZE));
//! let span = Span::from(layout::OTA_1);
//! let mut writer = flash.writer::<AlternateApp>(span, Slot::Ota0);
//! run(writer.begin().program(0, &[0; 4])).unwrap();
//! assert_eq!(flash.device().at(span.offset, 5), [0, 0, 0, 0, 0xFF]);
//! ```

#![allow(
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    reason = "test support panics on misuse, and its getters are plain"
)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use embassy_sync::blocking_mutex::CriticalSectionMutex;

pub use eveningstar_esp_formats::test_support::{Run, block_on, run};

use crate::nor::{self, NorDevice, SECTOR_SIZE, WORD_SIZE};
use crate::roles::{Internal, Role, Writable};
use crate::{DeviceError, Slot, Span};

#[cfg(test)]
mod board;
#[cfg(test)]
pub(crate) use board::*;

/// A call a [`RamNor`] received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// A read of the given length at the given address.
    Read(u32, u32),
    /// An erase of the sector at the given address.
    Erase(u32),
    /// A program of the given length at the given address.
    Program(u32, u32),
}

/// A hook run before a call.
type Hook = Box<dyn FnOnce() + Send>;

/// A NOR flash in RAM: an erase sets a sector to `0xFF`, a program ANDs the
/// bytes in, and bytes never written read as `0xFF`. Only the sectors written
/// take memory.
///
/// Every call is logged ([`calls`](Self::calls)), and a hook can run before
/// any of them ([`before_call`](Self::before_call)), which stands for a task
/// that runs between two calls. A call that breaks the [`NorDevice`] contract
/// (misaligned, past the capacity, more than a sector programmed) panics, as
/// it is a bug in the caller.
pub struct RamNor {
    capacity: u32,
    sectors: CriticalSectionMutex<RefCell<BTreeMap<u32, Vec<u8>>>>,
    calls: CriticalSectionMutex<RefCell<Vec<Call>>>,
    hooks: CriticalSectionMutex<RefCell<Vec<(usize, Hook)>>>,
    /// The number of calls received.
    count: AtomicUsize,
    /// Whether erases and programs fail, changing nothing.
    failing: AtomicBool,
}

impl RamNor {
    /// An erased flash of `capacity` bytes.
    pub fn erased(capacity: u32) -> Self {
        Self {
            capacity,
            sectors: CriticalSectionMutex::new(RefCell::new(BTreeMap::new())),
            calls: CriticalSectionMutex::new(RefCell::new(Vec::new())),
            hooks: CriticalSectionMutex::new(RefCell::new(Vec::new())),
            count: AtomicUsize::new(0),
            failing: AtomicBool::new(false),
        }
    }

    /// A flash of `capacity` bytes holding `parts` at their addresses and
    /// `0xFF` elsewhere.
    pub fn with(capacity: u32, parts: &[(u32, &[u8])]) -> Self {
        let flash = Self::erased(capacity);
        for (address, bytes) in parts {
            flash.poke(*address, bytes);
        }
        flash
    }

    /// Runs `hook` before the call that is the `n`-th from now, counting
    /// from 0 for the next one, and before that call takes effect. The calls
    /// the hook makes are counted too.
    pub fn before_call(&self, n: usize, hook: impl FnOnce() + Send + 'static) {
        let at = self.count.load(Ordering::SeqCst) + n;
        self.hooks
            .lock(|hooks| hooks.borrow_mut().push((at, Box::new(hook))));
    }

    /// Makes every later erase and program fail without changing a bit, or
    /// succeed again.
    pub fn fail_writes(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    /// The calls received so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock(|calls| calls.borrow().clone())
    }

    /// The erases and programs received so far, in order.
    pub fn writes(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|call| !matches!(call, Call::Read(..)))
            .collect()
    }

    /// A copy of the `len` bytes at `address`.
    pub fn at(&self, address: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        self.copy_out(address, &mut bytes);
        bytes
    }

    /// Writes `bytes` at `address` as they are, without logging or failing:
    /// what another writer of the chip leaves, such as esptool.
    pub fn poke(&self, address: u32, bytes: &[u8]) {
        self.check(address, bytes.len());
        self.sectors.lock(|sectors| {
            let mut sectors = sectors.borrow_mut();
            for (at, byte) in (address..).zip(bytes) {
                Self::sector(&mut sectors, at)[Self::index(at)] = *byte;
            }
        });
    }

    /// Panics unless the `len` bytes at `address` lie inside the capacity.
    fn check(&self, address: u32, len: usize) {
        let end = u32::try_from(len)
            .ok()
            .and_then(|len| address.checked_add(len));
        assert!(
            end.is_some_and(|end| end <= self.capacity),
            "{len} bytes at {address:#x} run past the capacity {:#x}",
            self.capacity
        );
    }

    /// The offset of `address` in its sector.
    fn index(address: u32) -> usize {
        (address % SECTOR_SIZE) as usize
    }

    /// The sector holding `address`, erased when it was never written.
    fn sector(sectors: &mut BTreeMap<u32, Vec<u8>>, address: u32) -> &mut Vec<u8> {
        sectors
            .entry(address / SECTOR_SIZE)
            .or_insert_with(|| vec![0xFF; SECTOR_SIZE as usize])
    }

    fn copy_out(&self, address: u32, buf: &mut [u8]) {
        self.check(address, buf.len());
        self.sectors.lock(|sectors| {
            let sectors = sectors.borrow();
            for (at, byte) in (address..).zip(buf.iter_mut()) {
                *byte = sectors
                    .get(&(at / SECTOR_SIZE))
                    .map_or(0xFF, |sector| sector[Self::index(at)]);
            }
        });
    }

    /// Counts and logs `call`, after running the hooks due before it.
    fn receive(&self, call: Call) {
        let index = self.count.fetch_add(1, Ordering::SeqCst);
        let due: Vec<Hook> = self.hooks.lock(|hooks| {
            let mut hooks = hooks.borrow_mut();
            let (due, later) = hooks.drain(..).partition(|(at, _)| *at == index);
            *hooks = later;
            due.into_iter().map(|(_, hook)| hook).collect()
        });
        // Outside every lock: a hook makes calls of its own.
        for hook in due {
            hook();
        }
        self.calls.lock(|calls| calls.borrow_mut().push(call));
    }
}

impl NorDevice for RamNor {
    fn capacity(&self) -> u32 {
        self.capacity
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        let len = u32::try_from(buf.len()).expect("read length fits");
        self.receive(Call::Read(address, len));
        self.copy_out(address, buf);
        Ok(())
    }

    fn erase_sector(&self, address: u32) -> Result<(), DeviceError> {
        assert!(address.is_multiple_of(SECTOR_SIZE), "erase at {address:#x}");
        self.check(address, SECTOR_SIZE as usize);
        self.receive(Call::Erase(address));
        if self.failing.load(Ordering::SeqCst) {
            return Err(DeviceError);
        }
        self.sectors.lock(|sectors| {
            sectors.borrow_mut().remove(&(address / SECTOR_SIZE));
        });
        Ok(())
    }

    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError> {
        let len = u32::try_from(bytes.len()).expect("program length fits");
        assert!(
            address.is_multiple_of(WORD_SIZE) && len.is_multiple_of(WORD_SIZE),
            "program of {len} bytes at {address:#x}"
        );
        assert!(len <= SECTOR_SIZE, "program of {len} bytes in one call");
        self.check(address, bytes.len());
        self.receive(Call::Program(address, len));
        if self.failing.load(Ordering::SeqCst) {
            return Err(DeviceError);
        }
        self.sectors.lock(|sectors| {
            let mut sectors = sectors.borrow_mut();
            for (at, byte) in (address..).zip(bytes) {
                Self::sector(&mut sectors, at)[Self::index(at)] &= *byte;
            }
        });
        Ok(())
    }
}

impl fmt::Debug for RamNor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RamNor")
            .field("capacity", &self.capacity)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// A [`RamNor`] and a state table, both leaked so the handles made over them
/// are `'static` and can move into a hook.
///
/// It makes any number of readers and writers, of any span: the uniqueness
/// `split` gives is up to the test.
#[derive(Clone, Copy, Debug)]
pub struct TestFlash {
    device: &'static RamNor,
    state: &'static nor::State,
}

impl TestFlash {
    /// The handles' flash: `device`, with a new state table.
    pub fn new(device: RamNor) -> Self {
        Self {
            device: Box::leak(Box::new(device)),
            state: Box::leak(Box::new(nor::State::new())),
        }
    }

    /// The device.
    pub fn device(&self) -> &'static RamNor {
        self.device
    }

    /// A reader of `span`, absolute, with role `R`. Panics unless the span
    /// lies inside the device.
    pub fn reader<R: Role<Device = Internal>>(&self, span: Span) -> nor::Reader<'static, R> {
        nor::Reader::new(self.device, self.state, span).expect("the span lies inside the device")
    }

    /// A writer of `span`, absolute, with role `R`, while the app runs from
    /// `running`. Panics unless the span lies inside the device and inside
    /// one physical span of the compiled layout.
    pub fn writer<R: Writable<Device = Internal>>(
        &self,
        span: Span,
        running: Slot,
    ) -> nor::Writer<'static, R> {
        nor::Writer::new(self.device, self.state, span, running)
            .expect("the span lies inside the device and one physical span")
    }
}
