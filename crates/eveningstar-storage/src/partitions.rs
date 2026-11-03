//! Typed handles to the partitions, and the roles that decide what each can
//! do.
//!
//! A handle is a [`Partition<'f, R>`]: a span of the flash and a role `R`, an
//! uninhabited marker type. Only [`split`](crate::split) makes handles, one
//! per role at most. What a handle can do is a trait it implements:
//!
//! | Role | Class | [`SensitiveRead`] | [`Read`] | [`Erasable`] | [`Writable`] | [`FixedSize`] | Copy |
//! |---|---|---|---|---|---|---|---|
//! | [`Bootloader`] | public | yes | yes | | | | yes |
//! | [`PartitionTable`] | public | yes | yes | | | | yes |
//! | [`RunningApp`] | public | yes | yes | | | | yes |
//! | [`OtherApp`] | public | yes | yes | | | yes | yes |
//! | [`Otadata`] | public | yes | yes | | | yes | yes |
//! | [`Config`] | sensitive | yes | | yes | yes | yes | |
//! | [`OtherConfig`] | sensitive | yes | | yes | | yes | |
//! | [`Diag`] | sensitive | yes | | yes | yes | yes | |
//!
//! [`Role`] and [`FixedSize`] are traits of the role, as they are known
//! without a handle; the capabilities are traits of the handle, so a function
//! taking `&impl Read` takes a public handle and nothing else. Every trait is
//! sealed.
//!
//! [`SensitiveRead`] is the read every handle has. [`Read`] adds nothing but
//! the permission for the contents to leave the device, and only public
//! handles have it. Public handles are also [`Source`]s, which the image and
//! table parsers read, and `Copy`. Sensitive handles are neither, and don't
//! implement the `embedded_storage_async` traits either:
//! [`into_nor_flash`](Partition::into_nor_flash) turns a [`Writable`] handle
//! into a [`NorPartition`] that does, consuming it.
//!
//! ```
//! use eveningstar_storage::partitions::{Config, OtherApp, Partition, Read, SensitiveRead};
//!
//! fn export(_: &impl Read) {}
//! fn load(_: &impl SensitiveRead) {}
//!
//! fn run(config: Partition<'_, Config>, app: Partition<'_, OtherApp>) {
//!     load(&config);
//!     load(&app);
//!     export(&app);
//!     let copy = app;
//!     export(&copy);
//!     export(&app);
//! }
//! ```
//!
//! A sensitive handle isn't [`Read`]:
//!
//! ```compile_fail,E0277
//! use eveningstar_storage::partitions::{Config, Partition, Read};
//!
//! fn export(_: &impl Read) {}
//!
//! fn run(config: Partition<'_, Config>) {
//!     export(&config);
//! }
//! ```
//!
//! nor is the other slot's configuration ever programmed:
//!
//! ```
//! use eveningstar_storage::partitions::{Config, Erasable, OtherConfig, Partition, Writable};
//!
//! async fn run(mut config: Partition<'_, Config>, mut other: Partition<'_, OtherConfig>) {
//!     let _ = config.program(0, &[0; 4]).await;
//!     let _ = other.erase(0, 4096).await;
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::partitions::{OtherConfig, Partition, Writable};
//!
//! async fn run(mut other: Partition<'_, OtherConfig>) {
//!     let _ = other.program(0, &[0; 4]).await;
//! }
//! ```
//!
//! and a public handle is never erased:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::partitions::{Erasable, OtherApp, Partition};
//!
//! async fn run(mut app: Partition<'_, OtherApp>) {
//!     let _ = app.erase(0, 4096).await;
//! }
//! ```
//!
//! Sensitive handles are unique: they are neither cloned nor copied.
//!
//! ```
//! use eveningstar_storage::partitions::{Config, Partition};
//!
//! fn keep(config: Partition<'_, Config>) -> Partition<'_, Config> {
//!     config
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::partitions::{Config, Partition};
//!
//! fn run(config: Partition<'_, Config>) {
//!     let _twin = config.clone();
//! }
//! ```
//!
//! ```compile_fail,E0382
//! use eveningstar_storage::partitions::{Config, Partition};
//!
//! fn run(config: Partition<'_, Config>) -> [Partition<'_, Config>; 2] {
//!     [config, config]
//! }
//! ```
//!
//! [`FixedSize`] gives the partitions of the layout their size at compile
//! time:
//!
//! ```
//! use eveningstar_storage::partitions::{Config, FixedSize};
//!
//! const { assert!(Config::SIZE >= 2 * eveningstar_storage::SECTOR_SIZE) };
//! ```
//!
//! but the boot chain's spans are found at runtime, so they have none:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::partitions::{FixedSize, RunningApp};
//!
//! const SIZE: u32 = RunningApp::SIZE;
//! ```

use core::fmt;
use core::marker::PhantomData;

use embedded_storage_async::nor_flash::{ErrorType, MultiwriteNorFlash, NorFlash, ReadNorFlash};

