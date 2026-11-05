//! The verification chain of the internal flash, the trust tiers it decides,
//! and the handles of each tier.
//!
//! [`split`] runs the chain once per boot and never fails: the worst outcome
//! is the unverified tier. Its [`Report`] records every step.
//!
//! 1. **Bootloader.** The header at 0x0 is read whether or not the image
//!    verifies: its flash size code is the chip size the bootloader checks
//!    the table against ([`image::flash_size`]). The image is verified at
//!    the length its headers describe, within the flash, and its digest
//!    looked up in [`known::BOOTLOADERS`].
//! 2. **Table offset.** A known bootloader's entry gives the offset it reads
//!    the table from ([`TableSource::KnownBootloader`]). Otherwise the flash
//!    below the booted offset is searched, and only a single candidate
//!    counts ([`TableSource::Searched`]). The bootloader's image has to end at
//!    or before the located offset, or the compiled one when none is located;
//!    one that doesn't is reported ([`BootloaderReport::fits`]), and its
//!    region ends before its end, so it can't be published whole.
//! 3. **Table.** The table at that offset is valid as the bootloader judges
//!    it ([`table::verify`]), for the chip size of step 1.
//! 4. **Partitions.** The partitions are the ones the bootloader loads
//!    ([`table::partitions`]), compared with the layout by label.
//! 5. **Layout.** The table's offset is [`layout::TABLE_OFFSET`] and its
//!    partitions are the layout's, field by field.
//! 6. **Flash and booted slot.** The flash holds at least
//!    [`layout::FLASH_SIZE`] bytes, and the booted offset is a slot's.
//!
//! - [`Tier::Unverified`]
//!   - Reached when: step 2 locates no single table, or any of steps 3 to 6
//!     fails.
//!   - Handles: [`Storage::bootloader`], [`Storage::partition_table`], and
//!     [`Located`]: readers of what the chain located.
//! - [`Tier::Layout`]
//!   - Reached when: steps 2 to 6 pass, the table found by the search or a
//!     known bootloader without rollback.
//!   - Handles: [`Storage::bootloader`], [`Storage::partition_table`],
//!     [`LayoutHandles`]: readers of the slots and the otadata, writers of the
//!     configurations and `diag`.
//! - [`Tier::Boot`]
//!   - Reached when: steps 2 to 6 pass, the table located by a known
//!     bootloader that supports rollback.
//!   - Handles: the layout tier's, and [`BootWriters`]: writers of the
//!     alternate slot and the otadata.
//!
//! No reader of a role located at runtime ([`Located`]) is made over a span
//! that shares a byte with `factory`, a configuration or `diag` of the
//! compiled layout; such a partition is reported only.
//!
//! Each tier's handles are its variant's of [`Handles`]:
//!
//! ```
//! use eveningstar_storage::chain::{Handles, Storage};
//!
//! fn owners(storage: Storage<'_>) {
//!     match storage.handles {
//!         Handles::Unverified(located) => drop((located.table, located.app, located.otadata)),
//!         Handles::Layout(handles) => drop((handles.current_app, handles.diag)),
//!         Handles::Boot(handles, writers) => drop((handles.otadata, writers.alternate_app)),
//!     }
//! }
//! ```
//!
//! The unverified tier has no handle of the layout's regions:
//!
//! ```compile_fail,E0609
//! use eveningstar_storage::chain::Located;
//!
//! fn store(located: Located<'_>) {
//!     let _ = located.current_config;
//! }
//! ```
//!
//! The layout tier reads the alternate slot and the otadata, and writes
//! neither:
//!
//! ```compile_fail,E0308
//! use eveningstar_storage::chain::LayoutHandles;
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::AlternateApp;
//!
//! fn update(handles: LayoutHandles<'_>) -> Writer<'_, AlternateApp> {
//!     handles.alternate_app
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::chain::LayoutHandles;
//!
//! fn select(handles: &mut LayoutHandles<'_>) {
//!     let _ = handles.otadata.begin();
//! }
//! ```
//!
//! and only `split` makes the boot writers:
//!
//! ```compile_fail,E0451
//! use eveningstar_storage::chain::BootWriters;
//! use eveningstar_storage::nor::Writer;
//! use eveningstar_storage::roles::{AlternateApp, Otadata};
//! use eveningstar_storage::Slot;
//!
//! fn forge<'f>(alternate_app: Writer<'f, AlternateApp>, otadata: Writer<'f, Otadata>) -> BootWriters<'f> {
//!     BootWriters { alternate_app, otadata, slot: Slot::Ota0 }
//! }
//! ```

