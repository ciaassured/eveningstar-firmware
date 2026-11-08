//! The record encoding every firmware version shares: the image information
//! block an image carries after its layout record, and the diagnostics
//! mailbox and log.
//!
//! Older firmware reads what newer firmware wrote (after a rollback), and
//! newer reads older (after an update), so the encoding never changes and
//! grows only by addition:
//!
//! - A [`Container`] starts with a magic (`u32`) and a total length (`u16`),
//!   little-endian, and holds entries up to that length.
//! - An [`Entry`] is a type (`u16`), a length (`u16`) and that many bytes of
//!   value, zero-padded to a multiple of 4; the length counts the padding.
//! - Unknown types are skipped by the reader that doesn't need them.
//! - A value grows at its end only. Each type has a minimum length, fixed when
//!   the type is first released; fields added later are optional, and a
//!   [`Value`] reads one the value ends before as absent.
//! - A field never changes meaning: a new meaning is a new type.
//!
//! The [`registry`] describes types and checks that a new description is
//! compatible with a released one.
//!
//! The crate is `no_std`, allocation-free and free of `unsafe`; every parser
//! returns an error rather than panicking on any input.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod container;
pub mod registry;
mod value;
mod writer;

pub use container::{Container, Entries, Entry, Error, HEADER_LEN};
pub use value::{TooShort, Value};
pub use writer::{Full, Writer};

/// Length of an entry's header, its type and its length, in bytes.
pub const ENTRY_HEADER_LEN: usize = 4;

/// The unit entry values are padded to, in bytes.
pub const ALIGN: usize = 4;
