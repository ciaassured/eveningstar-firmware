//! Byte sources: anything that can be read at offsets.

/// Why a read from a [`Source`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Part of the range lies at or past the end of the source.
    OutOfRange,
    /// The device behind the source failed the read.
    Device,
}

/// Bytes that can be read at offsets: a byte slice, or a region of flash.
///
/// Offsets are relative to the start of the source, and every byte from 0 up
/// to [`size`](Self::size) is readable.
pub trait Source {
    /// Size of the source in bytes.
    fn size(&self) -> u32;

    /// Fills `buf` with the bytes starting at `offset`.
    ///
    /// # Errors
    ///
    /// [`ReadError::OutOfRange`] when the range doesn't lie inside the source,
    /// and [`ReadError::Device`] when the device behind it fails the read.
    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError>;
}

impl Source for [u8] {
    /// The slice's length, or `u32::MAX` for a slice longer than that.
    fn size(&self) -> u32 {
        u32::try_from(self.len()).unwrap_or(u32::MAX)
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        let len = u32::try_from(buf.len()).map_err(|_| ReadError::OutOfRange)?;
        let end = offset.checked_add(len).ok_or(ReadError::OutOfRange)?;
        if end > self.size() {
            return Err(ReadError::OutOfRange);
        }
        let start = usize::try_from(offset).map_err(|_| ReadError::OutOfRange)?;
        let end = usize::try_from(end).map_err(|_| ReadError::OutOfRange)?;
        let bytes = self.get(start..end).ok_or(ReadError::OutOfRange)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

/// Reads the `N` bytes at `offset`.
pub(crate) fn read_array<S: Source + ?Sized, const N: usize>(
    source: &S,
    offset: u32,
) -> Result<[u8; N], ReadError> {
    let mut bytes = [0; N];
    source.read(offset, &mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_reads_inside_its_bounds() {
        let bytes: &[u8] = &[1, 2, 3, 4, 5];
        assert_eq!(bytes.size(), 5);

        let mut buf = [0; 2];
        bytes.read(3, &mut buf).unwrap();
        assert_eq!(buf, [4, 5]);

        bytes.read(5, &mut []).unwrap();
        assert_eq!(read_array::<_, 5>(bytes, 0), Ok([1, 2, 3, 4, 5]));
    }

    #[test]
    fn a_slice_refuses_reads_past_its_end() {
        let bytes: &[u8] = &[1, 2, 3, 4, 5];
        let mut buf = [0; 2];
        assert_eq!(bytes.read(4, &mut buf), Err(ReadError::OutOfRange));
        assert_eq!(bytes.read(6, &mut []), Err(ReadError::OutOfRange));
        assert_eq!(bytes.read(u32::MAX, &mut buf), Err(ReadError::OutOfRange));
        assert_eq!(read_array::<_, 6>(bytes, 0), Err(ReadError::OutOfRange));
        // Nothing is written on failure.
        assert_eq!(buf, [0, 0]);
    }

    #[test]
    fn an_empty_slice_has_nothing_to_read() {
        let bytes: &[u8] = &[];
        assert_eq!(bytes.size(), 0);
        bytes.read(0, &mut []).unwrap();
        assert_eq!(bytes.read(0, &mut [0]), Err(ReadError::OutOfRange));
    }
}