use eveningstar_esp_formats::image::Header;
use eveningstar_esp_formats::table::{Found, LabelComparison, TableError, Verified};

use crate::image::{self, ImageError};
use crate::known::{self, Known};
use crate::nor::{NorDevice, Reader, SECTOR_SIZE, State, Writer};
use crate::roles::{
    AlternateApp, AlternateConfig, Bootloader, CurrentApp, CurrentConfig, Diag, FoundApp,
    FoundOtadata, FoundTable, Internal, Otadata, PartitionTable, Role, is_sensitive,
};
use crate::{Entry, ReadError, Slot, SlotPartitions as _, Source, Span, layout, table};

/// The label comparison of the located table's partitions with the layout.
pub type LayoutComparison = LabelComparison<{ layout::PARTITIONS.len() }>;

/// How far the board is trusted, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// The chain located no single table, or the table isn't valid, isn't
    /// the layout at the compiled offset, the flash is smaller than the
    /// layout, or the app didn't boot from a slot: only what was located is
    /// read, and nothing is written.
    Unverified,
    /// The table is the layout and the app booted from a slot, but the
    /// bootloader isn't known to read the compiled offset and roll back: the
    /// slots and the otadata are read, the configurations and `diag`
    /// written.
    Layout,
    /// As the layout tier, with a known bootloader that supports rollback:
    /// the alternate slot and the otadata are written too.
    Boot,
}

/// A step of the chain, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// The bootloader verifies and is a known one that supports rollback.
    Bootloader,
    /// A single table is located.
    TableOffset,
    /// The located table is valid as the bootloader judges it, and its
    /// partitions read.
    Table,
    /// The located table is at [`layout::TABLE_OFFSET`] and its partitions
    /// are the layout's.
    Layout,
    /// The flash holds at least [`layout::FLASH_SIZE`] bytes.
    FlashSize,
    /// The booted offset is `ota_0`'s or `ota_1`'s.
    BootedSlot,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Self; 6] = [
        Self::Bootloader,
        Self::TableOffset,
        Self::Table,
        Self::Layout,
        Self::FlashSize,
        Self::BootedSlot,
    ];
}

/// How the table was located.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TableSource {
    /// At the offset the known bootloader's entry gives.
    KnownBootloader,
    /// By the search, which found it alone.
    Searched,
}

/// The bootloader's identity, from its verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootloaderId {
    /// The image's length in bytes, the digest included.
    pub size: u32,
    /// The SHA-256 digest the image stores and matches, of every byte before
    /// it.
    pub digest: [u8; 32],
}

/// What step 1 found of the bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootloaderReport {
    /// The flash size code of the header at 0x0, read whatever the rest of
    /// the header holds; `None` when the header can't be read.
    pub flash_size_code: Option<u8>,
    /// The header, or why it isn't one.
    pub header: Result<Header, ImageError>,
    /// The image verified, or why it doesn't verify.
    pub image: Result<BootloaderId, ImageError>,
    /// Its entry in [`known::BOOTLOADERS`], when it verifies and is listed.
    pub known: Option<&'static Known>,
    /// Whether the image ends at or before the located table's offset, or
    /// the compiled one when none is located; `None` when it doesn't verify.
    pub fits: Option<bool>,
}

