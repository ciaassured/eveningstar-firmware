//! Test support: a NOR flash in RAM with a hook on its calls and power cuts,
//! the simulated ESP-IDF bootloader run on it, and readers and writers over
//! it made without `split`. The module allocates, and is built only for tests
//! and with the `test-support` feature.
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
use core::ops::Range;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use embassy_sync::blocking_mutex::CriticalSectionMutex;

pub(crate) use eveningstar_esp_formats::test_support::{OTADATA_LEN, boot, idf_loads};
pub use eveningstar_esp_formats::test_support::{Run, Tear, block_on, run};

use crate::chain::BootWriters;
use crate::nor::{self, NorDevice, SECTOR_SIZE, WORD_SIZE};
use crate::roles::{AlternateApp, Internal, Otadata, Role, Writable};
use crate::{DeviceError, Slot, SlotPartitions as _, Span, layout};

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

/// A power cut armed on a [`RamNor`]: the erase or program at index `at` of
/// its log of writes fails, and every later one, the first of them landing as
/// `tear` says when it is a program.
#[derive(Clone, Copy, Debug)]
struct Cut {
    at: usize,
    tear: Option<Tear>,
}

/// A NOR flash in RAM: an erase sets a sector to `0xFF`, a program ANDs the
/// bytes in, and bytes never written read as `0xFF`. Only the sectors written
/// take memory.
///
/// Every call is logged ([`calls`](Self::calls)), the erases and programs
/// also on their own ([`writes`](Self::writes)), and a hook can run before
/// any of them ([`before_call`](Self::before_call)), which stands for a task
/// that runs between two calls. Erases and programs fail on request, changing
/// nothing ([`fail_writes`](Self::fail_writes),
/// [`fail_after`](Self::fail_after)), or as a power cut leaves them
/// ([`cut_after`](Self::cut_after)). A call that breaks the [`NorDevice`]
/// contract (misaligned, past the capacity, more than a sector programmed)
/// panics, as it is a bug in the caller.
pub struct RamNor {
    capacity: u32,
    sectors: CriticalSectionMutex<RefCell<BTreeMap<u32, Vec<u8>>>>,
    calls: CriticalSectionMutex<RefCell<Vec<Call>>>,
    writes: CriticalSectionMutex<RefCell<Vec<Call>>>,
    hooks: CriticalSectionMutex<RefCell<Vec<(usize, Hook)>>>,
    /// The number of calls received.
    count: AtomicUsize,
    /// Whether erases and programs fail, changing nothing.
    failing: AtomicBool,
    /// The power cut armed, if any.
    cut: CriticalSectionMutex<RefCell<Option<Cut>>>,
}

