//! The partition table: the table the chain located, judged as the
//! bootloader judges it, beside the layout the firmware expects.
//!
//! [`PartitionTable`] owns the reader of the compiled table's sector and, in
//! the unverified tier, the reader of the table the search found alone. The
//! table it parses is the located one: at the offset a known bootloader
//! reads, or the one the search found. Its model ([`TableModel`]) holds the
//! table as the bootloader judges it, the partitions the bootloader loads
//! from it ([`PartitionTable::partitions`]), its MD5 entry, and their
//! comparison with the layout by label; [`Location`] says how it was
//! located, and [`EXPECTED`] gives the layout in the same shape.
//!
//! A byte of the table is published only when a check covers it
//! ([`Coverage`]):
//!
//! - in the layout and boot tiers, the entries equal the compiled layout's;
//! - in the unverified tier, the table's MD5 entry covers the entries before
//!   it;
//! - a table with no MD5 entry in the unverified tier publishes no bytes and
//!   no labels: each partition's type, subtype, offset, size and flags are
//!   still given, as they say where regions lie, not what they hold.
//!
//! The view is the entries, the MD5 entry and the end entry, as the
//! bootloader read them. The bytes the bootloader ignores between those it
//! reads, the MD5 entry's 14 bytes between its magic and its digest and the
//! end entry's 28 bytes after its magic, type and subtype, are served as
//! `0xFF`, and listed as sanitized when the flash holds something else there.
//! Entries between the MD5 entry and the end entry, which the bootloader
//! accepts but the MD5 doesn't cover, end the view after the MD5 entry. When
//! the search finds several tables, none is located: their offsets are
//! given, and neither their entries nor their bytes.
//!
//! Only the module's parser makes a model:
//!
//! ```compile_fail,E0451
//! use eveningstar_boot::table::{Coverage, TableModel};
//!
//! fn forge(model: TableModel) -> TableModel {
//!     TableModel { offset: 0, ..model }
//! }
//! ```

use eveningstar_esp_formats::hash::Sha256Hasher;
use eveningstar_esp_formats::pace::Pace;
use eveningstar_esp_formats::table::{
    self as format, ENTRY_LEN, Found, Label, Md5Entry, RawEntry, TableError, Verified,
};
use eveningstar_storage::chain::{LayoutComparison, Report, TableSource, Tier};
use eveningstar_storage::nor::{Published, Reader, Snapshot};
use eveningstar_storage::roles::{self, FoundTable, Internal, Publishable};
use eveningstar_storage::{
    Entry, Epoch, PublicRead as _, ReadError, SECTOR_SIZE, Source, Span, ViewError, layout,
};

use crate::digest::{Digest, digest};
use crate::error::{LoadError, Unpublished, at, publish, whole};
use crate::pace::Progress;

/// The layout the firmware expects, in the shape of the found partitions:
/// the bootloader's and the table's regions, and the partitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    /// The bootloader's region: from 0x0 up to the compiled table offset.
    pub bootloader: Span,
    /// The table's region: the compiled table's sector.
    pub table: Span,
    /// The partitions, in the table's order.
    pub partitions: &'static [Entry],
}

/// The layout the firmware expects.
pub const EXPECTED: Expected = Expected {
    bootloader: Span::new(0, layout::TABLE_OFFSET),
    table: Span::new(layout::TABLE_OFFSET, SECTOR_SIZE),
    partitions: &layout::PARTITIONS,
};

/// How the chain located the table, or didn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    source: Option<TableSource>,
    offset: Option<u32>,
    search: Option<Found>,
}

impl Location {
    /// How the table was located, `None` when it wasn't.
    #[must_use]
    pub const fn source(&self) -> Option<TableSource> {
        self.source
    }

    /// The located table's offset, from the start of the flash.
    #[must_use]
    pub const fn offset(&self) -> Option<u32> {
        self.offset
    }

    /// What the search found, when it ran: [`Found::Several`] gives the
    /// first two candidates' offsets, of which neither is located.
    #[must_use]
    pub const fn search(&self) -> Option<Found> {
        self.search
    }
}

/// What covers the table's bytes, and so what of it is published.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Coverage {
    /// The partitions equal the compiled layout's, at the compiled offset, in
    /// the layout or boot tier.
    Layout,
    /// The MD5 entry covers the entries before it.
    Md5,
    /// Nothing does: no MD5 entry, below the layout tier. No byte and no
    /// label is published.
    Uncovered,
}