/// What steps 2 to 4 found of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableReport {
    /// How the table was located, `None` when it wasn't.
    pub source: Option<TableSource>,
    /// The located table's offset.
    pub offset: Option<u32>,
    /// What the search found, when it ran: only for a bootloader that isn't
    /// known, whose header reads, and a booted offset inside the flash.
    /// [`Found::Several`] gives the first two candidates' offsets.
    pub search: Option<Found>,
    /// The located table as the bootloader judges it, or the error reading
    /// it or its partitions ([`TableError::Read`]).
    pub verdict: Option<Result<Verified, TableError>>,
    /// The partitions the bootloader loads from it, when it is valid and
    /// they read.
    pub partitions: Option<Partitions>,
}

/// The partitions the bootloader loads from the located table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partitions {
    /// How many: [`Verified::partitions`].
    pub count: usize,
    /// Whether the MD5 entry is one of them, with the last entry left out
    /// ([`Verified::md5_read_as_partition`]).
    pub md5_read_as_partition: bool,
    /// Them compared with the layout by label.
    pub comparison: LayoutComparison,
    /// The span of the app partition the bootloader keeps at the booted
    /// offset: the first of the factory, test and OTA apps there.
    pub booted: Option<Span>,
    /// The span of the otadata partition the bootloader keeps: the last
    /// `data`/`ota` entry.
    pub otadata: Option<Span>,
}

impl BootloaderReport {
    /// The chip size the bootloader checks the table against, which the
    /// flash size code gives: 2 MiB for an unknown code.
    #[must_use]
    pub const fn chip_size(&self) -> Option<u32> {
        match self.flash_size_code {
            Some(code) => Some(image::flash_size(code)),
            None => None,
        }
    }
}

/// What [`split`] found: every step's result, the tier, and the step that
/// kept it below the boot tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Size of the flash in bytes, as the device gives it.
    pub capacity: u32,
    /// Step 1.
    pub bootloader: BootloaderReport,
    /// Steps 2 to 4.
    pub table: TableReport,
    /// The offset of the app the bootloader started, from MMU entry 0.
    pub booted: u32,
    /// The slot the app booted from, in the layout and boot tiers.
    pub slot: Option<Slot>,
    /// The tier reached: the one whose handles [`split`] made.
    pub tier: Tier,
    /// The step that kept the tier below the boot tier: in the unverified
    /// tier, the first of steps 2 to 6 that failed; in the layout tier, the
    /// bootloader's; `None` in the boot tier. [`passed`](Self::passed) gives
    /// every step's result.
    pub failed: Option<Step>,
}

impl Report {
    /// The report before any step has run.
    const START: Self = Self {
        capacity: 0,
        bootloader: BootloaderReport {
            flash_size_code: None,
            header: Err(ImageError::PastEnd),
            image: Err(ImageError::PastEnd),
            known: None,
            fits: None,
        },
        table: TableReport {
            source: None,
            offset: None,
            search: None,
            verdict: None,
            partitions: None,
        },
        booted: 0,
        slot: None,
        tier: Tier::Unverified,
        failed: Some(Step::Bootloader),
    };

    /// Whether `step` passed.
    #[must_use]
    pub fn passed(&self, step: Step) -> bool {
        let partitions = self.table.partitions;
        match step {
            Step::Bootloader => {
                self.bootloader.image.is_ok() && self.bootloader.known.is_some_and(|k| k.rollback)
            }
            Step::TableOffset => self.table.offset.is_some(),
            Step::Table => partitions.is_some(),
            Step::Layout => {
                self.table.offset == Some(layout::TABLE_OFFSET)
                    && partitions.is_some_and(|p| p.comparison.matches())
            }
            Step::FlashSize => self.capacity >= layout::FLASH_SIZE,
            Step::BootedSlot => booted_slot(self.booted).is_some(),
        }
    }

    /// The tier the steps reach, and the step that kept it below the boot
    /// tier: the first of steps 2 to 6 that failed, else the bootloader's.
    fn evaluate(&self) -> (Tier, Option<Step>) {
        if let Some(step) = Step::ALL[1..].iter().find(|step| !self.passed(**step)) {
            return (Tier::Unverified, Some(*step));
        }
        if self.passed(Step::Bootloader) && self.table.source == Some(TableSource::KnownBootloader)
        {
            (Tier::Boot, None)
        } else {
            (Tier::Layout, Some(Step::Bootloader))
        }
    }
}

