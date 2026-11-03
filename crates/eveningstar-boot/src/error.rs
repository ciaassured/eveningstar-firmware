//! What the modules return when a region can't be parsed or published, and
//! why an image doesn't verify, without the values of bytes nothing
//! verified.

use eveningstar_esp_formats::image::ImageError;
use eveningstar_storage::nor::{Published, Reader, Snapshot};
use eveningstar_storage::roles::{Internal, Publishable, Role};
use eveningstar_storage::{Epoch, PublishError, ReadError, SnapshotError, Span};

/// Why a module couldn't parse its region now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// A write session is open on the region.
    Busy,
    /// The region was written while it was parsed, or isn't at the epoch
    /// asked for.
    Stale,
    /// The module has no reader of the region: the chain located none.
    Absent,
}

/// Why a module published no view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpublished {
    /// The model shows nothing a check covers, so no byte is published.
    Unverified,
    /// The region was written since the model was parsed, or a session is
    /// open on it.
    Stale,
    /// The storage crate refused the view.
    Refused(PublishError),
}

impl From<PublishError> for Unpublished {
    fn from(error: PublishError) -> Self {
        match error {
            PublishError::Stale => Self::Stale,
            other => Self::Refused(other),
        }
    }
}

/// Why an image doesn't verify, as [`ImageError`] gives it, without the
/// values it read: a magic, a chip ID, a length or a checksum of bytes that
/// didn't verify are bytes nothing covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFailure {
    /// Reading the region failed.
    Read(ReadError),
    /// The region ends before the header, the checksum or the digest does.
    PastEnd,
    /// The first byte isn't an image's magic.
    Magic,
    /// The header names another chip than the ESP32-C6.
    Chip,
    /// The header names more segments than an image has.
    TooManySegments,
    /// A segment's length isn't a multiple of 4.
    SegmentLength {
        /// Position of the segment.
        index: u8,
    },
    /// A segment's header or data runs past the end of the region.
    SegmentPastEnd {
        /// Position of the segment.
        index: u8,
    },
    /// A segment mapped from flash lies at another offset within an MMU page
    /// in the flash than at its load address.
    SegmentMapping {
        /// Position of the segment.
        index: u8,
    },
    /// The app descriptor names an MMU page size that doesn't fit in 32
    /// bits.
    MmuPageSize,
    /// The checksum stored doesn't match the segment data.
    Checksum,
    /// The header says no SHA-256 digest is appended.
    NoDigest,
    /// The appended SHA-256 digest doesn't match the image.
    DigestMismatch,
    /// The first segment doesn't start with an app descriptor.
    MissingDescriptor,
}

impl From<ImageError> for ImageFailure {
    fn from(error: ImageError) -> Self {
        match error {
            ImageError::Read(error) => Self::Read(error),
            ImageError::PastEnd => Self::PastEnd,
            ImageError::Magic(_) => Self::Magic,
            ImageError::Chip(_) => Self::Chip,
            ImageError::TooManySegments(_) => Self::TooManySegments,
            ImageError::SegmentLength { index, .. } => Self::SegmentLength { index },
            ImageError::SegmentPastEnd { index } => Self::SegmentPastEnd { index },
            ImageError::SegmentMapping { index } => Self::SegmentMapping { index },
            ImageError::MmuPageSize(_) => Self::MmuPageSize,
            ImageError::Checksum { .. } => Self::Checksum,
            ImageError::NoDigest => Self::NoDigest,
            ImageError::DigestMismatch => Self::DigestMismatch,
            ImageError::MissingDescriptor => Self::MissingDescriptor,
        }
    }
}

/// A snapshot of the whole of `reader`'s region.
///
/// # Errors
///
/// [`LoadError::Busy`] while a write session is open on the region.
pub(crate) fn whole<'r, R: Role<Device = Internal>>(
    reader: &'r Reader<'_, R>,
) -> Result<Snapshot<'r, R>, LoadError> {
    // A span from 0 to the region's length lies inside it, so the only
    // refusal is a session.
    reader
        .snapshot(Span::new(0, reader.len()))
        .map_err(|error| match error {
            SnapshotError::Busy | SnapshotError::OutOfRange => LoadError::Busy,
        })
}

/// A snapshot of the whole of `reader`'s region at `epoch`.
///
/// # Errors
///
/// [`LoadError::Busy`] while a write session is open on the region, and
/// [`LoadError::Stale`] when it is stable at another epoch.
pub(crate) fn at<'r, R: Role<Device = Internal>>(
    reader: &'r Reader<'_, R>,
    epoch: Epoch,
) -> Result<Snapshot<'r, R>, LoadError> {
    let snapshot = whole(reader)?;
    if snapshot.epoch() == epoch {
        Ok(snapshot)
    } else {
        Err(LoadError::Stale)
    }
}

/// Why an image read through a snapshot doesn't verify, without the values
/// read: `Err(Stale)` when a read found the region written, as that is no
/// result for the snapshot's epoch.
pub(crate) fn failure(error: ImageError) -> Result<ImageFailure, LoadError> {
    match error {
        ImageError::Read(ReadError::Stale) => Err(LoadError::Stale),
        other => Ok(other.into()),
    }
}

/// The result of a verification read through a snapshot, its error as
/// [`failure`] gives it.
pub(crate) fn verdict<T>(
    result: Result<T, ImageError>,
) -> Result<Result<T, ImageFailure>, LoadError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(error) => failure(error).map(Err),
    }
}

/// `reader`'s view of `span` as of `epoch`, with `sanitized` served as
/// `0xFF` ([`Reader::publish`]). Never inlined, so the reads of the
/// sanitized spans stay out of the callers' frames.
#[inline(never)]
pub(crate) fn publish<'r, R: Publishable<Device = Internal>>(
    reader: &'r Reader<'_, R>,
    epoch: Epoch,
    span: Span,
    sanitized: &[Span],
) -> Result<Published<'r, R>, PublishError> {
    reader.publish(epoch, span, sanitized)
}