/// Why the bootloader refuses the table, as [`TableError`] gives it, without
/// the values it read from the table refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableFailure {
    /// The entry at `index` is neither a partition, the MD5 entry nor the end
    /// entry.
    Magic {
        /// Position of the entry.
        index: usize,
    },
    /// The first entry is erased: there is no table.
    Empty,
    /// No end entry among the most entries a table has.
    Unterminated,
    /// The partition entry at `index` starts or ends past the chip size.
    PastChip {
        /// Position of the entry.
        index: usize,
    },
    /// The entry at `index` is a second MD5 entry.
    SecondMd5 {
        /// Position of the entry.
        index: usize,
    },
    /// The MD5 entry at `index` doesn't hold the MD5 of the entries before
    /// it.
    Md5 {
        /// Position of the MD5 entry.
        index: usize,
    },
    /// Reading the entry at `index` failed.
    Read {
        /// Position of the entry.
        index: usize,
        /// Why the read failed.
        error: ReadError,
    },
}

impl From<TableError> for TableFailure {
    fn from(error: TableError) -> Self {
        match error {
            TableError::Magic { index, .. } => Self::Magic { index },
            TableError::Empty => Self::Empty,
            TableError::Unterminated => Self::Unterminated,
            TableError::PastChip { index, .. } => Self::PastChip { index },
            TableError::SecondMd5 { index } => Self::SecondMd5 { index },
            TableError::Md5 { index, .. } => Self::Md5 { index },
            TableError::Read { index, error } => Self::Read { index, error },
        }
    }
}

/// A table the bootloader accepts, as of the model's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Valid {
    verified: Verified,
    coverage: Coverage,
    comparison: LayoutComparison,
}

impl Valid {
    /// Position of the end entry: the number of entries before it, the MD5
    /// entry included.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.verified.end
    }

    /// The number of entries the bootloader loads as partitions.
    #[must_use]
    pub const fn partitions(&self) -> usize {
        self.verified.partitions()
    }

    /// Whether the bootloader reads the MD5 entry as a partition, as entries
    /// lie between it and the end entry, and leaves out the last entry.
    #[must_use]
    pub const fn md5_read_as_partition(&self) -> bool {
        self.verified.md5_read_as_partition()
    }

    /// The position of the MD5 entry, and the digest it stores, which is the
    /// MD5 of the entries before it as computed.
    #[must_use]
    pub const fn md5(&self) -> Option<Md5Entry> {
        self.verified.md5
    }

    /// What covers the table's bytes.
    #[must_use]
    pub const fn coverage(&self) -> Coverage {
        self.coverage
    }

    /// The partitions the bootloader loads, compared with the layout by
    /// label.
    #[must_use]
    pub const fn comparison(&self) -> &LayoutComparison {
        &self.comparison
    }

    /// Whether the label of the entry at `index` is covered.
    const fn label_covered(&self, index: usize) -> bool {
        match (self.coverage, self.verified.md5) {
            (Coverage::Uncovered, _) => false,
            (_, Some(md5)) => index < md5.index,
            (_, None) => true,
        }
    }

    /// The view's span and its sanitized spans, relative to the table;
    /// `None` when nothing is covered.
    fn view(&self) -> Option<(Span, [Span; 2], usize)> {
        let at = |index: usize, offset: u32| {
            u32::try_from(index)
                .ok()?
                .checked_mul(ENTRY_LEN)?
                .checked_add(offset)
        };
        let end = self.verified.end;
        let end_tail = Span::new(at(end, 4)?, ENTRY_LEN - 4);
        let up_to = |index: usize| Some(Span::new(0, at(index.checked_add(1)?, 0)?));
        match (self.coverage, self.verified.md5) {
            (Coverage::Uncovered, _) | (Coverage::Md5, None) => None,
            (_, Some(md5)) => {
                let padding = Span::new(at(md5.index, 2)?, 14);
                if md5.index.checked_add(1) == Some(end) {
                    Some((up_to(end)?, [padding, end_tail], 2))
                } else {
                    // Entries between the MD5 entry and the end entry, which
                    // the MD5 doesn't cover: the view ends after the MD5
                    // entry.
                    Some((up_to(md5.index)?, [padding, Span::default()], 1))
                }
            }
            (Coverage::Layout, None) => Some((up_to(end)?, [end_tail, Span::default()], 1)),
        }
    }
}

