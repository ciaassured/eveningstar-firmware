//! Downloads of the public regions: one at a time, supervised, and cut short
//! when the region is written.
//!
//! [`StorageView::content`] decides whether a region's bytes may leave the
//! device from its name alone, before any handle is looked up: only a
//! [`Contents::Public`] region does. It then takes the region's handle, a
//! public one, and the download lock. A [`Content`] reads through
//! [`Handle`], whose variants are the public handles and whose every read
//! goes through a function that takes `impl Read` only, so a handle to a
//! configuration or diagnostics can't be added to it.
//!
//! The lock holds the download watchdog handle, as there is one download at a
//! time. Taking the lock feeds it, each chunk read feeds it (a response reads
//! a chunk once the previous one is written, so the feed also marks that
//! write), and releasing the lock, when the [`Content`] is dropped, sets it
//! idle. It feeds on progress,
//! never on a timer, so a download that hangs while the rest of the system
//! runs is caught.
//!
//! A content notes its region's write generation when it is made, and each
//! read checks it after reading: a read after a write to the region fails,
//! so no read returns bytes of two contents.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::Duration;
use eveningstar_storage::partitions::{
    Bootloader, Otadata, OtherApp, Partition, PartitionTable, Read, RunningApp,
};
use eveningstar_storage::{ReadError, Slot};

use super::regions::{Contents, RegionName};
use super::{AppRole, Image, Service, StorageView};
use crate::watchdog::WatchdogHandle;
use crate::wd;

/// Task watchdog timeout of a download. A download feeds on every chunk it
/// reads, and a client that stops reading is ended by the HTTP server's
/// write timeout well within it, so only a hang in the firmware misses it.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

/// The download lock, with the watchdog handle that supervises the download
/// holding it.
pub(super) struct Lock {
    taken: AtomicBool,
    wd: WatchdogHandle<wd::StorageDownload>,
}

impl core::fmt::Debug for Lock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Lock")
            .field("taken", &self.taken)
            .finish_non_exhaustive()
    }
}

impl Lock {
    /// The lock, free, with its watchdog handle idle until a download takes
    /// it.
    pub(super) fn new(wd: WatchdogHandle<wd::StorageDownload>) -> Self {
        wd.idle();
        Self {
            taken: AtomicBool::new(false),
            wd,
        }
    }

    /// Takes the lock and feeds its watchdog handle, or `None` while another
    /// download holds it.
    fn take(&'static self) -> Option<Held> {
        if self.taken.swap(true, Ordering::Acquire) {
            return None;
        }
        self.wd.feed();
        Some(Held(self))
    }
}

/// The download lock, held until dropped.
#[derive(Debug)]
struct Held(&'static Lock);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.wd.idle();
        self.0.taken.store(false, Ordering::Release);
    }
}

/// Why a region's content isn't given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub(crate) enum ContentError {
    /// The region isn't public: its contents never leave the device.
    Sensitive,
    /// The region is public, and the firmware holds no handle for it this
    /// boot.
    Unavailable,
    /// Another download holds the lock.
    Busy,
}

/// Why a read of a region's content failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentReadError {
    /// The flash failed the read, or the range doesn't lie inside the region.
    Read(ReadError),
    /// The region was written since the content was made.
    Written,
}

impl defmt::Format for ContentReadError {
    fn format(&self, f: defmt::Formatter<'_>) {
        match self {
            Self::Read(ReadError::OutOfRange) => defmt::write!(f, "read out of range"),
            Self::Read(ReadError::Device) => defmt::write!(f, "flash read failed"),
            Self::Written => defmt::write!(f, "the region was written"),
        }
    }
}

/// What a part of a region holds, as far as the firmware knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// The whole region, holding nothing the firmware has a current digest
    /// of.
    Whole,
    /// Exactly the image the region holds, with the SHA-256 of the whole
    /// image.
    Image([u8; 32]),
    /// The whole app slot, holding the image with this SHA-256 and the bytes
    /// after it.
    SlotOf([u8; 32]),
    /// The bytes from `first` to `last`, both included.
    Range {
        /// Offset of the first byte in the region.
        first: u32,
        /// Offset of the last byte in the region.
        last: u32,
    },
}

