//! NOR flash: the device trait, its sizes, the state table and the handles
//! of the internal flash's regions.
//!
//! A region's handles are a [`Reader<'f, R>`] and a [`Writer<'f, R>`], where
//! `R` is its [role]. Each holds the device, the [`State`] table and the
//! region's span; neither is `Clone` nor `Copy`, and only the crate makes them.
//! A writer reads at any time and writes only through a [`Session`], which
//! marks the region written to while it lives.
//!
//! A writer erases and programs only inside a session:
//!
//! ```
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::{AlternateConfig, CurrentConfig};
//!
//! async fn store(config: &mut Writer<'_, CurrentConfig>, old: &mut Writer<'_, AlternateConfig>) {
//!     let mut session = config.begin();
//!     let _ = session.erase(0, 4096).await;
//!     let _ = session.program(0, &[0; 4]).await;
//!     drop(session);
//!     let _ = old.begin().wipe().await;
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::CurrentConfig;
//!
//! async fn store(config: &mut Writer<'_, CurrentConfig>) {
//!     let _ = config.program(0, &[0; 4]).await;
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::Diag;
//!
//! async fn wipe(diag: &mut Writer<'_, Diag>) {
//!     let _ = diag.erase(0, 4096).await;
//! }
//! ```
//!
//! The alternate configuration's session wipes the whole region, and never
//! programs:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::AlternateConfig;
//!
//! async fn store(old: &mut Writer<'_, AlternateConfig>) {
//!     let _ = old.begin().program(0, &[0; 4]).await;
//! }
//! ```
//!
//! and never erases part of it:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::AlternateConfig;
//!
//! async fn store(old: &mut Writer<'_, AlternateConfig>) {
//!     let _ = old.begin().erase(0, 4096).await;
//! }
//! ```
//!
//! A role without a writer has none:
//!
//! ```compile_fail,E0277
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::Bootloader;
//!
//! fn rewrite(_: &Writer<'_, Bootloader>) {}
//! ```
//!
//! ```compile_fail,E0277
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::CurrentApp;
//!
//! fn overwrite(_: &Writer<'_, CurrentApp>) {}
//! ```
//!
//! A function takes the role it needs, and no handle of another role:
//!
//! ```compile_fail,E0308
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::{AlternateApp, CurrentConfig};
//!
//! fn activate(_: &Writer<'_, AlternateApp>) {}
//!
//! fn run(config: &Writer<'_, CurrentConfig>) {
//!     activate(config);
//! }
//! ```
//!
//! ```compile_fail,E0308
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::{AlternateApp, CurrentApp};
//!
//! fn check(_: &Reader<'_, CurrentApp>) {}
//!
//! fn run(alternate: &Reader<'_, AlternateApp>) {
//!     check(alternate);
//! }
//! ```
//!
//! No handle is cloned or copied:
//!
//! ```
//! use eveningstar_storage::nor::{Reader, Writer};
//! use eveningstar_storage::roles::{AlternateApp, Diag};
//!
//! fn keep<'f>(
//!     app: Reader<'f, AlternateApp>,
//!     diag: Writer<'f, Diag>,
//! ) -> (Reader<'f, AlternateApp>, Writer<'f, Diag>) {
//!     (app, diag)
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::AlternateApp;
//!
//! fn run(app: Reader<'_, AlternateApp>) {
//!     let _twin = app.clone();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::Diag;
//!
//! fn run(diag: Writer<'_, Diag>) {
//!     let _twin = diag.clone();
//! }
//! ```
//!
//! ```compile_fail,E0382
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::Otadata;
//!
//! fn run(otadata: Reader<'_, Otadata>) -> [Reader<'_, Otadata>; 2] {
//!     [otadata, otadata]
//! }
//! ```
//!
//! ```compile_fail,E0382
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::Otadata;
//!
//! fn run(otadata: Writer<'_, Otadata>) -> [Writer<'_, Otadata>; 2] {
//!     [otadata, otadata]
//! }
//! ```
//!
//! A reader reads through snapshots, which fail once the region is written
//! ([`Snapshot`]). Only a reader of a [`Publishable`] role publishes views
//! ([`Published`]), whose bytes may leave the device:
//!
//! ```
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::{AlternateApp, Bootloader, FoundApp, Otadata};
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(
//!     app: &Reader<'_, AlternateApp>,
//!     boot: &Reader<'_, Bootloader>,
//!     found: &Reader<'_, FoundApp>,
//!     otadata: &Reader<'_, Otadata>,
//!     epoch: Epoch,
//! ) {
//!     let _ = app.publish(epoch, Span::new(0, 4), &[]);
//!     let _ = boot.publish(epoch, Span::new(0, 4), &[]);
//!     let _ = found.publish(epoch, Span::new(0, 4), &[]);
//!     let _ = otadata.publish(epoch, Span::new(0, 32), &[]);
//! }
//! ```
//!
//! The located otadata, the sensitive roles and the whole chip have no
//! `publish`, whatever the tier:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::FoundOtadata;
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(otadata: &Reader<'_, FoundOtadata>, epoch: Epoch) {
//!     let _ = otadata.publish(epoch, Span::new(0, 4), &[]);
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::Diag;
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(diag: &Reader<'_, Diag>, epoch: Epoch) {
//!     let _ = diag.publish(epoch, Span::new(0, 4), &[]);
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::CurrentConfig;
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(config: &Reader<'_, CurrentConfig>, epoch: Epoch) {
//!     let _ = config.publish(epoch, Span::new(0, 4), &[]);
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::Factory;
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(factory: &Reader<'_, Factory>, epoch: Epoch) {
//!     let _ = factory.publish(epoch, Span::new(0, 4), &[]);
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::nor::Reader;
//! use eveningstar_storage::roles::Chip;
//! use eveningstar_storage::{Epoch, Span};
//!
//! fn export(chip: &Reader<'_, Chip>, epoch: Epoch) {
//!     let _ = chip.publish(epoch, Span::new(0, 4), &[]);
//! }
//! ```
//!
//! Only the crate's views are published views:
//!
//! ```compile_fail,E0277
//! use eveningstar_storage::{Epoch, PublicRead, Span, ViewError};
//!
//! struct Raw;
//!
//! impl PublicRead for Raw {
//!     fn len(&self) -> u32 { 0 }
//!     fn epoch(&self) -> Epoch { unimplemented!() }
//!     fn sanitized(&self) -> &[Span] { &[] }
//!     async fn read(&self, _: u32, _: &mut [u8]) -> Result<(), ViewError> { Ok(()) }
//! }
//! ```
//!
//! A session borrows its writer mutably, so a region has one session at a
//! time:
//!
//! ```compile_fail,E0499
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::Diag;
//!
//! fn run(diag: &mut Writer<'_, Diag>) {
//!     let first = diag.begin();
//!     let second = diag.begin();
//!     drop((first, second));
//! }
//! ```
//!
//! [role]: crate::roles
//! [`Publishable`]: crate::roles::Publishable

mod handle;
mod published;
mod snapshot;
mod state;

pub use handle::{Reader, Session, Writer};
pub use published::Published;
pub use snapshot::Snapshot;
pub use state::State;

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

#[cfg(test)]
mod tests;
