//! OpenBSD signify's signed manifests, verified under a compiled set of trusted
//! keys.
//!
//! A release's `SHA256.sig` is what `signify -S -e` writes for a SHA256
//! listing: an untrusted comment line, the signature as one line of base64,
//! and the listing itself. [`verify`] accepts a subset of what signify
//! accepts, never more:
//!
//! - the whole file at most 4096 bytes, holding no NUL;
//! - line 1 `untrusted comment: ` and 1 to 1023 bytes, whose content is
//!   ignored;
//! - line 2 exactly 100 characters of canonical base64, decoding to the 74
//!   bytes `Ed`, a keynum and the Ed25519 signature;
//! - the rest the signed message, 1 to 2048 bytes, every line of it
//!   `SHA256 (<name>) = <64 lowercase hex>`, `<name>` 1 to 128 bytes of
//!   `[A-Za-z0-9._+-]`.
//!
//! The keynum picks the key out of the [`TrustSet`]; the signature then has to
//! verify under it with ed25519-dalek's `verify_strict`, which, unlike
//! signify's, refuses a scalar of `L` or more and small-order points. A
//! [`Verified`] file then gives the [`Line`] whose digest is an image's.
//!
//! [`TrustSet`]s and their [`TrustedKey`]s have no public constructor: only
//! this crate makes them, and, with the `test-support` feature, tests.
//! [`public_key`] reads a signify `.pub` file, and [`check_key`] refuses a key
//! no trust set may hold.
//!
//! The crate is `no_std`, allocation-free and free of `unsafe`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod file;
mod key;
mod manifest;

pub use file::{FILE_MAX, MESSAGE_MAX, SigError, verify};
pub use key::{Generation, KeyError, Keynum, Purpose, TrustSet, TrustedKey, check_key, public_key};
pub use manifest::{Line, ManifestError, Verified};

/// Length of a SHA-256 digest in bytes.
pub const DIGEST_LEN: usize = 32;