use crate::nor::{SECTOR_SIZE, WORD_SIZE};
use crate::region::{AccessError, Region};
use crate::{ReadError, Source, layout};

mod sealed {
    /// Implemented by the roles and the handles of this module only.
    pub trait Sealed {}
}

use sealed::Sealed;

/// A partition's role: what it holds, and so what its handle can do.
pub trait Role: Sealed {
    /// The role's name, for logs.
    const NAME: &'static str;
}

/// A role whose partition is one of the generated layout's, so its size is
/// known at compile time. Its handles are made only from the layout's
/// constants: `len()` is always [`SIZE`](Self::SIZE).
pub trait FixedSize: Role {
    /// Size of the partition in bytes.
    const SIZE: u32;
}

/// A handle that reads its partition. Every handle has it, the sensitive ones
/// included: a function that handles secrets takes `impl SensitiveRead`.
pub trait SensitiveRead: Sealed {
    /// Offset of the partition from the start of the flash.
    fn offset(&self) -> u32;

    /// Size of the partition in bytes.
    fn len(&self) -> u32;

    /// Whether the partition has no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` with the bytes at `offset` in the partition, yielding
    /// after every 4 KiB read.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfRange`] when the range doesn't lie inside the
    /// partition, and [`ReadError::Device`] when the flash fails the read.
    fn read(&self, offset: u32, buf: &mut [u8]) -> impl Future<Output = Result<(), ReadError>>;
}

/// A handle whose contents may leave the device: a public partition. A
/// function that exports data takes `impl Read`, so it can't be given a
/// sensitive handle.
pub trait Read: SensitiveRead {}

/// A handle that erases its partition.
pub trait Erasable: SensitiveRead {
    /// Erases the sectors from `from` up to `to`, offsets in the partition on
    /// sector boundaries, setting their bytes to `0xFF`. Erases one sector at
    /// a time and yields after each.
    ///
    /// # Errors
    ///
    /// [`AccessError::OutOfRange`] when `from` is past `to` or `to` past the
    /// end of the partition, [`AccessError::NotAligned`] when either isn't on
    /// a sector boundary, [`AccessError::Protected`] when the range touches a
    /// protected region, which no handle covers, all before anything is
    /// erased, and [`AccessError::Device`] when the flash fails an erase.
    fn erase(&mut self, from: u32, to: u32) -> impl Future<Output = Result<(), AccessError>>;
}

/// A handle that programs its partition, with NOR semantics: programming
/// only clears bits, so each byte becomes the AND of what it held and what is
/// programmed. Nothing reads, erases and rewrites a sector on the way.
pub trait Writable: Erasable {
    /// Programs `bytes` at `offset` in the partition, both the offset and the
    /// length multiples of [`WORD_SIZE`](crate::WORD_SIZE), in steps of at
    /// most 4 KiB with a yield after each.
    ///
    /// # Errors
    ///
    /// [`AccessError::NotAligned`] when the offset or the length isn't a
    /// multiple of the word size, [`AccessError::OutOfRange`] when the range
    /// doesn't lie inside the partition, [`AccessError::Protected`] when it
    /// touches a protected region, all before anything is programmed, and
    /// [`AccessError::Device`] when the flash fails a program.
    fn program(
        &mut self,
        offset: u32,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), AccessError>>;
}

/// The bootloader at 0x0, up to the partition table.
#[derive(Debug)]
pub enum Bootloader {}

/// The partition table the bootloader read: the table at
/// [`layout::TABLE_OFFSET`] when it matches the layout, else the one found on
/// the board.
#[derive(Debug)]
pub enum PartitionTable {}

/// The running app: its slot in the data and full tiers, else its entry in
/// the board's table, or its image when there is no table.
#[derive(Debug)]
pub enum RunningApp {}

/// The app slot that isn't running, read only: an update writes it through
/// [`Ota`](crate::Ota).
#[derive(Debug)]
pub enum OtherApp {}

/// The otadata partition, read only: the boot selection changes through
/// [`Ota`](crate::Ota).
#[derive(Debug)]
pub enum Otadata {}

/// The running slot's configuration partition: read, erased and programmed.
#[derive(Debug)]
pub enum Config {}

/// The other slot's configuration partition: read, to migrate after an
/// update, and erased, never programmed.
#[derive(Debug)]
pub enum OtherConfig {}

/// The diagnostics partition: read, erased and programmed.
#[derive(Debug)]
pub enum Diag {}

const _: () = assert!(layout::OTA_0.size == layout::OTA_1.size);
const _: () = assert!(layout::CONFIG_0.size == layout::CONFIG_1.size);

macro_rules! roles {
    ($($role:ident = $name:literal $(, size $size:expr)?;)*) => {$(
        impl Sealed for $role {}

        impl Role for $role {
            const NAME: &'static str = $name;
        }

        $(impl FixedSize for $role {
            const SIZE: u32 = $size;
        })?
    )*};
}

