//! The ESP-IDF partition table: reading its entries, comparing it with the
//! generated layout, and finding it at another offset.
//!
//! The table is a list of 32-byte entries starting at the table offset, at
//! most [`MAX_LEN`] bytes long. Each entry is the magic `0x50AA` (`u16`), the
//! type and subtype (`u8` each), the offset and size (`u32`), the label (16
//! bytes, NUL padded) and the flags (`u32`), little-endian. The list ends at an
//! MD5 entry (magic `0xEBEB`, with the MD5 of the preceding entries in its last
//! 16 bytes) or at an erased entry (magic, type and subtype all `0xFF`).
//!
//! The MD5 isn't checked: the comparison with the layout is exact, so a table
//! that matches is the table the layout describes whatever its MD5, and the
//! discovery of a moved table relies on an app entry at the booted offset.
//! What follows the MD5 entry is ignored.

use core::fmt;
use core::ops::BitOr;

use crate::source::{Source, read_array};
use crate::{Entry, ReadError, TYPE_APP, Text, layout};

/// Maximum length of the table in bytes.
pub const MAX_LEN: u32 = 0xC00;

/// Length of one entry in bytes.
pub const ENTRY_LEN: u32 = 32;

/// Maximum number of entries, the MD5 entry and the end of the list included.
pub const MAX_ENTRIES: usize = 96;

/// Alignment of the offsets [`find`] looks at: one flash sector.
pub const SECTOR_SIZE: u32 = 4096;

const MAGIC: u16 = 0x50AA;
const MD5_MAGIC: u16 = 0xEBEB;
const ERASED: u8 = 0xFF;

/// A partition's label: up to 16 bytes, NUL padded.
pub type Label = Text<16>;

/// One partition entry, as stored in the table. Types, subtypes and flags are
/// kept as the raw numbers, whether or not ESP-IDF defines them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawEntry {
    /// The label, as the 16 bytes stored.
    pub label: Label,
    /// Partition type (`kind`, as `type` is a keyword).
    pub kind: u8,
    /// Partition subtype.
    pub subtype: u8,
    /// Offset from the start of the flash, in bytes.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
    /// Flag bits.
    pub flags: u32,
}

impl RawEntry {
    /// The fields in which this entry differs from `expected`. Labels are
    /// compared as all 16 bytes, so a label with bytes after its NUL differs.
    #[must_use]
    pub fn differences(&self, expected: &Entry) -> Fields {
        let mut fields = Fields::NONE;
        let mut differs = |field, same: bool| {
            if !same {
                fields = fields | field;
            }
        };
        differs(Fields::LABEL, self.label.is_padded(expected.label));
        differs(Fields::KIND, self.kind == expected.kind);
        differs(Fields::SUBTYPE, self.subtype == expected.subtype);
        differs(Fields::OFFSET, self.offset == expected.offset);
        differs(Fields::SIZE, self.size == expected.size);
        differs(Fields::FLAGS, self.flags == expected.flags);
        fields
    }

    /// The entry stored in `bytes`, whose magic has already been checked.
    fn from_bytes(bytes: &[u8; 32]) -> Self {
        let [
            _,
            _,
            kind,
            subtype,
            o0,
            o1,
            o2,
            o3,
            s0,
            s1,
            s2,
            s3,
            label @ ..,
            f0,
            f1,
            f2,
            f3,
        ] = *bytes;
        Self {
            label: Text::from_raw(label),
            kind,
            subtype,
            offset: u32::from_le_bytes([o0, o1, o2, o3]),
            size: u32::from_le_bytes([s0, s1, s2, s3]),
            flags: u32::from_le_bytes([f0, f1, f2, f3]),
        }
    }
}

/// A set of entry fields, such as the fields in which an entry differs from
/// the layout.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Fields(u8);

impl Fields {
    /// No field.
    pub const NONE: Self = Self(0);
    /// The label.
    pub const LABEL: Self = Self(1 << 0);
    /// The partition type.
    pub const KIND: Self = Self(1 << 1);
    /// The partition subtype.
    pub const SUBTYPE: Self = Self(1 << 2);
    /// The offset.
    pub const OFFSET: Self = Self(1 << 3);
    /// The size.
    pub const SIZE: Self = Self(1 << 4);
    /// The flags.
    pub const FLAGS: Self = Self(1 << 5);
    /// Every field.
    pub const ALL: Self = Self(0x3F);

