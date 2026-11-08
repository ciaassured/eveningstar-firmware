//! Reading a container and its entries.

use core::fmt;

use crate::{ALIGN, ENTRY_HEADER_LEN};

/// Length of a container's header, its magic and its total length, in bytes.
pub const HEADER_LEN: usize = 6;

/// Why a container or one of its entries doesn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Fewer bytes than a container header.
    Truncated,
    /// The magic isn't the one expected.
    Magic {
        /// The magic read.
        found: u32,
    },
    /// The total length is shorter than the header, longer than the bytes
    /// given, or leaves entry bytes that aren't a multiple of 4.
    Length {
        /// The total length read.
        total: u16,
    },
    /// An entry's header or value runs past the container's total length.
    EntryTruncated {
        /// The entry's offset in the container.
        at: usize,
    },
    /// An entry's length isn't a multiple of 4.
    EntryPadding {
        /// The entry's offset in the container.
        at: usize,
    },
    /// A type a reader looks for appears more than once.
    Duplicate {
        /// The type.
        kind: u16,
    },
    /// A type a reader requires is missing.
    Missing {
        /// The type.
        kind: u16,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("shorter than a container header"),
            Self::Magic { found } => write!(f, "unexpected magic {found:#010x}"),
            Self::Length { total } => write!(f, "total length {total} doesn't fit"),
            Self::EntryTruncated { at } => write!(f, "entry at {at} runs past the container"),
            Self::EntryPadding { at } => write!(f, "entry at {at} isn't padded to 4 bytes"),
            Self::Duplicate { kind } => write!(f, "type {kind} appears more than once"),
            Self::Missing { kind } => write!(f, "type {kind} is missing"),
        }
    }
}

/// A container whose header read: a magic and a total length that fits the
/// bytes it was read from. Its entries are checked as they are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Container<'a> {
    magic: u32,
    /// The entries, from just after the header to the total length.
    body: &'a [u8],
}

impl<'a> Container<'a> {
    /// The container at the start of `bytes`, whose magic has to be `magic`.
    /// Bytes past its total length are ignored.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] when `bytes` is shorter than a header,
    /// [`Error::Magic`] for another magic, and [`Error::Length`] when the
    /// total length is shorter than the header, longer than `bytes`, or
    /// leaves entry bytes that aren't a multiple of 4.
    pub fn parse(bytes: &'a [u8], magic: u32) -> Result<Self, Error> {
        let (Some(found), Some(total)) = (read_u32(bytes, 0), read_u16(bytes, 4)) else {
            return Err(Error::Truncated);
        };
        if found != magic {
            return Err(Error::Magic { found });
        }
        let body = bytes
            .get(HEADER_LEN..usize::from(total))
            .filter(|body| body.len() % ALIGN == 0)
            .ok_or(Error::Length { total })?;
        Ok(Self { magic, body })
    }

    /// The container's magic.
    #[must_use]
    pub const fn magic(&self) -> u32 {
        self.magic
    }

    /// The container's total length, its header included.
    #[must_use]
    pub const fn total_len(&self) -> usize {
        HEADER_LEN + self.body.len()
    }

    /// The entries, in order. After an entry that doesn't read, the iterator
    /// yields that error and ends.
    #[must_use]
    pub const fn entries(&self) -> Entries<'a> {
        Entries {
            body: self.body,
            at: 0,
            failed: false,
        }
    }

    /// The entry of type `kind`, when there is exactly one.
    ///
    /// # Errors
    ///
    /// [`Error::Duplicate`] when it appears more than once, and any error of
    /// an entry before the end.
    pub fn find(&self, kind: u16) -> Result<Option<Entry<'a>>, Error> {
        let mut found = None;
        for entry in self.entries() {
            let entry = entry?;
            if entry.kind == kind {
                if found.is_some() {
                    return Err(Error::Duplicate { kind });
                }
                found = Some(entry);
            }
        }
        Ok(found)
    }

    /// The entry of type `kind`, which has to be there exactly once.
    ///
    /// # Errors
    ///
    /// As [`find`](Self::find), and [`Error::Missing`] when it isn't there.
    pub fn require(&self, kind: u16) -> Result<Entry<'a>, Error> {
        self.find(kind)?.ok_or(Error::Missing { kind })
    }
}

/// An entry: its type and its value, padding included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The entry's type.
    pub kind: u16,
    /// The entry's value, padding included.
    pub value: &'a [u8],
}

/// The entries of a [`Container`], from [`Container::entries`].
#[derive(Clone, Debug)]
pub struct Entries<'a> {
    body: &'a [u8],
    /// Offset of the next entry in the body.
    at: usize,
    failed: bool,
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<Entry<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.at >= self.body.len() {
            return None;
        }
        let at = self.at;
        let entry = read_entry(self.body, at);
        match entry {
            Ok((entry, next)) => {
                self.at = next;
                Some(Ok(entry))
            }
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
}

/// The entry at `at` in `body`, and the offset after it. Offsets in errors
/// are the container's, header included.
fn read_entry(body: &[u8], at: usize) -> Result<(Entry<'_>, usize), Error> {
    let offset = HEADER_LEN + at;
    let (Some(kind), Some(len)) = (read_u16(body, at), read_u16(body, at + 2)) else {
        return Err(Error::EntryTruncated { at: offset });
    };
    let len = usize::from(len);
    if len % ALIGN != 0 {
        return Err(Error::EntryPadding { at: offset });
    }
    let start = at + ENTRY_HEADER_LEN;
    let value = body
        .get(start..start + len)
        .ok_or(Error::EntryTruncated { at: offset })?;
    Ok((Entry { kind, value }, start + len))
}

/// The little-endian `u16` at `at` in `bytes`.
pub(crate) fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let word = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(word.try_into().ok()?))
}

/// The little-endian `u32` at `at` in `bytes`.
pub(crate) fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(word.try_into().ok()?))
}

#[cfg(test)]
mod tests;