roles! {
    Bootloader = "bootloader";
    PartitionTable = "partition table";
    RunningApp = "running app";
    OtherApp = "other app", size layout::OTA_0.size;
    Otadata = "otadata", size layout::OTADATA.size;
    Config = "config", size layout::CONFIG_0.size;
    OtherConfig = "other config", size layout::CONFIG_0.size;
    Diag = "diag", size layout::DIAG.size;
}

/// A handle to a partition with role `R`, made only by
/// [`split`](crate::split). The module documentation lists what each role's
/// handle can do.
pub struct Partition<'f, R: Role> {
    region: Region<'f>,
    role: PhantomData<R>,
}

impl<'f, R: Role> Partition<'f, R> {
    pub(crate) const fn new(region: Region<'f>) -> Self {
        Self {
            region,
            role: PhantomData,
        }
    }
}

impl<R: Role> Sealed for Partition<'_, R> {}

impl<R: Role> SensitiveRead for Partition<'_, R> {
    fn offset(&self) -> u32 {
        self.region.offset()
    }

    fn len(&self) -> u32 {
        self.region.len()
    }

    async fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        self.region.read_async(offset, buf).await
    }
}

impl<R: Role> fmt::Debug for Partition<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Partition")
            .field("role", &R::NAME)
            .field("offset", &self.region.offset())
            .field("len", &self.region.len())
            .finish()
    }
}

macro_rules! public {
    ($($role:ident),*) => {$(
        impl Read for Partition<'_, $role> {}

        impl Clone for Partition<'_, $role> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl Copy for Partition<'_, $role> {}

        impl Source for Partition<'_, $role> {
            fn size(&self) -> u32 {
                self.region.len()
            }

            fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
                Source::read(&self.region, offset, buf)
            }
        }

        impl Partition<'_, $role> {
            /// The SHA-256 digest of the `len` bytes at `offset` in the
            /// partition, yielding after every 4 KiB hashed.
            ///
            /// # Errors
            ///
            /// [`ReadError::OutOfRange`] when the range doesn't lie inside
            /// the partition, and [`ReadError::Device`] when the flash fails
            /// a read.
            pub async fn sha256(&self, offset: u32, len: u32) -> Result<[u8; 32], ReadError> {
                self.region.sha256(offset, len).await
            }
        }
    )*};
}

public!(Bootloader, PartitionTable, RunningApp, OtherApp, Otadata);

macro_rules! erasable {
    ($($role:ident),*) => {$(
        impl Erasable for Partition<'_, $role> {
            async fn erase(&mut self, from: u32, to: u32) -> Result<(), AccessError> {
                self.region.erase(from, to, None).await
            }
        }
    )*};
}

erasable!(Config, OtherConfig, Diag);

macro_rules! writable {
    ($($role:ident),*) => {$(
        impl Writable for Partition<'_, $role> {
            async fn program(&mut self, offset: u32, bytes: &[u8]) -> Result<(), AccessError> {
                self.region.program(offset, bytes, None).await
            }
        }

        impl<'f> Partition<'f, $role> {
            /// Turns the handle into a [`NorPartition`], which implements the
            /// `embedded_storage_async` NOR flash traits, for the store that
            /// owns the partition.
            #[must_use]
            pub fn into_nor_flash(self) -> NorPartition<'f, $role> {
                NorPartition {
                    region: self.region,
                    role: PhantomData,
                }
            }
        }
    )*};
}

writable!(Config, Diag);

/// A [`Writable`] partition behind the `embedded_storage_async` NOR flash
/// traits, from [`Partition::into_nor_flash`]: reads at any alignment,
/// programs words of [`WORD_SIZE`](crate::WORD_SIZE) bytes, any number of
/// times ([`MultiwriteNorFlash`]), and erases sectors. It holds the
/// partition's handle, so there is one at most.
pub struct NorPartition<'f, R: Role> {
    region: Region<'f>,
    role: PhantomData<R>,
}

impl<R: Role> fmt::Debug for NorPartition<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NorPartition")
            .field("role", &R::NAME)
            .field("offset", &self.region.offset())
            .field("len", &self.region.len())
            .finish()
    }
}

impl<R: Role> ErrorType for NorPartition<'_, R> {
    type Error = AccessError;
}

impl<R: Role> ReadNorFlash for NorPartition<'_, R> {
    const READ_SIZE: usize = 1;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), AccessError> {
        Ok(self.region.read_async(offset, bytes).await?)
    }

    fn capacity(&self) -> usize {
        usize::try_from(self.region.len()).unwrap_or(usize::MAX)
    }
}

impl<R: Role> NorFlash for NorPartition<'_, R> {
    const WRITE_SIZE: usize = WORD_SIZE as usize;
    const ERASE_SIZE: usize = SECTOR_SIZE as usize;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), AccessError> {
        self.region.erase(from, to, None).await
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), AccessError> {
        self.region.program(offset, bytes, None).await
    }
}

impl<R: Role> MultiwriteNorFlash for NorPartition<'_, R> {}

#[cfg(test)]
mod tests;
