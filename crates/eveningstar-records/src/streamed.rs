//! Finding an entry in a container that isn't held in memory, read through a
//! callback, with the rules [`Container`] applies.
//!
//! [`Container`]: crate::Container

use crate::container::{Error, HEADER_LEN};
use crate::{ALIGN, ENTRY_HEADER_LEN};

/// Where an entry's value lies in its container: the offset from the
/// container's start, its header included, and the length, padding included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Offset of the value from the start of the container.
    pub offset: usize,
    /// Length of the value, padding included.
    pub len: usize,
}

/// [`Container::parse`] of the first `limit` bytes of a container with
/// magic `magic`, then [`Container::find`] of `kind`, over a container read
/// through `read` rather than held: `read(offset, buf)` fills `buf` with
/// the bytes at `offset` of the container, and is called only inside
/// `limit`. Each step gives the verdict the held container would give: the
/// same error, or the place of the same entry.
///
/// # Errors
///
/// What `read` returns, outside; inside, the [`Error`] the held container
/// would give.
///
/// [`Container::parse`]: crate::Container::parse
/// [`Container::find`]: crate::Container::find
pub fn find<E>(
    limit: usize,
    magic: u32,
    kind: u16,
    mut read: impl FnMut(usize, &mut [u8]) -> Result<(), E>,
) -> Result<Result<Option<Place>, Error>, E> {
    if limit < HEADER_LEN {
        return Ok(Err(Error::Truncated));
    }
    let mut header = [0; HEADER_LEN];
    read(0, &mut header)?;
    let [m0, m1, m2, m3, l0, l1] = header;
    let found = u32::from_le_bytes([m0, m1, m2, m3]);
    if found != magic {
        return Ok(Err(Error::Magic { found }));
    }
    let total = u16::from_le_bytes([l0, l1]);
    let end = usize::from(total);
    if end < HEADER_LEN || end > limit || !(end - HEADER_LEN).is_multiple_of(ALIGN) {
        return Ok(Err(Error::Length { total }));
    }
    let mut at = HEADER_LEN;
    let mut place = None;
    while at < end {
        // The body's length is a multiple of 4, so an entry's header always
        // fits once one starts.
        let mut entry = [0; ENTRY_HEADER_LEN];
        read(at, &mut entry)?;
        let [k0, k1, n0, n1] = entry;
        let len = usize::from(u16::from_le_bytes([n0, n1]));
        if !len.is_multiple_of(ALIGN) {
            return Ok(Err(Error::EntryPadding { at }));
        }
        let start = at + ENTRY_HEADER_LEN;
        if start + len > end {
            return Ok(Err(Error::EntryTruncated { at }));
        }
        if u16::from_le_bytes([k0, k1]) == kind {
            if place.is_some() {
                return Ok(Err(Error::Duplicate { kind }));
            }
            place = Some(Place { offset: start, len });
        }
        at = start + len;
    }
    Ok(Ok(place))
}

#[cfg(test)]
mod tests;
