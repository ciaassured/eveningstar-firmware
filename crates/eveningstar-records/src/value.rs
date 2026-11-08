//! Reading an entry's fields.

use core::fmt;

/// A value shorter than its type's minimum length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooShort {
    /// The value's length.
    pub len: usize,
    /// The type's minimum length.
    pub min: usize,
}

impl fmt::Display for TooShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "value of {} bytes, shorter than {}", self.len, self.min)
    }
}

/// An entry's value, at least as long as its type's minimum length.
///
/// Fields lie at fixed offsets. A field that ends within the minimum length
/// is always there; one past it was added later, and reads as absent when
/// the value ends before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value<'a> {
    bytes: &'a [u8],
}

impl<'a> Value<'a> {
    /// `bytes` as a value of a type whose minimum length is `min`.
    ///
    /// # Errors
    ///
    /// [`TooShort`] when `bytes` is shorter than `min`.
    pub const fn new(bytes: &'a [u8], min: usize) -> Result<Self, TooShort> {
        if bytes.len() < min {
            return Err(TooShort {
                len: bytes.len(),
                min,
            });
        }
        Ok(Self { bytes })
    }

    /// The value's bytes, padding included.
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The `N` bytes at `offset`, or `None` when the value ends before them.
    #[must_use]
    pub fn array<const N: usize>(&self, offset: usize) -> Option<[u8; N]> {
        let end = offset.checked_add(N)?;
        self.bytes.get(offset..end)?.try_into().ok()
    }

    /// The `len` bytes at `offset`, or `None` when the value ends before them.
    #[must_use]
    pub fn slice(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        self.bytes.get(offset..offset.checked_add(len)?)
    }

    /// The byte at `offset`.
    #[must_use]
    pub fn u8(&self, offset: usize) -> Option<u8> {
        self.bytes.get(offset).copied()
    }

    /// The little-endian `u16` at `offset`.
    #[must_use]
    pub fn u16(&self, offset: usize) -> Option<u16> {
        self.array(offset).map(u16::from_le_bytes)
    }

    /// The little-endian `u32` at `offset`.
    #[must_use]
    pub fn u32(&self, offset: usize) -> Option<u32> {
        self.array(offset).map(u32::from_le_bytes)
    }

    /// The little-endian `u64` at `offset`.
    #[must_use]
    pub fn u64(&self, offset: usize) -> Option<u64> {
        self.array(offset).map(u64::from_le_bytes)
    }
}