/// The readers of what the chain located, in the unverified tier. Each is
/// `None` when nothing was located, or when its span shares a byte with
/// `factory`, a configuration or `diag` of the compiled layout, or is empty.
#[derive(Debug)]
pub struct Located<'f> {
    /// The sector of the table the search found alone.
    pub table: Option<Reader<'f, FoundTable>>,
    /// The app the bootloader started: the located table's app partition at
    /// the booted offset, cut to the flash, or, when no valid table is
    /// located, the image at the booted offset, as long as its headers
    /// describe.
    pub app: Option<Reader<'f, FoundApp>>,
    /// The located table's otadata partition, cut to the flash.
    pub otadata: Option<Reader<'f, FoundOtadata>>,
}

/// The handles of the layout tier, which the boot tier has too: the compiled
/// layout's regions for the app running from [`Report::slot`].
#[derive(Debug)]
pub struct LayoutHandles<'f> {
    /// The slot that booted.
    pub current_app: Reader<'f, CurrentApp>,
    /// The other slot.
    pub alternate_app: Reader<'f, AlternateApp>,
    /// The otadata.
    pub otadata: Reader<'f, Otadata>,
    /// The current slot's configuration.
    pub current_config: Writer<'f, CurrentConfig>,
    /// The alternate slot's configuration, which is only erased.
    pub alternate_config: Writer<'f, AlternateConfig>,
    /// The diagnostics partition.
    pub diag: Writer<'f, Diag>,
}

/// The writers of the boot selection, in the boot tier only.
#[derive(Debug)]
pub struct BootWriters<'f> {
    /// The other slot, which an update writes.
    pub alternate_app: Writer<'f, AlternateApp>,
    /// The otadata, which selects the slot to boot.
    pub otadata: Writer<'f, Otadata>,
    /// The slot that booted.
    slot: Slot,
}

#[cfg(any(test, feature = "test-support"))]
impl<'f> BootWriters<'f> {
    /// The boot writers `alternate_app` and `otadata`, for the app running
    /// from `slot`, made of any writers for the tests.
    pub(crate) const fn new(
        alternate_app: Writer<'f, AlternateApp>,
        otadata: Writer<'f, Otadata>,
        slot: Slot,
    ) -> Self {
        Self {
            alternate_app,
            otadata,
            slot,
        }
    }
}

impl BootWriters<'_> {
    /// The slot that booted, which the writers' guard protects.
    #[must_use]
    pub const fn slot(&self) -> Slot {
        self.slot
    }
}

/// The handles of the tier reached, beside the bootloader's and the table's
/// readers, which every tier has.
#[derive(Debug)]
pub enum Handles<'f> {
    /// The unverified tier's readers.
    Unverified(Located<'f>),
    /// The layout tier's handles.
    Layout(LayoutHandles<'f>),
    /// The boot tier's handles: the layout tier's and the boot writers.
    Boot(LayoutHandles<'f>, BootWriters<'f>),
}

/// What [`split`] hands out: the report, the readers every tier has, and the
/// handles of the tier reached. Each role has at most one reader and one
/// writer, and none is made twice: they borrow the device and its state table
/// for as long as they live.
#[derive(Debug)]
pub struct Storage<'f> {
    /// What the chain found, and the tier reached.
    pub report: Report,
    /// The bootloader: from 0x0 up to the located table, or up to the compiled
    /// offset when none is located, within the flash.
    pub bootloader: Reader<'f, Bootloader>,
    /// The compiled table's sector, `None` only on a flash too small to hold
    /// it.
    pub partition_table: Option<Reader<'f, PartitionTable>>,
    /// The handles of the tier reached.
    pub handles: Handles<'f>,
}

