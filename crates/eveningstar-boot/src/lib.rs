//! The content modules of the internal flash's boot regions: what the
//! bootloader, the partition table, the otadata and the app slots hold, and
//! which of their bytes may leave the device.
//!
//! Each module owns the readers `eveningstar-storage` hands out for its
//! regions and never hands one out, nor a snapshot of one. It offers parsed
//! models, which only its parser makes and which carry the epoch they were
//! parsed at, and published views ([`Published`]) over the bytes its parser
//! verified with a check that covers each of them. The views are made fresh
//! for each call from a model, so a view never serves bytes of an epoch other
//! than the one its model was parsed at.
//!
//! - [`Bootloader`] owns the bootloader's region.
//!   - Model: header, length, stored digest, file SHA-256, known entry; the
//!     descriptor on request.
//!   - Published: the image, once its SHA-256 verifies, known or not.
//! - [`PartitionTable`] owns the compiled table's sector, the table the search
//!   found.
//!   - Model: the table as the bootloader judges it, its partitions, MD5 entry
//!     and comparison with the layout, how it was located.
//!   - Published: the entries, the MD5 entry and the end entry a check covers,
//!     with the bytes nothing checks sanitized.
//! - [`Otadata`] owns the otadata.
//!   - Model: both entries, the active one, the boot state.
//!   - Published: each entry of the compiled otadata the parser pins down,
//!     blank or with a matching CRC and a defined state, its label sanitized;
//!     the fields of [`otadata::OtadataFields`].
//! - [`AppSlot`] owns one app region.
//!   - Model: header, length, stored digest, file SHA-256, per epoch; the
//!     descriptor on request.
//!   - Published: the image, once verified at the region's epoch now.
//!
//! [`App`] refers to the slots of a tier and runs their checks in the
//! background; [`AppSlot::verdict`] waits for a slot's result at an epoch.
//! [`digest`] gives the SHA-256 of any range of a published view as served,
//! the transport digest.
//!
//! A failed verification keeps its reason without the values it read
//! ([`ImageFailure`], [`table::TableFailure`]), as they are bytes nothing
//! verified.
//!
//! Every method takes `&self`, and every module is `Sync`, so a module in a
//! `static` is shared between tasks. Results that cost a hash to compute are
//! kept behind a critical-section mutex, held only to copy them in or out,
//! never across an `.await`; the others are parsed again on each request.
//!
//! Long jobs, hashing an image or a digest, take the caller's hasher
//! ([`Sha256Hasher`]), pace ([`Pace`]) and progress hook ([`Progress`]): the
//! hook is called and the pace run after each KiB hashed. The app checks
//! ([`App::run`]) also tell the hook when they wait for a slot to change
//! ([`Progress::idle`]).
//!
//! The crate is `no_std`, allocation-free, and also builds on the host.
//!
//! [`Published`]: eveningstar_storage::nor::Published
//! [`Sha256Hasher`]: eveningstar_esp_formats::hash::Sha256Hasher
//! [`Pace`]: eveningstar_esp_formats::pace::Pace

#![cfg_attr(not(test), no_std)]

pub mod app;
pub mod bootloader;
mod digest;
mod error;
pub mod otadata;
mod pace;
pub mod table;
#[cfg(test)]
mod test_support;

pub use app::{App, AppSlot};
pub use bootloader::Bootloader;
pub use digest::{Digest, PAUSE_INTERVAL, digest};
pub use error::{ImageFailure, LoadError, Unpublished};
pub use otadata::Otadata;
pub use pace::Progress;
pub use table::PartitionTable;