    const NAMES: [(Self, &'static str); 6] = [
        (Self::LABEL, "LABEL"),
        (Self::KIND, "KIND"),
        (Self::SUBTYPE, "SUBTYPE"),
        (Self::OFFSET, "OFFSET"),
        (Self::SIZE, "SIZE"),
        (Self::FLAGS, "FLAGS"),
    ];

    /// Whether every field in `other` is in this set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set has no field.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The set as bits: label, type, subtype, offset, size and flags from the
    /// lowest bit up.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl BitOr for Fields {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl fmt::Debug for Fields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Fields(")?;
        let mut first = true;
        for (field, name) in Self::NAMES {
            if self.contains(field) {
                if !first {
                    f.write_str(" | ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str(")")
    }
}

/// How a well-formed list of entries ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// At an MD5 entry, the entry at `index`, holding `digest` (not checked).
    Md5 {
        /// Position of the MD5 entry, which is also the number of partitions.
        index: usize,
        /// The digest the entry stores.
        digest: [u8; 16],
    },
    /// At an erased entry, the entry at `index`.
    Erased {
        /// Position of the erased entry, which is also the number of
        /// partitions.
        index: usize,
    },
}

impl End {
    /// The number of partition entries before the end.
    #[must_use]
    pub const fn partitions(&self) -> usize {
        match *self {
            Self::Md5 { index, .. } | Self::Erased { index } => index,
        }
    }
}

/// Why a table isn't well formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// The entry at `index` has a magic that is neither a partition's, the MD5
    /// entry's nor erased (an erased magic with a type or subtype that isn't
    /// `0xFF` included).
    Magic {
        /// Position of the entry.
        index: usize,
        /// The magic it stores.
        magic: u16,
    },
    /// The first entry is erased: there is no table.
    Empty,
    /// [`MAX_ENTRIES`] entries and no MD5 or erased entry among them.
    Unterminated,
    /// Reading the entry at `index` failed.
    Read {
        /// Position of the entry.
        index: usize,
        /// Why the read failed.
        error: ReadError,
    },
}

/// One slot of the table.
enum Slot {
    Partition(RawEntry),
    Md5([u8; 16]),
    Erased,
}

/// The slot stored in `bytes`, or its magic when that is malformed.
fn parse_slot(bytes: &[u8; 32]) -> Result<Slot, u16> {
    let [m0, m1, kind, subtype, ..] = *bytes;
    match u16::from_le_bytes([m0, m1]) {
        MAGIC => Ok(Slot::Partition(RawEntry::from_bytes(bytes))),
        MD5_MAGIC => {
            let [_, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, digest @ ..] = *bytes;
            Ok(Slot::Md5(digest))
        }
        0xFFFF if kind == ERASED && subtype == ERASED => Ok(Slot::Erased),
        magic => Err(magic),
    }
}

/// The partition entries of the table at `offset` in `source`, read one at a
/// time.
pub fn entries<S: Source + ?Sized>(source: &S, offset: u32) -> Entries<'_, S> {
    Entries {
        source,
        offset,
        index: 0,
        state: State::Reading,
    }
}

/// An iterator over the partition entries of a table, from [`entries`].
///
/// It yields each partition entry in order. At the end of the list it stops;
/// at a malformed or unreadable entry it yields the error and then stops.
/// [`end`](Self::end) then says which of the two it was.
pub struct Entries<'s, S: ?Sized> {
    source: &'s S,
    offset: u32,
    index: usize,
    state: State,
}

#[derive(Clone, Copy)]
enum State {
    Reading,
    Done(Result<End, TableError>),
}

impl<S: Source + ?Sized> Entries<'_, S> {
    /// How the table ended: `None` while there are entries left to read, then
    /// the end of a well-formed list or the reason the table isn't well formed.
    #[must_use]
    pub fn end(&self) -> Option<Result<End, TableError>> {
        match self.state {
            State::Reading => None,
            State::Done(end) => Some(end),
        }
    }

    /// Reads the remaining entries and returns how the table ended.
    ///
    /// # Errors
    ///
    /// The reason the table isn't well formed, as [`end`](Self::end) gives it.
    pub fn finish(&mut self) -> Result<End, TableError> {
        loop {
            if let State::Done(end) = self.state {
                return end;
            }
            self.next();
        }
    }

    fn read_slot(&self) -> Result<Slot, TableError> {
        let index = self.index;
        let read_error = |error| TableError::Read { index, error };
        let offset = u32::try_from(index)
            .ok()
            .and_then(|i| i.checked_mul(ENTRY_LEN))
            .and_then(|at| self.offset.checked_add(at))
            .ok_or(read_error(ReadError::OutOfRange))?;
        let bytes = read_array(self.source, offset).map_err(read_error)?;
        parse_slot(&bytes).map_err(|magic| TableError::Magic { index, magic })
    }
}