/// Runs the verification chain on `device`, the internal flash, for the app
/// the bootloader started at `booted`, the offset MMU entry 0 maps, and hands
/// out the handles of the tier reached over `state`, the device's state
/// table.
///
/// The bootloader is hashed yielding every [`image::YIELD_INTERVAL`] bytes;
/// the rest runs without yielding. The device and the state table are
/// borrowed mutably for as long as any handle lives, so no handle is made
/// twice. The chain's state moves into the future rather than living in its
/// frame.
pub fn split<'f>(
    device: &'f mut dyn NorDevice,
    state: &'f mut State,
    booted: u32,
) -> impl Future<Output = Storage<'f>> + 'f {
    split_with(device, state, booted, known::BOOTLOADERS)
}

/// [`split`] with `bootloaders` as the known list.
pub(crate) fn split_with<'f>(
    device: &'f mut dyn NorDevice,
    state: &'f mut State,
    booted: u32,
    bootloaders: &'static [Known],
) -> impl Future<Output = Storage<'f>> + 'f {
    let device: &'f dyn NorDevice = device;
    let state: &'f State = state;
    let mut chain = Chain::new(device, booted, bootloaders);
    async move {
        chain.verify_bootloader().await;
        chain.locate();
        chain.judge();
        chain.storage(state)
    }
}

/// The layout's slot that starts at `booted`.
fn booted_slot(booted: u32) -> Option<Slot> {
    Slot::ALL
        .into_iter()
        .find(|slot| slot.partition().offset == booted)
}

/// A span of the device as a [`Source`], for the chain's reads. It is never
/// handed out.
#[derive(Clone, Copy)]
struct Raw<'f> {
    device: &'f dyn NorDevice,
    span: Span,
}

impl<'f> Raw<'f> {
    /// The whole device.
    fn whole(device: &'f dyn NorDevice) -> Self {
        Self {
            device,
            span: Span::new(0, device.capacity()),
        }
    }

    /// The device from `offset` to its end, or `None` when `offset` lies
    /// past it.
    fn from(device: &'f dyn NorDevice, offset: u32) -> Option<Self> {
        let len = device.capacity().checked_sub(offset)?;
        Some(Self {
            device,
            span: Span::new(offset, len),
        })
    }
}

impl Source for Raw<'_> {
    fn size(&self) -> u32 {
        self.span.len
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        let len = u32::try_from(buf.len()).map_err(|_| ReadError::OutOfRange)?;
        let address = self
            .span
            .inner(Span::new(offset, len))
            .ok_or(ReadError::OutOfRange)?
            .offset;
        self.device
            .read(address, buf)
            .map_err(|_| ReadError::Device)
    }
}

/// The chain's state: the device and the report, which each step writes in
/// place, so no frame holds more than one.
struct Chain<'f> {
    device: &'f dyn NorDevice,
    bootloaders: &'static [Known],
    report: Report,
}

impl<'f> Chain<'f> {
    /// The chain, with the bootloader's header read.
    fn new(device: &'f dyn NorDevice, booted: u32, bootloaders: &'static [Known]) -> Self {
        let mut chain = Self {
            device,
            bootloaders,
            report: Report::START,
        };
        chain.report.capacity = device.capacity();
        chain.report.booted = booted;
        chain.read_header();
        chain
    }

