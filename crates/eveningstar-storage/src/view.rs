//! What readers hand out, whatever the device: snapshots, published views,
//! and their errors.

use core::future::Future;

use crate::{Epoch, ReadError, Span};

mod sealed {
    /// Implemented by the crate's published views only.
    pub trait Sealed {}
}

pub(crate) use sealed::Sealed;

/// Why a reader refused a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// A write session is open on the region.
    Busy,
    /// The span doesn't lie inside the region.
    OutOfRange,
}

/// The most sanitized spans a published view has.
pub const MAX_SANITIZED: usize = 4;

/// Why a reader refused to publish a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishError {
    /// The region isn't stable at the epoch given: a session is open on it,
    /// or it was written since, or the epoch is another region's.
    Stale,
    /// The view's span doesn't lie inside the region.
    OutOfRange,
    /// A sanitized span doesn't lie inside the view.
    SanitizedOutOfRange,
    /// More than [`MAX_SANITIZED`] sanitized spans.
    TooManySanitized,
    /// The view's span overlaps a span the compiled layout gives to
    /// sensitive data.
    Sensitive,
    /// The flash failed a read of a sanitized span.
    Device,
}

/// Why a read of a published view failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// Part of the range lies past the end of the view.
    OutOfRange,
    /// The flash failed the read.
    Device,
    /// The region was written since the view was published.
    Stale,
}

impl From<ReadError> for ViewError {
    fn from(error: ReadError) -> Self {
        match error {
            ReadError::OutOfRange => Self::OutOfRange,
            ReadError::Device => Self::Device,
            ReadError::Stale => Self::Stale,
        }
    }
}

/// A published view: bytes that may leave the device, as of one epoch.
///
/// The trait is sealed: the crate's published views are its only
/// implementations. It returns `impl Future`, so it isn't dyn-compatible.
pub trait PublicRead: Sealed {
    /// Length of the view in bytes.
    fn len(&self) -> u32;

    /// Whether the view has no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The region's epoch the view serves.
    fn epoch(&self) -> Epoch;

    /// The view-relative spans served as `0xFF` that hold something else on
    /// the flash.
    fn sanitized(&self) -> &[Span];

    /// Fills `buf` with the view's bytes at `offset`, the sanitized ones as
    /// `0xFF`.
    ///
    /// # Errors
    ///
    /// [`ViewError::OutOfRange`] when the range doesn't lie inside the view,
    /// [`ViewError::Device`] when the flash fails a read, and
    /// [`ViewError::Stale`] when the region was written since the view was
    /// published; `buf` is zeroed on any.
    fn read(&self, offset: u32, buf: &mut [u8]) -> impl Future<Output = Result<(), ViewError>>;
}
