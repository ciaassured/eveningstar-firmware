//! The ESP-IDF partition table: judging it as the bootloader does, the
//! partitions the bootloader loads from it, comparing them with the
//! partitions expected, and finding the table at another offset.
//!
//! The table is a list of 32-byte entries starting at the table offset, at
//! most [`MAX_LEN`] bytes long. Each entry is the magic `0x50AA` (`u16`), the
//! type and subtype (`u8` each), the offset and size (`u32`), the label (16
//! bytes, NUL padded) and the flags (`u32`), little-endian. An MD5 entry
//! (magic `0xEBEB`, with the MD5 of the preceding entries in its last 16
//! bytes) may follow them, and an end entry (magic, type and subtype all
//! `0xFF`) ends the list.
//!
//! [`verify`] judges a table as ESP-IDF 5.5's bootloader does
//! (`esp_partition_table_verify`), and no more strictly: every partition entry
//! inside the chip size, compared in wrapping 32-bit arithmetic as the
//! bootloader compares it; at most one MD5 entry, whose digest has to be the
//! MD5 of the entries before it; and an end entry that isn't the first. What
//! the end entry holds past its magic, type and subtype, the MD5 entry's
//! bytes between its magic and its digest, and everything after the end
//! entry are never read. [`partitions`] then reads the entries the
//! bootloader loads as partitions (`bootloader_utility_load_partition_table`):
//! the first ones, up to the end entry, less one when the table has an MD5
//! entry. With entries between the MD5 entry and the end entry, those
//! include the MD5 entry, read as a partition whatever its magic, and leave
//! out the last entry, as the bootloader does. [`Loaded`] keeps the ones the
//! bootloader boots from and reads the otadata from, the last of each kind.
//!
//! [`compare_labels`] compares the partitions with the expected ones by
//! label, and [`find`] looks for the table a bootloader read at an offset
//! other than the expected one, judging each candidate as [`verify`] does.
//!
//! [`entries`] and [`compare`] read the list as the first design did: it ends
//! at the MD5 entry, whose digest is reported in [`End::Md5`] and decides
//! nothing else, and the entries are compared by position.

use core::fmt;
use core::ops::BitOr;

use crate::md5::Md5;
use crate::source::{Source, read_array};
use crate::{Entry, ReadError, TYPE_APP, TYPE_DATA, Text};

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
/// the partition expected.
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
    /// At an MD5 entry, the entry at `index`, holding `digest`.
    Md5 {
        /// Position of the MD5 entry, which is also the number of partitions.
        index: usize,
        /// The digest the entry stores.
        digest: [u8; 16],
        /// Whether `digest` is the MD5 of the 32 bytes of each entry before
        /// it, as stored.
        matches: bool,
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

/// Why a table isn't valid: what makes the bootloader refuse it, from
/// [`verify`], or why it isn't well formed, from [`entries`].
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
    /// [`MAX_ENTRIES`] entries and no end entry among them ([`verify`]), or
    /// no MD5 or erased entry ([`entries`]).
    Unterminated,
    /// The partition entry at `index` starts or ends past the chip size, as
    /// the bootloader compares them: `offset > chip size` or
    /// `offset + size > chip size`, the sum wrapping at 2^32.
    PastChip {
        /// Position of the entry.
        index: usize,
        /// The offset it stores.
        offset: u32,
        /// The size it stores.
        size: u32,
    },
    /// The entry at `index` is a second MD5 entry.
    SecondMd5 {
        /// Position of the entry.
        index: usize,
    },
    /// The digest of the MD5 entry at `index` isn't the MD5 of the entries
    /// before it.
    Md5 {
        /// Position of the MD5 entry.
        index: usize,
        /// The digest it stores.
        stored: [u8; 16],
        /// The MD5 of the entries before it.
        computed: [u8; 16],
    },
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
        md5: Md5::new(),
    }
}

