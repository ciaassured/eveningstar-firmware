//! Transport digests: the SHA-256 of a range of a published view, as served.

use eveningstar_esp_formats::hash::Sha256Hasher;
use eveningstar_esp_formats::pace::Pace;
use eveningstar_storage::{Epoch, PublicRead, Span, ViewError};

use crate::pace::{Paced, Progress};

/// Length of each read of a digest, in bytes.
const READ_LEN: usize = 256;

/// The number of bytes hashed between two pauses.
pub const PAUSE_INTERVAL: u32 = 1024;

/// The SHA-256 of a range of a published view, with the view's epoch: the
/// digest of the bytes as the view serves them, sanitized ones as `0xFF`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Digest {
    /// The SHA-256 of the range.
    pub sha256: [u8; 32],
    /// The region's epoch every byte hashed was read at.
    pub epoch: Epoch,
}

/// The SHA-256 of the bytes `span` of `view`, relative to the view, as the
/// view serves them: every sanitized byte as `0xFF`, whatever the flash
/// holds. Hashes with `sha`, a hash not yet fed, reads 256 bytes at a time,
/// and after each [`PAUSE_INTERVAL`] bytes hashed calls `progress`, then
/// `pace`.
///
/// Every read is checked against the view's epoch, so the digest covers bytes
/// of that epoch only, which it is returned with.
///
/// # Errors
///
/// [`ViewError::OutOfRange`] when `span` doesn't lie inside the view, before
/// anything is read, [`ViewError::Stale`] once the region was written since
/// the view was published, and [`ViewError::Device`] when the flash fails a
/// read.
pub async fn digest<V, H, P, G>(
    view: &V,
    span: Span,
    sha: H,
    pace: &mut P,
    progress: &mut G,
) -> Result<Digest, ViewError>
where
    V: PublicRead,
    H: Sha256Hasher,
    P: Pace,
    G: Progress,
{
    if !Span::new(0, view.len()).contains(span) {
        return Err(ViewError::OutOfRange);
    }
    let end = span.end().ok_or(ViewError::OutOfRange)?;
    let mut paced = Paced::new(pace, progress);
    let mut sha = sha;
    let mut buf = [0; READ_LEN];
    let mut at = span.offset;
    let mut since_pause = 0_u32;
    while at < end {
        // At most READ_LEN.
        let step = usize::try_from(end - at).unwrap_or(READ_LEN).min(READ_LEN);
        let chunk = buf.get_mut(..step).unwrap_or_default();
        view.read(at, chunk).await?;
        sha.update(chunk);
        let step = u32::try_from(step).map_err(|_| ViewError::OutOfRange)?;
        at = at.checked_add(step).ok_or(ViewError::OutOfRange)?;
        since_pause = since_pause.saturating_add(step);
        if since_pause >= PAUSE_INTERVAL {
            since_pause = 0;
            paced.pause().await;
        }
    }
    Ok(Digest {
        sha256: sha.finalize(),
        epoch: view.epoch(),
    })
}

#[cfg(test)]
mod tests;