/// The located table, as of one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableModel {
    epoch: Epoch,
    offset: u32,
    verdict: Result<Valid, TableFailure>,
}

impl TableModel {
    /// The region's epoch the model was parsed at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The table's offset, from the start of the flash.
    #[must_use]
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// The table, valid as the bootloader judges it, or why it isn't.
    ///
    /// # Errors
    ///
    /// The first reason the bootloader refuses the table.
    pub const fn verdict(&self) -> Result<&Valid, TableFailure> {
        match &self.verdict {
            Ok(valid) => Ok(valid),
            Err(failure) => Err(*failure),
        }
    }
}

/// A partition the bootloader loads, as the table stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoundPartition {
    index: usize,
    label: Option<Label>,
    kind: u8,
    subtype: u8,
    offset: u32,
    size: u32,
    flags: u32,
}

impl FoundPartition {
    /// Position of the entry in the table.
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    /// The label, when a check covers it ([`Coverage`]).
    #[must_use]
    pub const fn label(&self) -> Option<&Label> {
        self.label.as_ref()
    }

    /// Partition type.
    #[must_use]
    pub const fn kind(&self) -> u8 {
        self.kind
    }

    /// Partition subtype.
    #[must_use]
    pub const fn subtype(&self) -> u8 {
        self.subtype
    }

    /// Offset from the start of the flash, in bytes.
    #[must_use]
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// Size in bytes.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// Flag bits.
    #[must_use]
    pub const fn flags(&self) -> u32 {
        self.flags
    }
}

/// One of the entries the bootloader loads as partitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionRow {
    /// A partition entry.
    Partition(FoundPartition),
    /// The MD5 entry, which the bootloader reads as a partition when entries
    /// lie between it and the end entry. Its type, subtype, offset and size
    /// lie in the bytes the view sanitizes, so they aren't given.
    Md5 {
        /// Position of the entry.
        index: usize,
    },
}

/// The reader of the located table.
#[derive(Debug)]
enum Located<'f> {
    /// The compiled table's sector, which a known bootloader reads.
    Compiled(Reader<'f, roles::PartitionTable>),
    /// The sector of the table the search found alone.
    Found(Reader<'f, FoundTable>),
    /// None: no table is located, or no reader covers it.
    None,
}

/// The partition table's content module.
#[derive(Debug)]
pub struct PartitionTable<'f> {
    located: Located<'f>,
    location: Location,
    tier: Tier,
    chip_size: Option<u32>,
}

impl<'f> PartitionTable<'f> {
    /// The module of the table `report` describes, with `compiled`, the
    /// compiled table's sector, and `found`, the table the search found.
    /// The located table is read through the one whose region starts at the
    /// located offset; the other is dropped.
    #[must_use]
    pub fn new(
        compiled: Option<Reader<'f, roles::PartitionTable>>,
        found: Option<Reader<'f, FoundTable>>,
        report: &Report,
    ) -> Self {
        let offset = report.table.offset;
        let starts = |span: Span| offset == Some(span.offset);
        let located = match (found, compiled) {
            (Some(found), _) if starts(found.span()) => Located::Found(found),
            (_, Some(compiled)) if starts(compiled.span()) => Located::Compiled(compiled),
            _ => Located::None,
        };
        Self {
            located,
            location: Location {
                source: report.table.source,
                offset,
                search: report.table.search,
            },
            tier: report.tier,
            chip_size: report.bootloader.chip_size(),
        }
    }

    /// How the table was located.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }

    /// The span of the located table's region, from the start of the flash.
    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        match &self.located {
            Located::Compiled(reader) => Some(reader.span()),
            Located::Found(reader) => Some(reader.span()),
            Located::None => None,
        }
    }

    /// Parses the located table through a snapshot.
    ///
    /// # Errors
    ///
    /// [`LoadError::Absent`] when no table is located or no reader covers it,
    /// [`LoadError::Busy`] while a write session is open on its region, and
    /// [`LoadError::Stale`] when one wrote it while it was parsed.
    pub fn parse(&self) -> Result<TableModel, LoadError> {
        let chip_size = self.chip_size.ok_or(LoadError::Absent)?;
        match &self.located {
            Located::Compiled(reader) => parse(reader, chip_size, self.tier),
            Located::Found(reader) => parse(reader, chip_size, self.tier),
            Located::None => Err(LoadError::Absent),
        }
    }

    /// The partitions the bootloader loads from the table `model`
    /// describes, read again at the model's epoch, in the table's order. A
    /// label is given only when a check covers it.
    ///
    /// # Errors
    ///
    /// [`LoadError::Absent`] when the table isn't valid, and
    /// [`LoadError::Stale`] or [`LoadError::Busy`] once the region isn't
    /// stable at the model's epoch.
    pub fn partitions(&self, model: &TableModel) -> Result<Rows<'_>, LoadError> {
        let Ok(valid) = &model.verdict else {
            return Err(LoadError::Absent);
        };
        let snapshot = match &self.located {
            Located::Compiled(reader) => at(reader, model.epoch).map(TableSnapshot::Compiled),
            Located::Found(reader) => at(reader, model.epoch).map(TableSnapshot::Found),
            Located::None => Err(LoadError::Absent),
        };
        snapshot.map(|snapshot| Rows {
            snapshot,
            valid: *valid,
            index: 0,
        })
    }

    /// The view of the table `model` describes: its covered bytes, as the
    /// module's documentation describes them.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] when the table isn't valid or nothing
    /// covers it, [`Unpublished::Stale`] once the region isn't stable at the
    /// model's epoch, and [`Unpublished::Refused`] for the storage crate's
    /// other refusals.
    pub fn published(&self, model: &TableModel) -> Result<TableView<'_>, Unpublished> {
        let Ok(valid) = &model.verdict else {
            return Err(Unpublished::Unverified);
        };
        let (span, sanitized, count) = valid.view().ok_or(Unpublished::Unverified)?;
        let sanitized = sanitized.get(..count).unwrap_or_default();
        let epoch = model.epoch;
        match &self.located {
            Located::Compiled(reader) => publish(reader, epoch, span, sanitized)
                .map(TableView::Compiled)
                .map_err(Unpublished::from),
            Located::Found(reader) => publish(reader, epoch, span, sanitized)
                .map(TableView::Found)
                .map_err(Unpublished::from),
            Located::None => Err(Unpublished::Unverified),
        }
    }
}

/// Parses the table at the start of `reader`'s region, judged for
/// `chip_size` and covered as `tier` allows.
fn parse<R: Publishable<Device = Internal>>(
    reader: &Reader<'_, R>,
    chip_size: u32,
    tier: Tier,
) -> Result<TableModel, LoadError> {
    let snapshot = whole(reader)?;
    let offset = reader.span().offset;
    let verdict = match format::verify(&snapshot, 0, chip_size) {
        Ok(verified) => judge(&snapshot, offset, verified, tier)?,
        Err(TableError::Read {
            error: ReadError::Stale,
            ..
        }) => return Err(LoadError::Stale),
        Err(error) => Err(error.into()),
    };
    Ok(TableModel {
        epoch: snapshot.epoch(),
        offset,
        verdict,
    })
}

/// The valid table at `offset` that `snapshot` reads, which
/// [`format::verify`] judged `verified`: its partitions compared with the
/// layout, and what covers it in `tier`. Never inlined, so the comparison
/// stays out of the caller's frame.
#[inline(never)]
fn judge<S: Source + ?Sized>(
    snapshot: &S,
    offset: u32,
    verified: Verified,
    tier: Tier,
) -> Result<Result<Valid, TableFailure>, LoadError> {
    let mut failed = None;
    let read = format::partitions(snapshot, 0, &verified).map_while(|partition| match partition {
        Ok((_, entry)) => Some(entry),
        Err(error) => {
            failed = Some(error);
            None
        }
    });
    let comparison = format::compare_labels(read, &layout::PARTITIONS);
    match failed {
        Some(TableError::Read {
            error: ReadError::Stale,
            ..
        }) => return Err(LoadError::Stale),
        Some(error) => return Ok(Err(error.into())),
        None => {}
    }
    let layout = matches!(tier, Tier::Layout | Tier::Boot)
        && offset == layout::TABLE_OFFSET
        && comparison.matches();
    let coverage = if layout {
        Coverage::Layout
    } else if verified.md5.is_some() {
        Coverage::Md5
    } else {
        Coverage::Uncovered
    };
    Ok(Ok(Valid {
        verified,
        coverage,
        comparison,
    }))
}