/// An iterator over the partition entries of a table, from [`entries`].
///
/// It yields each partition entry in order. At the end of the list it stops;
/// at a malformed or unreadable entry it yields the error and then stops.
/// [`end`](Self::end) then says which of the two it was. It hashes each entry
/// as it reads it, so an MD5 entry is checked without reading the table again.
pub struct Entries<'s, S: ?Sized> {
    source: &'s S,
    offset: u32,
    index: usize,
    state: State,
    /// The MD5 of the entries read so far.
    md5: Md5,
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

    /// The slot at `index`, a partition entry's bytes hashed into `md5`.
    fn read_slot(&mut self) -> Result<Slot, TableError> {
        let index = self.index;
        let read_error = |error| TableError::Read { index, error };
        let offset = u32::try_from(index)
            .ok()
            .and_then(|i| i.checked_mul(ENTRY_LEN))
            .and_then(|at| self.offset.checked_add(at))
            .ok_or(read_error(ReadError::OutOfRange))?;
        let bytes = read_array(self.source, offset).map_err(read_error)?;
        let slot = parse_slot(&bytes).map_err(|magic| TableError::Magic { index, magic })?;
        if let Slot::Partition(_) = slot {
            self.md5.update(&bytes);
        }
        Ok(slot)
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
                Ok(Slot::Md5(digest)) => Ok(End::Md5 {
                    index,
                    digest,
                    matches: self.md5.clone().finalize() == digest,
                }),
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

/// How one expected partition compares with the table's entry with its label
/// ([`compare_labels`]), or at its position ([`compare`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// partitions, in order, whatever its MD5.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.end.is_ok() && self.extra == 0 && self.partitions.iter().all(|m| *m == Match::Matches)
    }
}

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

/// The app subtype of a factory app.
const SUBTYPE_FACTORY: u8 = 0x00;
/// The app subtype of a test app.
const SUBTYPE_TEST: u8 = 0x20;
/// The app subtypes of the OTA slots: `0x10` for `ota_0` up to `0x1F` for
/// `ota_15`.
const SUBTYPE_OTA: u8 = 0x10;
/// The data subtype of the otadata.
const SUBTYPE_DATA_OTA: u8 = 0x00;

/// The number of OTA app slots the bootloader keeps, `ota_0` to `ota_15`.
pub const OTA_SLOTS: usize = 16;

/// An MD5 entry of a valid table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Md5Entry {
    /// Position of the entry.
    pub index: usize,
    /// The digest it stores, which is the MD5 of the entries before it.
    pub digest: [u8; 16],
}

/// A table the bootloader accepts, from [`verify`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Verified {
    /// Position of the end entry: the number of entries before it, the MD5
    /// entry included.
    pub end: usize,
    /// The MD5 entry, when the table has one.
    pub md5: Option<Md5Entry>,
}

impl Verified {
    /// The number of entries the bootloader loads as partitions: those before
    /// the end entry, less one when the table has an MD5 entry.
    #[must_use]
    pub const fn partitions(&self) -> usize {
        match self.md5 {
            Some(_) => self.end.saturating_sub(1),
            None => self.end,
        }
    }

    /// Whether the bootloader reads the MD5 entry as a partition: entries lie
    /// between it and the end entry, so it is one of the first
    /// [`partitions`](Self::partitions) entries, and the last entry before
    /// the end entry is left out.
    #[must_use]
    pub const fn md5_read_as_partition(&self) -> bool {
        match self.md5 {
            Some(md5) => md5.index < self.partitions(),
            None => false,
        }
    }
}

/// The 32 bytes of the entry at `index` of the table at `offset`.
fn read_entry<S: Source + ?Sized>(
    source: &S,
    offset: u32,
    index: usize,
) -> Result<[u8; 32], TableError> {
    let read_error = |error| TableError::Read { index, error };
    let at = u32::try_from(index)
        .ok()
        .and_then(|i| i.checked_mul(ENTRY_LEN))
        .and_then(|at| offset.checked_add(at))
        .ok_or(read_error(ReadError::OutOfRange))?;
    read_array(source, at).map_err(read_error)
}

