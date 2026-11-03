//! Publication: the views of a publishable region whose bytes may leave the
//! device.

use core::fmt;
use core::marker::PhantomData;

use super::Reader;
use super::handle::{Extent, read_chunked};
use super::snapshot::read_checked;
use crate::roles::is_sensitive;
use crate::roles::{Internal, Publishable};
use crate::state::{Epoch, WriteState};
use crate::view::{MAX_SANITIZED, PublicRead, PublishError, Sealed, ViewError};
use crate::{ReadError, Span};

/// Length of the buffer the sanitized spans are checked through.
const CHECK_BUF_LEN: usize = 256;

/// Whether every byte of `span`, relative to `extent`'s span, is `0xFF` at
/// `epoch`, read in steps of [`CHECK_BUF_LEN`] bytes.
fn erased(extent: &Extent<'_>, epoch: Epoch, span: Span) -> Result<bool, PublishError> {
    let mut buf = [0; CHECK_BUF_LEN];
    let end = span.end().ok_or(PublishError::SanitizedOutOfRange)?;
    let mut at = span.offset;
    while at < end {
        // At most CHECK_BUF_LEN.
        let step = usize::try_from(end - at)
            .unwrap_or(CHECK_BUF_LEN)
            .min(CHECK_BUF_LEN);
        let chunk = buf.get_mut(..step).unwrap_or_default();
        read_checked(extent, epoch, at, chunk).map_err(|error| match error {
            ReadError::Stale => PublishError::Stale,
            ReadError::Device => PublishError::Device,
            ReadError::OutOfRange => PublishError::SanitizedOutOfRange,
        })?;
        if chunk.iter().any(|byte| *byte != 0xFF) {
            return Ok(false);
        }
        at = at.saturating_add(u32::try_from(step).unwrap_or(u32::MAX));
    }
    Ok(true)
}

impl<R: Publishable<Device = Internal>> Reader<'_, R> {
    /// The bytes `span` of the region, for export, as of `epoch`, the epoch
    /// the caller's parser read them at. The bytes of each `sanitized` span,
    /// relative to the view, are served as `0xFF` whatever the flash holds.
    ///
    /// The sanitized spans are read once, here, to record those whose flash
    /// bytes aren't all `0xFF` already ([`PublicRead::sanitized`]); the
    /// reads take one device call per 256 bytes and are checked against
    /// `epoch` like a snapshot's. The view's bytes are read when it is read.
    ///
    /// # Errors
    ///
    /// [`PublishError::OutOfRange`] when `span` doesn't lie inside the
    /// region, [`PublishError::Sensitive`] when it overlaps `factory`, a
    /// configuration or `diag` of the compiled layout,
    /// [`PublishError::TooManySanitized`] for more than [`MAX_SANITIZED`]
    /// sanitized spans, [`PublishError::SanitizedOutOfRange`] when one
    /// doesn't lie inside the view, [`PublishError::Stale`] unless the
    /// region is stable at `epoch` now and while the sanitized spans are
    /// read, and [`PublishError::Device`] when the flash fails one of those
    /// reads.
    pub fn publish(
        &self,
        epoch: Epoch,
        span: Span,
        sanitized: &[Span],
    ) -> Result<Published<'_, R>, PublishError> {
        let absolute = self
            .extent
            .span
            .inner(span)
            .ok_or(PublishError::OutOfRange)?;
        if is_sensitive(absolute) {
            return Err(PublishError::Sensitive);
        }
        if sanitized.len() > MAX_SANITIZED {
            return Err(PublishError::TooManySanitized);
        }
        let view = Span::new(0, span.len);
        if !sanitized.iter().all(|inner| view.contains(*inner)) {
            return Err(PublishError::SanitizedOutOfRange);
        }
        if self.write_state() != WriteState::Stable(epoch) {
            return Err(PublishError::Stale);
        }
        let extent = Extent {
            span: absolute,
            ..self.extent
        };
        let mut kept = [Span::default(); MAX_SANITIZED];
        let mut count = 0;
        for inner in sanitized {
            if !erased(&extent, epoch, *inner)?
                && let Some(slot) = kept.get_mut(count)
            {
                *slot = *inner;
                count += 1;
            }
        }
        Ok(Published {
            extent,
            span,
            epoch,
            sanitized: kept,
            count,
            role: PhantomData,
        })
    }
}

/// A published view of a publishable region with role `R`, from
/// [`Reader::publish`]: bytes that may leave the device, as of one epoch.
///
/// Reads are checked against the epoch like a snapshot's, and fail
/// [`ViewError::Stale`] once the region was written. The sanitized spans are
/// served as `0xFF`; those whose flash bytes aren't `0xFF` at the epoch are
/// listed ([`PublicRead::sanitized`]), and no others.
pub struct Published<'r, R: Publishable<Device = Internal>> {
    /// The device, the view's absolute span, and the region's physical
    /// spans.
    extent: Extent<'r>,
    /// The view's span, relative to the region.
    span: Span,
    epoch: Epoch,
    /// The sanitized spans whose bytes aren't all `0xFF`, relative to the
    /// view: the first `count`.
    sanitized: [Span; MAX_SANITIZED],
    count: usize,
    role: PhantomData<fn() -> R>,
}

impl<R: Publishable<Device = Internal>> Published<'_, R> {
    /// The view's span, relative to the region.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }
}

impl<R: Publishable<Device = Internal>> Sealed for Published<'_, R> {}

impl<R: Publishable<Device = Internal>> PublicRead for Published<'_, R> {
    fn len(&self) -> u32 {
        self.span.len
    }

    fn epoch(&self) -> Epoch {
        self.epoch
    }

    fn sanitized(&self) -> &[Span] {
        self.sanitized.get(..self.count).unwrap_or_default()
    }

    /// Reads the view in steps of at most 4 KiB with a yield after each,
    /// each checked against the epoch, then sets the sanitized bytes to
    /// `0xFF`. The sanitized spans not listed are `0xFF` on the flash at the
    /// epoch already.
    async fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ViewError> {
        read_chunked(self.span.len, offset, buf, |at, chunk| {
            read_checked(&self.extent, self.epoch, at, chunk)
        })
        .await?;
        let len = u32::try_from(buf.len()).map_err(|_| ViewError::OutOfRange)?;
        let read = Span::new(offset, len);
        for inner in self.sanitized() {
            let Some(shared) = read.intersection(*inner) else {
                continue;
            };
            let start = shared
                .offset
                .checked_sub(offset)
                .and_then(|start| usize::try_from(start).ok());
            let end = start.and_then(|start| start.checked_add(usize::try_from(shared.len).ok()?));
            if let Some(bytes) = start
                .zip(end)
                .and_then(|(start, end)| buf.get_mut(start..end))
            {
                bytes.fill(0xFF);
            }
        }
        Ok(())
    }
}

impl<R: Publishable<Device = Internal>> fmt::Debug for Published<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Published")
            .field("role", &R::NAME)
            .field("span", &self.span)
            .field("epoch", &self.epoch)
            .field("sanitized", &self.sanitized())
            .finish_non_exhaustive()
    }
}
