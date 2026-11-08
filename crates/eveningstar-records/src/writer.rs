//! Writing a container.

use core::fmt;

use crate::container::HEADER_LEN;
use crate::{ALIGN, ENTRY_HEADER_LEN};

/// [`HEADER_LEN`] as the `u16` the writer counts in.
const HEADER_LEN_U16: u16 = 6;
const _: () = assert!(HEADER_LEN_U16 as usize == HEADER_LEN);

/// The buffer has no room for the entry, or the container would pass the
/// largest total length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Full;

impl fmt::Display for Full {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no room for the entry")
    }
}

/// A container being written into a buffer: the header, then entries in the
/// order they are pushed, each value zero-padded to a multiple of 4.
#[derive(Debug)]
pub struct Writer<'a> {
    buf: &'a mut [u8],
    /// Bytes written, the header included.
    len: u16,
}

impl<'a> Writer<'a> {
    /// A container with `magic` at the start of `buf`.
    ///
    /// # Errors
    ///
    /// [`Full`] when `buf` is shorter than a header.
    pub fn new(buf: &'a mut [u8], magic: u32) -> Result<Self, Full> {
        let header = buf.get_mut(..HEADER_LEN).ok_or(Full)?;
        header[..4].copy_from_slice(&magic.to_le_bytes());
        header[4..].copy_from_slice(&0u16.to_le_bytes());
        Ok(Self {
            buf,
            len: HEADER_LEN_U16,
        })
    }

    /// Adds an entry of type `kind` holding `value`, padded with zeros.
    ///
    /// # Errors
    ///
    /// [`Full`] when the buffer has no room for it, or the container would
    /// be longer than a `u16` length can say; nothing is written then.
    pub fn push(&mut self, kind: u16, value: &[u8]) -> Result<(), Full> {
        let padded = value.len().checked_next_multiple_of(ALIGN).ok_or(Full)?;
        let padded_u16 = u16::try_from(padded).map_err(|_| Full)?;
        let end =
            u16::try_from(usize::from(self.len) + ENTRY_HEADER_LEN + padded).map_err(|_| Full)?;
        let entry = self
            .buf
            .get_mut(usize::from(self.len)..usize::from(end))
            .ok_or(Full)?;
        let (header, rest) = entry.split_at_mut(ENTRY_HEADER_LEN);
        header[..2].copy_from_slice(&kind.to_le_bytes());
        header[2..].copy_from_slice(&padded_u16.to_le_bytes());
        let (data, padding) = rest.split_at_mut(value.len());
        data.copy_from_slice(value);
        padding.fill(0);
        self.len = end;
        Ok(())
    }

    /// Ends the container: writes its total length, and returns its bytes.
    #[must_use]
    pub fn finish(self) -> &'a [u8] {
        let Self { buf, len } = self;
        // `new` refused a buffer shorter than the header.
        buf[4..6].copy_from_slice(&len.to_le_bytes());
        &buf[..usize::from(len)]
    }
}