impl RamNor {
    /// An erased flash of `capacity` bytes.
    pub fn erased(capacity: u32) -> Self {
        Self {
            capacity,
            sectors: CriticalSectionMutex::new(RefCell::new(BTreeMap::new())),
            calls: CriticalSectionMutex::new(RefCell::new(Vec::new())),
            writes: CriticalSectionMutex::new(RefCell::new(Vec::new())),
            hooks: CriticalSectionMutex::new(RefCell::new(Vec::new())),
            count: AtomicUsize::new(0),
            failing: AtomicBool::new(false),
            cut: CriticalSectionMutex::new(RefCell::new(None)),
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

    /// A flash of the same capacity holding the same bytes, with nothing
    /// logged, hooked or failing.
    #[must_use]
    pub fn copy(&self) -> Self {
        let copy = Self::erased(self.capacity);
        let sectors = self.sectors.lock(|sectors| sectors.borrow().clone());
        copy.sectors.lock(|to| *to.borrow_mut() = sectors);
        copy
    }

    /// The flash as a device [`split`](crate::split) can take mutably, while
    /// the caller keeps reading, hooking and cutting it. The adapter is
    /// leaked, so it lives as long as the flash.
    pub fn lend(&self) -> &mut dyn NorDevice {
        Box::leak(Box::new(Lent(self)))
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

    /// Makes every erase and program fail without changing a bit once `n`
    /// more have been received.
    pub fn fail_after(&self, n: usize) {
        self.arm(n, None);
    }

    /// Cuts the power once `n` more erases and programs have been received:
    /// the next one fails and every later one, and when the first to fail is
    /// a program, it lands as `tear` says all the same. An erase cut lands
    /// not at all.
    pub fn cut_after(&self, n: usize, tear: Tear) {
        self.arm(n, Some(tear));
    }

    /// Arms the failure of the erases and programs from the `n`-th from now.
    fn arm(&self, n: usize, tear: Option<Tear>) {
        let at = self.writes.lock(|writes| writes.borrow().len()) + n;
        self.cut
            .lock(|cut| *cut.borrow_mut() = Some(Cut { at, tear }));
    }

    /// Lets every erase and program succeed again, after a cut, a
    /// [`fail_after`](Self::fail_after) or a
    /// [`fail_writes`](Self::fail_writes).
    pub fn heal(&self) {
        self.cut.lock(|cut| *cut.borrow_mut() = None);
        self.failing.store(false, Ordering::SeqCst);
    }

    /// The calls received so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock(|calls| calls.borrow().clone())
    }

    /// The erases and programs received so far, in order, those that failed
    /// included.
    pub fn writes(&self) -> Vec<Call> {
        self.writes.lock(|writes| writes.borrow().clone())
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
            for (at, piece) in Self::pieces(address, bytes.len()) {
                let index = Self::index(at);
                Self::sector(&mut sectors, at)[index..index + piece.len()]
                    .copy_from_slice(&bytes[piece]);
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

    /// The pieces of the `len` bytes at `address`, which lie inside the
    /// capacity, that each lie in one sector: the address of each, and its
    /// range in the bytes.
    fn pieces(address: u32, len: usize) -> impl Iterator<Item = (u32, Range<usize>)> {
        let mut done = 0;
        core::iter::from_fn(move || {
            if done == len {
                return None;
            }
            let at = address + u32::try_from(done).expect("the bytes lie inside the capacity");
            let room = SECTOR_SIZE as usize - Self::index(at);
            let piece = done..len.min(done + room);
            done = piece.end;
            Some((at, piece))
        })
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
            for (at, piece) in Self::pieces(address, buf.len()) {
                let to = &mut buf[piece];
                match sectors.get(&(at / SECTOR_SIZE)) {
                    Some(sector) => {
                        let index = Self::index(at);
                        to.copy_from_slice(&sector[index..index + to.len()]);
                    }
                    None => to.fill(0xFF),
                }
            }
        });
    }

    /// The end of the last sector of `span` that holds bytes written, from
    /// the start of the flash, or the span's offset when none does: past it,
    /// the span reads erased.
    fn written_end(&self, span: Span) -> u32 {
        let first = span.offset / SECTOR_SIZE;
        let last = (span.offset + span.len).div_ceil(SECTOR_SIZE);
        self.sectors.lock(|sectors| {
            sectors
                .borrow()
                .range(first..last)
                .next_back()
                .map_or(span.offset, |(sector, _)| (sector + 1) * SECTOR_SIZE)
        })
    }

    /// Whether the erase or program logged last fails, and the tear it
    /// lands with: the armed cut's, which the first program that fails
    /// takes.
    fn fails(&self, program: bool) -> (bool, Option<Tear>) {
        if self.failing.load(Ordering::SeqCst) {
            return (true, None);
        }
        let index = self.writes.lock(|writes| writes.borrow().len()) - 1;
        self.cut.lock(|cut| match cut.borrow_mut().as_mut() {
            Some(cut) if cut.at <= index => {
                let tear = cut.tear.take();
                (true, tear.filter(|_| program))
            }
            _ => (false, None),
        })
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
        if !matches!(call, Call::Read(..)) {
            self.writes.lock(|writes| writes.borrow_mut().push(call));
        }
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
        if self.fails(false).0 {
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
        let (fails, tear) = self.fails(true);
        if fails {
            if let Some(tear) = tear {
                let mut landed = self.at(address, bytes.len());
                tear.land(&mut landed, bytes);
                self.poke(address, &landed);
            }
            return Err(DeviceError);
        }
        self.sectors.lock(|sectors| {
            let mut sectors = sectors.borrow_mut();
            for (at, piece) in Self::pieces(address, bytes.len()) {
                let index = Self::index(at);
                let sector = &mut Self::sector(&mut sectors, at)[index..index + piece.len()];
                for (old, new) in sector.iter_mut().zip(&bytes[piece]) {
                    *old &= *new;
                }
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

/// A shared [`RamNor`] as a device, for [`RamNor::lend`].
struct Lent<'a>(&'a RamNor);

impl NorDevice for Lent<'_> {
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

/// One boot of `device`, a flash holding the compiled layout, by the
/// simulated ESP-IDF bootloader
/// ([`boot`](eveningstar_esp_formats::test_support::boot)), which loads a
/// slot when [`idf_loads`](eveningstar_esp_formats::test_support::idf_loads)
/// says so and writes the otadata back, without logging, as the bootloader
/// does. Returns the slot started, `None` when none loads.
///
/// Each slot is judged on its bytes up to a sector past the last of its
/// sectors that holds bytes written, as it reads erased beyond.
pub fn boot_device(device: &RamNor) -> Option<Slot> {
    let loadable = Slot::ALL.map(|slot| {
        let span = Span::of(&slot.partition());
        let end = (device.written_end(span) + SECTOR_SIZE).min(span.offset + span.len);
        idf_loads(&device.at(span.offset, (end - span.offset) as usize))
    });
    let mut otadata = device.at(layout::OTADATA.offset, OTADATA_LEN);
    let booted = boot(&mut otadata, loadable);
    device.poke(layout::OTADATA.offset, &otadata);
    booted
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

    /// Boot writers for the app running from `running`, of any spans:
    /// `alternate`, absolute, as the alternate slot, and `otadata` as the
    /// otadata. Panics as [`writer`](Self::writer) does.
    pub fn boot_writers(
        &self,
        alternate: Span,
        otadata: Span,
        running: Slot,
    ) -> BootWriters<'static> {
        BootWriters::new(
            self.writer::<AlternateApp>(alternate, running),
            self.writer::<Otadata>(otadata, running),
            running,
        )
    }
}

#[cfg(test)]
mod tests;