/// Judges the table at `offset` in `source` as the ESP-IDF 5.5 bootloader
/// does (`esp_partition_table_verify`), for a flash whose chip size is
/// `chip_size` bytes, the size the bootloader takes from its header's flash
/// size code ([`image::flash_size`](crate::image::flash_size)).
///
/// The entries are read one at a time, from the first, until the end entry:
///
/// - a partition entry (magic `0x50AA`) has to start and end inside the chip
///   size, compared as the bootloader compares them, the end wrapping at
///   2^32, so an entry whose end wraps to a small number passes;
/// - an MD5 entry (magic `0xEBEB`) has to be the first, and its last 16
///   bytes the MD5 of the entries before it; the 14 bytes after its magic
///   aren't read;
/// - an entry whose magic, type and subtype are `0xFF` ends the table, unless
///   it is the first; the 28 bytes after them aren't read;
/// - any other entry makes the table invalid, as does reaching
///   [`MAX_ENTRIES`] entries without an end entry.
///
/// Nothing after the end entry is read.
///
/// # Errors
///
/// The first reason the bootloader refuses the table, as a [`TableError`]:
/// [`PastChip`](TableError::PastChip), [`SecondMd5`](TableError::SecondMd5),
/// [`Md5`](TableError::Md5), [`Empty`](TableError::Empty),
/// [`Magic`](TableError::Magic), [`Unterminated`](TableError::Unterminated),
/// or [`Read`](TableError::Read) when reading an entry fails.
pub fn verify<S: Source + ?Sized>(
    source: &S,
    offset: u32,
    chip_size: u32,
) -> Result<Verified, TableError> {
    let mut judge = Judge {
        chip_size,
        md5: Md5::new(),
        found: None,
    };
    for index in 0..MAX_ENTRIES {
        let bytes = read_entry(source, offset, index)?;
        if let Some(verified) = judge.entry(index, &bytes)? {
            return Ok(verified);
        }
    }
    Err(TableError::Unterminated)
}

/// The judgement [`verify`] makes, one entry at a time.
struct Judge {
    chip_size: u32,
    /// The MD5 of the partition entries before the MD5 entry.
    md5: Md5,
    /// The MD5 entry, once read.
    found: Option<Md5Entry>,
}

impl Judge {
    /// Judges the entry at `index`, stored in `bytes`: the table, when it is
    /// the end entry, else `None` to read on.
    fn entry(&mut self, index: usize, bytes: &[u8; 32]) -> Result<Option<Verified>, TableError> {
        let [m0, m1, kind, subtype, ..] = *bytes;
        match u16::from_le_bytes([m0, m1]) {
            MAGIC => self.partition(index, bytes).map(|()| None),
            MD5_MAGIC => self.md5_entry(index, bytes).map(|()| None),
            0xFFFF if kind == ERASED && subtype == ERASED => {
                if index == 0 {
                    return Err(TableError::Empty);
                }
                Ok(Some(Verified {
                    end: index,
                    md5: self.found,
                }))
            }
            magic => Err(TableError::Magic { index, magic }),
        }
    }

    /// Checks the partition entry at `index` against the chip size, and
    /// hashes it while no MD5 entry has been read: the entries before the
    /// MD5 entry are all partition entries, which its digest covers.
    fn partition(&mut self, index: usize, bytes: &[u8; 32]) -> Result<(), TableError> {
        let entry = RawEntry::from_bytes(bytes);
        let chip_size = self.chip_size;
        if entry.offset > chip_size || entry.offset.wrapping_add(entry.size) > chip_size {
            return Err(TableError::PastChip {
                index,
                offset: entry.offset,
                size: entry.size,
            });
        }
        if self.found.is_none() {
            self.md5.update(bytes);
        }
        Ok(())
    }

    /// Checks the MD5 entry at `index`: the first, with the digest of the
    /// entries before it. Never inlined, so the copy of the hash stays out of
    /// the caller's frame.
    #[inline(never)]
    fn md5_entry(&mut self, index: usize, bytes: &[u8; 32]) -> Result<(), TableError> {
        if self.found.is_some() {
            return Err(TableError::SecondMd5 { index });
        }
        let [_, _, _, _, _, _, _, _, _, _, _, _, _, _, _, _, stored @ ..] = *bytes;
        let computed = self.md5.clone().finalize();
        if computed != stored {
            return Err(TableError::Md5 {
                index,
                stored,
                computed,
            });
        }
        self.found = Some(Md5Entry {
            index,
            digest: stored,
        });
        Ok(())
    }
}