/// A snapshot of the located table's region.
enum TableSnapshot<'r> {
    Compiled(Snapshot<'r, roles::PartitionTable>),
    Found(Snapshot<'r, FoundTable>),
}

impl Source for TableSnapshot<'_> {
    fn size(&self) -> u32 {
        match self {
            Self::Compiled(snapshot) => snapshot.size(),
            Self::Found(snapshot) => snapshot.size(),
        }
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        match self {
            Self::Compiled(snapshot) => snapshot.read(offset, buf),
            Self::Found(snapshot) => snapshot.read(offset, buf),
        }
    }
}

/// The partitions the bootloader loads from a table, read one entry at a
/// time at the epoch of the model they were asked for with, from
/// [`PartitionTable::partitions`].
pub struct Rows<'r> {
    snapshot: TableSnapshot<'r>,
    valid: Valid,
    index: usize,
}

/// A table of one entry, the end entry after it: the bootloader loads its
/// entry as a partition, whatever its magic.
const ONE: Verified = Verified { end: 1, md5: None };

impl Iterator for Rows<'_> {
    type Item = Result<PartitionRow, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        if index >= self.valid.partitions() {
            return None;
        }
        self.index = index.saturating_add(1);
        if self.valid.md5().is_some_and(|md5| md5.index == index) {
            return Some(Ok(PartitionRow::Md5 { index }));
        }
        let at = u32::try_from(index)
            .ok()
            .and_then(|index| index.checked_mul(ENTRY_LEN));
        let Some(at) = at else {
            return Some(Err(ReadError::OutOfRange));
        };
        let entry = format::partitions(&self.snapshot, at, &ONE).next()?;
        Some(
            entry
                .map(|(_, entry)| self.row(index, &entry))
                .map_err(|error| match error {
                    TableError::Read { error, .. } => error,
                    _ => ReadError::OutOfRange,
                }),
        )
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.valid.partitions().saturating_sub(self.index);
        (left, Some(left))
    }
}

impl Rows<'_> {
    /// The row of `entry`, the entry at `index`.
    fn row(&self, index: usize, entry: &RawEntry) -> PartitionRow {
        PartitionRow::Partition(FoundPartition {
            index,
            label: self.valid.label_covered(index).then_some(entry.label),
            kind: entry.kind,
            subtype: entry.subtype,
            offset: entry.offset,
            size: entry.size,
            flags: entry.flags,
        })
    }
}

impl core::fmt::Debug for Rows<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rows")
            .field("valid", &self.valid)
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

/// The published view of the located table, through whichever reader reads
/// it.
#[derive(Debug)]
pub enum TableView<'r> {
    /// The compiled table's sector, which a known bootloader reads.
    Compiled(Published<'r, roles::PartitionTable>),
    /// The table the search found alone.
    Found(Published<'r, FoundTable>),
}

impl TableView<'_> {
    /// Length of the view in bytes.
    #[must_use]
    pub fn len(&self) -> u32 {
        match self {
            Self::Compiled(view) => view.len(),
            Self::Found(view) => view.len(),
        }
    }

    /// Whether the view has no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The region's epoch the view serves.
    #[must_use]
    pub fn epoch(&self) -> Epoch {
        match self {
            Self::Compiled(view) => view.epoch(),
            Self::Found(view) => view.epoch(),
        }
    }

    /// The view-relative spans served as `0xFF` that hold something else on
    /// the flash.
    #[must_use]
    pub fn sanitized(&self) -> &[Span] {
        match self {
            Self::Compiled(view) => view.sanitized(),
            Self::Found(view) => view.sanitized(),
        }
    }

    /// Fills `buf` with the view's bytes at `offset`, the sanitized ones as
    /// `0xFF` ([`PublicRead::read`](eveningstar_storage::PublicRead::read)).
    ///
    /// # Errors
    ///
    /// As [`PublicRead::read`](eveningstar_storage::PublicRead::read).
    pub async fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ViewError> {
        match self {
            Self::Compiled(view) => view.read(offset, buf).await,
            Self::Found(view) => view.read(offset, buf).await,
        }
    }

    /// The transport digest of the bytes `span` of the view ([`digest`]).
    ///
    /// # Errors
    ///
    /// As [`digest`].
    pub async fn digest<H, P, G>(
        &self,
        span: Span,
        sha: H,
        pace: &mut P,
        progress: &mut G,
    ) -> Result<Digest, ViewError>
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        match self {
            Self::Compiled(view) => digest(view, span, sha, pace, progress).await,
            Self::Found(view) => digest(view, span, sha, pace, progress).await,
        }
    }
}

#[cfg(test)]
mod tests;