/// A public handle. Every variant's handle implements [`Read`], and every
/// read of one goes through [`read`].
#[derive(Clone, Copy, Debug)]
enum Handle {
    Bootloader(Partition<'static, Bootloader>),
    PartitionTable(Partition<'static, PartitionTable>),
    Otadata(Partition<'static, Otadata>),
    RunningApp(Partition<'static, RunningApp>),
    OtherApp(Partition<'static, OtherApp>),
}

/// Evaluates `$body` with `$handle` bound to the handle `$value` holds,
/// whatever its role.
macro_rules! each {
    ($value:expr, $handle:ident => $body:expr) => {
        match $value {
            Handle::Bootloader($handle) => $body,
            Handle::PartitionTable($handle) => $body,
            Handle::Otadata($handle) => $body,
            Handle::RunningApp($handle) => $body,
            Handle::OtherApp($handle) => $body,
        }
    };
}

impl Handle {
    fn offset(&self) -> u32 {
        each!(self, handle => span(handle).0)
    }

    fn len(&self) -> u32 {
        each!(self, handle => span(handle).1)
    }

    async fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        each!(self, handle => read(handle, offset, buf).await)
    }
}

/// The offset and length of `handle`, a public handle.
fn span<P: Read>(handle: &P) -> (u32, u32) {
    (handle.offset(), handle.len())
}

/// Reads the bytes at `offset` in `handle` into `buf`. Takes only a public
/// handle: the one route from a handle to a download.
async fn read<P: Read>(handle: &P, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
    handle.read(offset, buf).await
}

/// The content of a public region, for one download: its handle, its write
/// generation and what it holds when the download started, and the download
/// lock, held until it is dropped.
#[derive(Debug)]
pub(crate) struct Content {
    view: StorageView,
    name: RegionName,
    handle: Handle,
    generation: u32,
    /// The length and SHA-256 of the image in the region, when the firmware
    /// has a current digest of it.
    image: Option<(u32, [u8; 32])>,
    _held: Held,
}

impl StorageView {
    /// The content of the region `name`, for a download, holding the
    /// download lock until it is dropped.
    ///
    /// # Errors
    ///
    /// [`ContentError::Sensitive`] when the region isn't public, decided from
    /// its name before anything else; [`ContentError::Unavailable`] when the
    /// firmware holds no handle for it this boot; [`ContentError::Busy`] when
    /// another download holds the lock.
    pub(crate) fn content(self, name: RegionName) -> Result<Content, ContentError> {
        if name.contents() != Contents::Public {
            return Err(ContentError::Sensitive);
        }
        let handle = self.0.handle(name).ok_or(ContentError::Unavailable)?;
        let held = self.0.download.take().ok_or(ContentError::Busy)?;
        let generation = self.generations().of(handle.offset(), handle.len());
        let image = match name {
            RegionName::Bootloader => self
                .report()
                .bootloader
                .ok()
                .map(|bootloader| (bootloader.size, bootloader.file_sha256)),
            RegionName::App(role) => self
                .image(role)
                .filter(|check| check.generation == generation)
                .and_then(|check| match check.image {
                    Image::Valid(app) => Some((app.size(), *app.file_sha256())),
                    Image::Empty | Image::Invalid(_) => None,
                }),
            _ => None,
        };
        Ok(Content {
            view: self,
            name,
            handle,
            generation,
            image,
            _held: held,
        })
    }
}

impl Content {
    /// The region's name.
    pub(crate) fn name(&self) -> RegionName {
        self.name
    }

    /// The slot an app region is, when the app booted from one.
    pub(crate) fn slot(&self) -> Option<Slot> {
        match self.name {
            RegionName::App(role) => self.view.0.slot(role),
            _ => None,
        }
    }

    /// The region's length in bytes.
    pub(crate) fn len(&self) -> u32 {
        self.handle.len()
    }

    /// The random value drawn once this boot.
    pub(crate) fn boot(&self) -> u64 {
        self.view.boot()
    }

    /// The region's write generation when the content was made.
    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }

    /// What the bytes from `first` to `last`, both included, hold: exactly
    /// the image, when the firmware has a current digest of it; the whole of
    /// an app slot holding such an image; the whole region; or a range.
    pub(crate) fn part(&self, first: u32, last: u32) -> Part {
        let whole = first == 0 && last.checked_add(1) == Some(self.len());
        match self.image {
            Some((size, sha256)) if first == 0 && last.checked_add(1) == Some(size) => {
                Part::Image(sha256)
            }
            Some((_, sha256)) if whole && matches!(self.name, RegionName::App(_)) => {
                Part::SlotOf(sha256)
            }
            _ if whole => Part::Whole,
            _ => Part::Range { first, last },
        }
    }

    /// Reads the bytes at `offset` in the region into `buf`, and feeds the
    /// download's watchdog handle.
    ///
    /// # Errors
    ///
    /// [`ContentReadError::Read`] when the read fails, and
    /// [`ContentReadError::Written`] when the region has been written since
    /// the content was made.
    pub(crate) async fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ContentReadError> {
        self.view.0.download.wd.feed();
        self.handle
            .read(offset, buf)
            .await
            .map_err(ContentReadError::Read)?;
        // Checked after the read: a write bumps the generation before it
        // runs, so a read that overlapped one sees the generation moved.
        let generation = self
            .view
            .generations()
            .of(self.handle.offset(), self.handle.len());
        if generation != self.generation {
            return Err(ContentReadError::Written);
        }
        Ok(())
    }
}

impl Service {
    /// The handle for the public region `name`, when the firmware holds one.
    fn handle(&self, name: RegionName) -> Option<Handle> {
        let public = &self.public;
        match name {
            RegionName::Bootloader => Some(Handle::Bootloader(public.boot.bootloader)),
            RegionName::PartitionTable => public.boot.table.map(Handle::PartitionTable),
            RegionName::Otadata => public.otadata.map(Handle::Otadata),
            RegionName::App(AppRole::Current) => public.boot.running_app.map(Handle::RunningApp),
            RegionName::App(AppRole::Alternate) => public.other_app.map(Handle::OtherApp),
            RegionName::Config(_)
            | RegionName::Diag
            | RegionName::Factory
            | RegionName::Reserved => None,
        }
    }
}