/// The entries the bootloader loads as partitions from the table at `offset`
/// in `source`, which [`verify`] judged `verified`: the first
/// [`Verified::partitions`] entries, each read as a partition whatever its
/// magic, as `bootloader_utility_load_partition_table` reads them.
pub fn partitions<'s, S: Source + ?Sized>(
    source: &'s S,
    offset: u32,
    verified: &Verified,
) -> Partitions<'s, S> {
    Partitions {
        source,
        offset,
        index: 0,
        count: verified.partitions(),
    }
}

/// An iterator over the partitions the bootloader loads, from
/// [`partitions`]: each with its position in the table, or the error reading
/// it.
pub struct Partitions<'s, S: ?Sized> {
    source: &'s S,
    offset: u32,
    index: usize,
    count: usize,
}

impl<S: Source + ?Sized> Iterator for Partitions<'_, S> {
    type Item = Result<(usize, RawEntry), TableError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count {
            return None;
        }
        let index = self.index;
        self.index += 1;
        Some(
            read_entry(self.source, self.offset, index)
                .map(|bytes| (index, RawEntry::from_bytes(&bytes))),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.count.saturating_sub(self.index);
        (left, Some(left))
    }
}

impl<S: ?Sized> fmt::Debug for Partitions<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Partitions")
            .field("offset", &self.offset)
            .field("index", &self.index)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// A partition the bootloader keeps: its position in the table, and the
/// offset and size it stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Kept {
    /// Position of the entry in the table.
    pub index: usize,
    /// Offset from the start of the flash, in bytes.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
}

/// The partitions the bootloader boots from and reads the otadata from, as
/// `bootloader_utility_load_partition_table` keeps them while it reads the
/// partitions: of several of one kind, the last.
///
/// A bootloader built without `CONFIG_SECURE_ENABLE_TEE`, as the Nix one is,
/// keeps no TEE partitions: their subtypes are other apps and other data
/// here, which it doesn't boot from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Loaded {
    /// The factory app (type app, subtype `0x00`).
    pub factory: Option<Kept>,
    /// The test app (type app, subtype `0x20`).
    pub test: Option<Kept>,
    /// The OTA apps, by slot number (type app, subtype `0x10 + n`).
    pub ota: [Option<Kept>; OTA_SLOTS],
    /// The otadata (type data, subtype `0x00`).
    pub otadata: Option<Kept>,
}

impl Loaded {
    /// No partition kept yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            factory: None,
            test: None,
            ota: [None; OTA_SLOTS],
            otadata: None,
        }
    }

    /// Keeps `entry`, the partition at `index`, where the bootloader keeps
    /// it, replacing the one kept there before; an entry of another kind is
    /// left out.
    pub fn keep(&mut self, index: usize, entry: &RawEntry) {
        let kept = Some(Kept {
            index,
            offset: entry.offset,
            size: entry.size,
        });
        let place = match (entry.kind, entry.subtype) {
            (TYPE_APP, SUBTYPE_FACTORY) => &mut self.factory,
            (TYPE_APP, SUBTYPE_TEST) => &mut self.test,
            (TYPE_APP, subtype) if subtype & !0x0F == SUBTYPE_OTA => {
                match self.ota.get_mut(usize::from(subtype & 0x0F)) {
                    Some(place) => place,
                    None => return,
                }
            }
            (TYPE_DATA, SUBTYPE_DATA_OTA) => &mut self.otadata,
            _ => return,
        };
        *place = kept;
    }

    /// The app partitions kept: the factory app, the test app, then each OTA
    /// app by slot.
    pub fn apps(&self) -> impl Iterator<Item = Kept> + '_ {
        [&self.factory, &self.test]
            .into_iter()
            .chain(&self.ota)
            .filter_map(|kept| *kept)
    }
}

/// Fills `loaded`, in place of what it held, with the partitions the
/// bootloader keeps from the table at `offset` in `source`, which [`verify`]
/// judged `verified`. Filled in place, it stays out of the function's frame.
///
/// # Errors
///
/// [`TableError::Read`] when reading a partition fails, after which `loaded`
/// holds what was kept from the partitions before it.
pub fn load<S: Source + ?Sized>(
    loaded: &mut Loaded,
    source: &S,
    offset: u32,
    verified: &Verified,
) -> Result<(), TableError> {
    *loaded = Loaded::new();
    for partition in partitions(source, offset, verified) {
        let (index, entry) = partition?;
        loaded.keep(index, &entry);
    }
    Ok(())
}