    fn whole(&self) -> Raw<'f> {
        Raw::whole(self.device)
    }

    /// Reads the bootloader's header, and its flash size code whatever the
    /// rest holds.
    fn read_header(&mut self) {
        let mut header = [0; image::HEADER_LEN as usize];
        let read = self.whole().read(0, &mut header);
        let bootloader = &mut self.report.bootloader;
        match read {
            Ok(()) => {
                let code = image::flash_size_code(&header);
                bootloader.flash_size_code = Some(code);
                bootloader.header = Header::parse(&header);
            }
            Err(error) => bootloader.header = Err(ImageError::Read(error)),
        }
    }

    /// Verifies the bootloader within the flash and looks it up.
    async fn verify_bootloader(&mut self) {
        let whole = self.whole();
        let verified = image::verify_bootloader(&whole).await;
        let bootloader = &mut self.report.bootloader;
        bootloader.image = verified.map(|verified| BootloaderId {
            size: verified.size(),
            digest: *verified.digest(),
        });
        bootloader.known = bootloader.image.ok().and_then(|id| {
            self.bootloaders
                .iter()
                .find(|known| known.digest == id.digest)
        });
    }

    /// Locates the table: at the known bootloader's offset, else by the
    /// search, which counts only a single candidate. Then checks that the
    /// bootloader ends before it.
    fn locate(&mut self) {
        let report = &mut self.report;
        if let Some(known) = report.bootloader.known {
            report.table.source = Some(TableSource::KnownBootloader);
            report.table.offset = Some(known.table_offset);
        } else if let Some(chip_size) = report.bootloader.chip_size()
            && report.booted < report.capacity
        {
            let found = table::find(&Raw::whole(self.device), report.booted, chip_size);
            report.table.search = Some(found);
            if let Some(offset) = found.offset() {
                report.table.source = Some(TableSource::Searched);
                report.table.offset = Some(offset);
            }
        }
        let end = report.table.offset.unwrap_or(layout::TABLE_OFFSET);
        report.bootloader.fits = report.bootloader.image.ok().map(|id| id.size <= end);
    }

    /// Judges the located table, and reads its partitions when it is valid.
    fn judge(&mut self) {
        let table = &mut self.report.table;
        let (Some(offset), Some(chip_size)) = (table.offset, self.report.bootloader.chip_size())
        else {
            return;
        };
        let whole = Raw::whole(self.device);
        let verdict = table::verify(&whole, offset, chip_size);
        table.verdict = Some(verdict);
        if let Ok(verified) = verdict {
            match read_partitions(&whole, offset, &verified, self.report.booted) {
                Ok(partitions) => table.partitions = Some(partitions),
                Err(error) => table.verdict = Some(Err(error)),
            }
        }
    }

    /// The report evaluated, and the handles of the tier reached.
    fn storage(&mut self, state: &'f State) -> Storage<'f> {
        // The handles first: they record the tier in the report.
        Storage {
            handles: self.handles(state),
            report: self.report,
            bootloader: self.bootloader_reader(state),
            partition_table: Reader::new(
                self.device,
                state,
                Span::new(layout::TABLE_OFFSET, SECTOR_SIZE),
            ),
        }
    }

    /// Evaluates the steps, records the tier and the slot, and makes the
    /// handles of the tier. Never inlined, so the handles it makes stay out
    /// of the caller's frame.
    #[inline(never)]
    fn handles(&mut self, state: &'f State) -> Handles<'f> {
        let (tier, failed) = self.report.evaluate();
        let slot = booted_slot(self.report.booted);
        self.report.failed = failed;
        let device = self.device;
        match (tier, slot) {
            (Tier::Boot, Some(slot)) => {
                self.record(Tier::Boot, Some(slot));
                Handles::Boot(
                    layout_handles(device, state, slot),
                    boot_writers(device, state, slot),
                )
            }
            (Tier::Layout, Some(slot)) => {
                self.record(Tier::Layout, Some(slot));
                Handles::Layout(layout_handles(device, state, slot))
            }
            _ => {
                self.record(Tier::Unverified, None);
                Handles::Unverified(self.located(state))
            }
        }
    }

    /// Records `tier` and `slot` in the report.
    fn record(&mut self, tier: Tier, slot: Option<Slot>) {
        self.report.tier = tier;
        self.report.slot = slot;
    }

    /// The bootloader's reader: up to the located table, else up to the
    /// compiled offset, within the flash.
    fn bootloader_reader(&self, state: &'f State) -> Reader<'f, Bootloader> {
        let end = self.report.table.offset.unwrap_or(layout::TABLE_OFFSET);
        Reader::within(self.device, state, Span::new(0, end))
    }

    /// The readers of what was located, for the unverified tier.
    fn located(&self, state: &'f State) -> Located<'f> {
        let report = &self.report;
        let device = self.device;
        let capacity = report.capacity;
        let partitions = report.table.partitions;
        let table = match (report.table.source, report.table.offset) {
            (Some(TableSource::Searched), Some(offset)) if partitions.is_some() => {
                located_reader(device, state, Span::new(offset, SECTOR_SIZE), capacity)
            }
            _ => None,
        };
        let app = match partitions {
            Some(partitions) => partitions
                .booted
                .and_then(|span| located_reader(device, state, span, capacity)),
            None => booted_image(device, report.booted)
                .and_then(|span| located_reader(device, state, span, capacity)),
        };
        let otadata = partitions
            .and_then(|partitions| partitions.otadata)
            .and_then(|span| located_reader(device, state, span, capacity));
        Located {
            table,
            app,
            otadata,
        }
    }
}

/// The partitions the bootloader loads from the table at `offset`, which
/// [`table::verify`] judged `verified`, for the app booted at `booted`. Never
/// inlined, so what it keeps stays out of the chain's frame.
#[inline(never)]
fn read_partitions(
    whole: &Raw<'_>,
    offset: u32,
    verified: &Verified,
    booted: u32,
) -> Result<Partitions, TableError> {
    let mut loaded = table::Loaded::new();
    let mut error = None;
    let read = table::partitions(whole, offset, verified).map_while(|partition| match partition {
        Ok((index, entry)) => {
            loaded.keep(index, &entry);
            Some(entry)
        }
        Err(read_error) => {
            error = Some(read_error);
            None
        }
    });
    let comparison = table::compare_labels(read, &layout::PARTITIONS);
    if let Some(error) = error {
        return Err(error);
    }
    Ok(Partitions {
        count: verified.partitions(),
        md5_read_as_partition: verified.md5_read_as_partition(),
        comparison,
        booted: loaded
            .apps()
            .find(|kept| kept.offset == booted)
            .map(|kept| Span::new(kept.offset, kept.size)),
        otadata: loaded.otadata.map(|kept| Span::new(kept.offset, kept.size)),
    })
}

/// The span of the app image at `booted`, as long as its headers describe:
/// `None` when they don't describe one that lies inside the flash.
fn booted_image(device: &dyn NorDevice, booted: u32) -> Option<Span> {
    let rest = Raw::from(device, booted)?;
    let info = image::inspect_app(&rest).ok()?;
    Some(Span::new(booted, info.size()))
}

/// The reader of a role located at runtime over `span`, cut to the flash's
/// `capacity`; `None` when the cut span is empty, starts past the flash, or
/// shares a byte with a span the compiled layout gives to sensitive data.
fn located_reader<'f, R: Role<Device = Internal>>(
    device: &'f dyn NorDevice,
    state: &'f State,
    span: Span,
    capacity: u32,
) -> Option<Reader<'f, R>> {
    if span.offset >= capacity {
        return None;
    }
    let cut = span.intersection(Span::new(0, capacity))?;
    if cut.is_empty() || is_sensitive(cut) {
        return None;
    }
    Reader::new(device, state, cut)
}

/// The layout tier's handles for the app running from `slot`, on a flash
/// that holds the layout, which the chain has checked.
fn layout_handles<'f>(
    device: &'f dyn NorDevice,
    state: &'f State,
    slot: Slot,
) -> LayoutHandles<'f> {
    let reader = |entry: Entry| Span::of(&entry);
    LayoutHandles {
        current_app: Reader::within(device, state, reader(slot.partition())),
        alternate_app: Reader::within(device, state, reader(slot.other().partition())),
        otadata: Reader::within(device, state, reader(layout::OTADATA)),
        current_config: Writer::of_partition(device, state, &slot.config(), slot),
        alternate_config: Writer::of_partition(device, state, &slot.other().config(), slot),
        diag: Writer::of_partition(device, state, &layout::DIAG, slot),
    }
}

/// The boot tier's writers for the app running from `slot`, on a flash that
/// holds the layout, which the chain has checked.
fn boot_writers<'f>(device: &'f dyn NorDevice, state: &'f State, slot: Slot) -> BootWriters<'f> {
    BootWriters {
        alternate_app: Writer::of_partition(device, state, &slot.other().partition(), slot),
        otadata: Writer::of_partition(device, state, &layout::OTADATA, slot),
        slot,
    }
}

#[cfg(test)]
mod tests;