impl<S: Source + ?Sized> Iterator for Entries<'_, S> {
    type Item = Result<RawEntry, TableError>;

    fn next(&mut self) -> Option<Self::Item> {
        if !matches!(self.state, State::Reading) {
            return None;
        }
        let index = self.index;
        let end = if index >= MAX_ENTRIES {
            Err(TableError::Unterminated)
        } else {
            match self.read_slot() {
                Ok(Slot::Partition(entry)) => {
                    self.index += 1;
                    return Some(Ok(entry));
                }
                Ok(Slot::Md5(digest)) => Ok(End::Md5 { index, digest }),
                Ok(Slot::Erased) if index == 0 => Err(TableError::Empty),
                Ok(Slot::Erased) => Ok(End::Erased { index }),
                Err(error) => Err(error),
            }
        };
        self.state = State::Done(end);
        end.err().map(Err)
    }
}

impl<S: ?Sized> fmt::Debug for Entries<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entries")
            .field("offset", &self.offset)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// How one expected partition compares with the table entry at its position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// The entry equals the partition in every field.
    Matches,
    /// The entry differs from the partition in these fields.
    Differs(Fields),
    /// The table has no entry at this position: the list ends before it, or
    /// the table isn't well formed up to it.
    Missing,
}

/// A table compared with the partitions it is expected to hold, position by
/// position: the table's entry `i` against expected partition `i`.
///
/// It holds no copy of the entries; [`entries`] reads them from the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comparison<const N: usize> {
    /// Per expected partition, in order, how it compares.
    pub partitions: [Match; N],
    /// The number of entries past the last expected partition. When it isn't
    /// zero, the first of them is entry `N`.
    pub extra: usize,
    /// How the table ends, or why it isn't well formed.
    pub end: Result<End, TableError>,
}

impl<const N: usize> Comparison<N> {
    /// Whether the table is well formed and holds exactly the expected
    /// partitions, in order.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.end.is_ok() && self.extra == 0 && self.partitions.iter().all(|m| *m == Match::Matches)
    }
}

/// The comparison of the board's table with the generated layout.
pub type LayoutComparison = Comparison<{ layout::PARTITIONS.len() }>;

/// Compares the table at `offset` in `source` with `expected`: every field of
/// every entry, in order, and that the list then ends with an MD5 or erased
/// entry. What follows the end is ignored.
pub fn compare<S: Source + ?Sized, const N: usize>(
    source: &S,
    offset: u32,
    expected: &[Entry; N],
) -> Comparison<N> {
    let mut partitions = [Match::Missing; N];
    let mut extra = 0;
    let mut entries = entries(source, offset);
    for (index, entry) in (&mut entries).enumerate() {
        let Ok(entry) = entry else { break };
        match (partitions.get_mut(index), expected.get(index)) {
            (Some(result), Some(expected)) => {
                let fields = entry.differences(expected);
                *result = if fields.is_empty() {
                    Match::Matches
                } else {
                    Match::Differs(fields)
                };
            }
            _ => extra += 1,
        }
    }
    Comparison {
        partitions,
        extra,
        end: entries.finish(),
    }
}

/// Compares the table at [`layout::TABLE_OFFSET`] in `flash`, a source that
/// starts at the start of the flash, with [`layout::PARTITIONS`].
pub fn compare_layout<S: Source + ?Sized>(flash: &S) -> LayoutComparison {
    compare(flash, layout::TABLE_OFFSET, &layout::PARTITIONS)
}

/// Finds the table a bootloader that booted the app at `booted` read, in
/// `flash`, a source that starts at the start of the flash.
///
/// Looks at each sector from the second (the first holds the bootloader) up
/// to the one below `booted`, and returns the offset of the first that holds a
/// well-formed table with an app entry starting exactly at `booted`. Tables
/// without one are skipped, as are sectors that can't be read. `None` when no
/// sector holds such a table, and for a `booted` that isn't a multiple of
/// [`SECTOR_SIZE`], where no partition can start.
#[must_use]
pub fn find<S: Source + ?Sized>(flash: &S, booted: u32) -> Option<u32> {
    if !booted.is_multiple_of(SECTOR_SIZE) {
        return None;
    }
    (1..booted / SECTOR_SIZE)
        .map(|sector| sector * SECTOR_SIZE)
        .find(|&offset| boots(flash, offset, booted))
}

/// Whether the table at `offset` is well formed and has an app entry starting
/// at `booted`.
fn boots<S: Source + ?Sized>(flash: &S, offset: u32, booted: u32) -> bool {
    let mut entries = entries(flash, offset);
    let mut found = false;
    for entry in &mut entries {
        match entry {
            Ok(entry) => found |= entry.kind == TYPE_APP && entry.offset == booted,
            Err(_) => return false,
        }
    }
    found && entries.finish().is_ok()
}

#[cfg(test)]
mod tests;