/// The partitions of a table compared with the partitions expected, by
/// label, from [`compare_labels`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LabelComparison<const N: usize> {
    /// Per expected partition, in order, how the table's partition with its
    /// label compares: [`Match::Differs`] never names the label.
    pub expected: [Match; N],
    /// The number of the table's partitions that no expected partition has
    /// the label of, or whose label an earlier partition of the table has.
    pub extra: usize,
}

impl<const N: usize> LabelComparison<N> {
    /// Whether the table's partitions are exactly the expected ones, field by
    /// field, in any order.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.extra == 0 && self.expected.iter().all(|m| *m == Match::Matches)
    }
}

/// Compares `partitions` with `expected` by label: each expected partition
/// with the first partition whose label (all 16 bytes) is its label, field by
/// field. The partitions with no expected label, and those whose label an
/// earlier one had, are extra.
pub fn compare_labels<const N: usize>(
    partitions: impl IntoIterator<Item = RawEntry>,
    expected: &[Entry; N],
) -> LabelComparison<N> {
    let mut result = [Match::Missing; N];
    let mut extra = 0_usize;
    for entry in partitions {
        let position = expected
            .iter()
            .position(|expected| entry.label.is_padded(expected.label));
        match position.and_then(|i| Some((result.get_mut(i)?, expected.get(i)?))) {
            Some((slot @ Match::Missing, expected)) => {
                let fields = entry.differences(expected);
                *slot = if fields.is_empty() {
                    Match::Matches
                } else {
                    Match::Differs(fields)
                };
            }
            _ => extra = extra.saturating_add(1),
        }
    }
    LabelComparison {
        expected: result,
        extra,
    }
}

/// The tables [`find`] found that a bootloader could have read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Found {
    /// No sector holds one.
    Nothing,
    /// One sector does, at this offset: the table the bootloader read.
    One(u32),
    /// Several sectors do, the first two at these offsets. The bootloader
    /// read one of them, and nothing on the flash says which.
    Several(u32, u32),
}

impl Found {
    /// The offset of the table the bootloader read, when exactly one was
    /// found.
    #[must_use]
    pub const fn offset(self) -> Option<u32> {
        match self {
            Self::One(offset) => Some(offset),
            Self::Nothing | Self::Several(..) => None,
        }
    }
}

/// Finds the table a bootloader that booted the app at `booted` read, in
/// `flash`, a source that starts at the start of the flash, for a chip size
/// of `chip_size` bytes.
///
/// Looks at each sector from the second (the first holds the bootloader) up
/// to the one below `booted` for a table that [`verify`] accepts and of whose
/// [`partitions`] one is an app starting exactly at `booted`. Other sectors
/// are skipped, as are those that can't be read. A table left over from an
/// earlier layout qualifies as well as the one the bootloader read, so a
/// second table is [`Found::Several`], not a choice. [`Found::Nothing`] for a
/// `booted` that isn't a multiple of [`SECTOR_SIZE`], where no partition can
/// start.
#[must_use]
pub fn find<S: Source + ?Sized>(flash: &S, booted: u32, chip_size: u32) -> Found {
    if !booted.is_multiple_of(SECTOR_SIZE) {
        return Found::Nothing;
    }
    let mut tables = (1..booted / SECTOR_SIZE)
        .map(|sector| sector * SECTOR_SIZE)
        .filter(|&offset| boots(flash, offset, booted, chip_size));
    match (tables.next(), tables.next()) {
        (None, _) => Found::Nothing,
        (Some(offset), None) => Found::One(offset),
        (Some(first), Some(second)) => Found::Several(first, second),
    }
}

/// Whether the table at `offset` is valid for `chip_size` and one of its
/// partitions is an app starting at `booted`.
fn boots<S: Source + ?Sized>(flash: &S, offset: u32, booted: u32, chip_size: u32) -> bool {
    let Ok(verified) = verify(flash, offset, chip_size) else {
        return false;
    };
    partitions(flash, offset, &verified)
        .any(|partition| partition.is_ok_and(|(_, e)| e.kind == TYPE_APP && e.offset == booted))
}

#[cfg(test)]
mod tests;
