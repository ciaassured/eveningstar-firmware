//! Snapshots: reads of a region that fail once it is written.

use core::fmt;
use core::marker::PhantomData;

use super::Reader;
use super::handle::{Extent, address, read_chunked};
use crate::roles::{Internal, Role};
use crate::span::Span;
use crate::state::{Epoch, WriteState};
use crate::{ReadError, SnapshotError, Source};

/// Fills `buf` with the bytes at `offset` in `extent`'s span, in one device
/// call, when the region covering `extent`'s physical spans is
/// `Stable(epoch)` both before and after it. On any error `buf` is zeroed, so
/// no bytes of a failed read are kept.
///
/// Both loads of the state are `SeqCst`, and the device call between them is
/// a compiler and hardware barrier on the one core that reaches the flash,
/// so the read happens after the first load and before the second. A session
/// stores its open state before its first device call and a new epoch after
/// its last, so a read whose loads both see `Stable(epoch)` overlapped no
/// session that issued a device call on one of those spans: its bytes are
/// those of `epoch`.
pub(super) fn read_checked(
    extent: &Extent<'_>,
    epoch: Epoch,
    offset: u32,
    buf: &mut [u8],
) -> Result<(), ReadError> {
    let result = (|| {
        let address = address(extent.span, offset, buf.len())?;
        if extent.state.load(extent.spans) != WriteState::Stable(epoch) {
            return Err(ReadError::Stale);
        }
        extent
            .device
            .read(address, buf)
            .map_err(|_| ReadError::Device)?;
        if extent.state.load(extent.spans) != WriteState::Stable(epoch) {
            return Err(ReadError::Stale);
        }
        Ok(())
    })();
    if result.is_err() {
        buf.fill(0);
    }
    result
}

/// A snapshot of part of a region with role `R`, from [`Reader::snapshot`]:
/// reads of the region's bytes as of one epoch.
///
/// Every read loads the region's write state, reads the device, and loads
/// the state again; unless both loads see the region stable at the
/// snapshot's epoch, the read fails with [`ReadError::Stale`] and its bytes
/// are discarded. So a snapshot's reads return bytes of its epoch or an
/// error, never a mix. A region over several physical spans has every one of
/// them checked.
///
/// Offsets are relative to the start of the snapshot's span.
pub struct Snapshot<'r, R: Role<Device = Internal>> {
    /// The device, the snapshot's absolute span, and the region's physical
    /// spans, whose state every read checks.
    extent: Extent<'r>,
    /// The span relative to the region.
    span: Span,
    epoch: Epoch,
    role: PhantomData<fn() -> R>,
}

impl<R: Role<Device = Internal>> Reader<'_, R> {
    /// A snapshot of `span`, relative to the region, at the region's epoch
    /// now.
    ///
    /// # Errors
    ///
    /// [`SnapshotError::Busy`] while a write session is open on the region,
    /// and [`SnapshotError::OutOfRange`] when `span` doesn't lie inside it.
    pub fn snapshot(&self, span: Span) -> Result<Snapshot<'_, R>, SnapshotError> {
        let absolute = self
            .extent
            .span
            .inner(span)
            .ok_or(SnapshotError::OutOfRange)?;
        let WriteState::Stable(epoch) = self.write_state() else {
            return Err(SnapshotError::Busy);
        };
        Ok(Snapshot {
            extent: Extent {
                span: absolute,
                ..self.extent
            },
            span,
            epoch,
            role: PhantomData,
        })
    }
}

impl<R: Role<Device = Internal>> Snapshot<'_, R> {
    /// The region's epoch the snapshot reads.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The snapshot's span, relative to the region.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// Fills `buf` with the bytes at `offset` in the snapshot, in steps of at
    /// most 4 KiB with a yield after each, each checked against the epoch.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfRange`] when the range doesn't lie inside the
    /// snapshot, before anything is read, [`ReadError::Device`] when the
    /// flash fails a read, and [`ReadError::Stale`] when the region was
    /// written since the snapshot was taken; `buf` is zeroed on any.
    pub async fn read_async(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_chunked(self.span.len, offset, buf, |at, chunk| {
            Source::read(self, at, chunk)
        })
        .await
    }
}

impl<R: Role<Device = Internal>> Source for Snapshot<'_, R> {
    fn size(&self) -> u32 {
        self.span.len
    }

    /// Reads the snapshot in one device call, checked against its epoch;
    /// `buf` is zeroed on any error.
    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        read_checked(&self.extent, self.epoch, offset, buf)
    }
}

impl<R: Role<Device = Internal>> fmt::Debug for Snapshot<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("role", &R::NAME)
            .field("span", &self.span)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}
