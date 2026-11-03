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
//! [`digest`] gives the SHA-256 of any range of a published view as served,
//! the transport digest.
//!
//! Every method takes `&self`. Results that cost a hash to compute are kept
//! behind a critical-section mutex, held only to copy them in or out, never
//! across an `.await`, so a module in a `static` is shared between tasks.
//!
//! Long jobs, hashing an image or a digest, take the caller's hasher
//! ([`Sha256Hasher`]), pace ([`Pace`]) and progress hook ([`Progress`]): the
//! hook is called and the pace run after each KiB hashed.
//!
//! The crate is `no_std`, allocation-free, and also builds on the host.
//!
//! [`Published`]: eveningstar_storage::nor::Published
//! [`Sha256Hasher`]: eveningstar_esp_formats::hash::Sha256Hasher
//! [`Pace`]: eveningstar_esp_formats::pace::Pace

#![cfg_attr(not(test), no_std)]

pub mod bootloader;
mod digest;
mod error;
mod pace;
pub mod table;
#[cfg(test)]
mod test_support;

pub use bootloader::Bootloader;
pub use digest::{Digest, PAUSE_INTERVAL, digest};
pub use error::{ImageFailure, LoadError, Unpublished};
pub use pace::Progress;
pub use table::PartitionTable;
