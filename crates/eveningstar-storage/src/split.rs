//! The checks of the board's table and bootloader, the trust tiers they
//! decide, and the handles of each tier.
//!
//! [`split`] runs once per boot. It never fails: when the flash isn't laid out
//! as the firmware expects, it hands out fewer handles, and its [`Report`]
//! says why.
//!
//! | Tier | Condition | Handles |
//! |---|---|---|
//! | [`Tier::Untrusted`] | the table at [`layout::TABLE_OFFSET`] doesn't match the layout, the flash is smaller than it, or the app didn't boot from a slot | [`BootChain`]: the bootloader, the table found on the board (if any), the running app (if its span is known) |
//! | [`Tier::Data`] | otherwise | + [`Data`]: config, other config, diag, other app, otadata |
//! | [`Tier::Full`] | + the bootloader is known and the running app's descriptor reads | + [`Ota`] |

use core::fmt;

use crate::image::{self, ImageError};
use crate::otadata::{self, BootState, State};
use crate::partitions::{
    Bootloader, Config, Diag, Otadata, OtherApp, OtherConfig, Partition, PartitionTable, RunningApp,
};
use crate::region::Region;
use crate::table::{self, LayoutComparison};
use crate::{Entry, Flash, ReadError, Slot, TYPE_APP, Text, known, layout};

/// How far the board is trusted, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// The layout on the board isn't the one compiled in, or the app didn't
    /// boot from one of its slots: only the boot chain is read.
    Untrusted,
    /// The layout is the one compiled in and the app booted from a slot: the
    /// data partitions are handed out.
    Data,
    /// The bootloader is known to support rollback as well, and the running
    /// app's descriptor reads: the boot selection can be changed.
    Full,
}

/// Why `split` stopped below the full tier: the first check that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Limit {
    /// The table at [`layout::TABLE_OFFSET`] doesn't match the layout
    /// ([`Report::table`] says how). Untrusted tier.
    Table,
    /// The flash is smaller than [`layout::FLASH_SIZE`]. Untrusted tier.
    FlashSize,
    /// The booted offset is neither `ota_0`'s nor `ota_1`'s. Untrusted tier.
    NotASlot,
    /// The bootloader doesn't verify ([`Report::bootloader`] says why). Data
    /// tier.
    BootloaderInvalid,
    /// The bootloader verifies, but its digest isn't in
    /// [`known::BOOTLOADERS`]. Data tier.
    BootloaderUnknown,
    /// The running app's descriptor doesn't read ([`Report::app`] says why),
    /// so there is no project name for an update to match. Data tier.
    AppDescriptor,
}

impl Limit {
    /// The tier `split` reaches when this check fails.
    #[must_use]
    pub const fn tier(self) -> Tier {
        match self {
            Self::Table | Self::FlashSize | Self::NotASlot => Tier::Untrusted,
            Self::BootloaderInvalid | Self::BootloaderUnknown | Self::AppDescriptor => Tier::Data,
        }
    }
}

/// The bootloader's identity: the digest of its image, and its name when it
/// is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootloaderId {
    /// The SHA-256 digest the image stores and matches.
    pub digest: [u8; 32],
    /// Its name in [`known::BOOTLOADERS`], or `None` when it isn't listed.
    pub name: Option<&'static str>,
}

/// The running app's name and version, from its descriptor. The rest of the
/// descriptor reads from the running app's handle
/// ([`image::app_descriptor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppId {
    /// The project's name.
    pub project_name: Text<32>,
    /// The app's version.
    pub version: Text<32>,
}

/// What `split` found on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// The table at [`layout::TABLE_OFFSET`] compared with the layout, entry
    /// by entry (T1).
    pub table: LayoutComparison,
    /// The offset of the table the bootloader read, found on the board when
    /// the one at the compiled offset doesn't match the layout (T2); `None`
    /// when it matches, or when no table was found.
    pub found_table: Option<u32>,
    /// The bootloader at 0x0, verified up to the table (T3).
    pub bootloader: Result<BootloaderId, ImageError>,
    /// The offset of the app the bootloader started.
    pub booted: u32,
    /// The slot the app booted from: `Some` in the data and full tiers, when
    /// the table matches the layout and the booted offset is a slot's (T5).
    pub running: Option<Slot>,
    /// The running app's name and version, or why its descriptor doesn't
    /// read.
    pub app: Result<AppId, ImageError>,
    /// The boot state the otadata gives, read in the data and full tiers.
    pub boot_state: Option<BootState>,
    /// Why the full tier wasn't reached; `None` in the full tier.
    pub limit: Option<Limit>,
}

impl Report {
    /// The tier reached.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.limit.map_or(Tier::Full, Limit::tier)
    }

    /// The offset of the table the boot chain's handle covers: the compiled
    /// offset when the table there matches, else the one found.
    #[must_use]
    pub fn table_offset(&self) -> Option<u32> {
        if self.table.matches() {
            Some(layout::TABLE_OFFSET)
        } else {
            self.found_table
        }
    }

    /// Whether the running image is [`State::PendingVerify`] in a tier below
    /// full, where it can't be confirmed (T7): unless confirmed, the
    /// bootloader rolls it back at the next boot. Always `false` in the
    /// untrusted tier, where the boot state isn't read; an image there can't
    /// have come from an update, which never changes the table.
    #[must_use]
    pub fn cannot_confirm(&self) -> bool {
        self.tier() != Tier::Full
            && self
                .boot_state
                .is_some_and(|state| state.state == Some(State::PendingVerify))
    }
}

/// The handles of the boot chain, which every tier has. They are public read
/// handles, describing what is on the board.
#[derive(Clone, Copy, Debug)]
pub struct BootChain<'f> {
    /// The bootloader: from 0x0 up to the table (the compiled offset when
    /// no table is found), within the flash.
    pub bootloader: Partition<'f, Bootloader>,
    /// The table the bootloader read, [`table::MAX_LEN`] bytes or up to the
    /// end of the flash; `None` when no table was found. Always `Some` in
    /// the data and full tiers.
    pub table: Option<Partition<'f, PartitionTable>>,
    /// The running app: its slot in the data and full tiers; else its entry
    /// in the board's table, or its image when the table has none; `None`
    /// when neither lies inside the flash.
    pub running_app: Option<Partition<'f, RunningApp>>,
}

/// The handles of the data tier: one of each, the read views being `Copy`.
#[derive(Debug)]
pub struct Data<'f> {
    /// The running slot's configuration.
    pub config: Partition<'f, Config>,
    /// The other slot's configuration.
    pub other_config: Partition<'f, OtherConfig>,
    /// The diagnostics partition.
    pub diag: Partition<'f, Diag>,
    /// The other app slot, read only.
    pub other_app: Partition<'f, OtherApp>,
    /// The otadata, read only.
    pub otadata: Partition<'f, Otadata>,
    /// The OTA mechanism, in the full tier only.
    pub ota: Option<Ota<'f>>,
}

/// The OTA mechanism, in the full tier only: the other app slot and the
/// otadata, which it alone writes, the running slot and the running app's
/// project name.
pub struct Ota<'f> {
    other: Region<'f>,
    otadata: Region<'f>,
    running: Slot,
    project: Text<32>,
}

impl Ota<'_> {
    /// The boot state the otadata gives now.
    #[must_use]
    pub fn boot_state(&self) -> BootState {
        otadata::read(&self.otadata).boot_state(self.running)
    }
}

impl fmt::Debug for Ota<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ota")
            .field("other", &self.other)
            .field("otadata", &self.otadata)
            .field("running", &self.running)
            .field("project", &self.project)
            .finish()
    }
}

/// What `split` hands out: the report, the boot chain, and the data tier's
/// handles when it was reached, the full tier's inside them.
#[derive(Debug)]
pub struct Storage<'f> {
    /// What was found, and the tier reached.
    pub report: Report,
    /// The boot chain's read handles.
    pub boot: BootChain<'f>,
    /// The data tier's handles, `None` in the untrusted tier.
    pub data: Option<Data<'f>>,
}

/// Checks the board's table and bootloader on `flash` and hands out the
/// handles of the tier reached, for the app the bootloader started at
/// `booted`, an offset from the start of the flash.
///
/// The table at [`layout::TABLE_OFFSET`] is compared with the layout; when it
/// doesn't match, the board's table is the first well-formed one below
/// `booted` with an app entry at `booted` ([`table::find`]). The bootloader is
/// verified up to that table, yielding every 4 KiB hashed, and looked up in
/// [`known::BOOTLOADERS`]. In the data and full tiers `booted` has to be a
/// slot's offset; in the untrusted tier the running app's span is its entry
/// in the board's table, or its image's length when the table has none. The
/// boot state is read in the data and full tiers.
///
/// Every handle lies inside the flash, and the data and full tiers' are the
/// layout's partitions. `flash` is borrowed mutably for as long as any handle
/// lives, so a second call can't hand out a second copy of a unique handle.
pub async fn split(flash: &mut dyn Flash, booted: u32) -> Storage<'_> {
    let flash: &dyn Flash = flash;
    let mut checks = Checks::new(flash, booted);
    checks.verify_bootloader().await;
    checks.storage()
}

/// What `split` checks, in order: the table, the bootloader and the running
/// app. Each step returns little or nothing and the async one writes its
/// result in place, so no frame holds more than one [`Storage`].
struct Checks<'f> {
    flash: &'f dyn Flash,
    booted: u32,
    comparison: LayoutComparison,
    found: Option<u32>,
    bootloader: Result<BootloaderId, ImageError>,
    running_app: Result<Region<'f>, ImageError>,
}

impl<'f> Checks<'f> {
    /// Compares the table and finds the board's, and finds the running app.
    fn new(flash: &'f dyn Flash, booted: u32) -> Self {
        let whole = Region::clamped(flash, 0, flash.capacity());
        let comparison = table::compare_layout(&whole);
        // A booted offset past the flash has no table entry to find.
        let found = if comparison.matches() || booted >= whole.len() {
            None
        } else {
            table::find(&whole, booted)
        };
        let mut checks = Self {
            flash,
            booted,
            comparison,
            found,
            bootloader: Err(ImageError::PastEnd),
            running_app: Err(ImageError::PastEnd),
        };
        checks.running_app = match checks.data_spans() {
            Some(spans) => Ok(spans.running),
            None => untrusted_app(flash, checks.table_offset(), booted),
        };
        checks
    }

    /// Verifies the bootloader up to the board's table and looks it up.
    async fn verify_bootloader(&mut self) {
        let region = self.bootloader_region();
        let verified = image::verify_bootloader(&region).await;
        self.bootloader = verified.map(|bootloader| BootloaderId {
            digest: *bootloader.digest(),
            name: known::name(bootloader.digest()),
        });
    }

    /// The offset of the board's table.
    fn table_offset(&self) -> Option<u32> {
        if self.comparison.matches() {
            Some(layout::TABLE_OFFSET)
        } else {
            self.found
        }
    }

    /// The bootloader's span: up to the board's table, else up to the
    /// compiled offset.
    fn bootloader_region(&self) -> Region<'f> {
        let end = self.table_offset().unwrap_or(layout::TABLE_OFFSET);
        Region::clamped(self.flash, 0, end)
    }

    /// The slot the app booted from, whether or not the table matches.
    fn booted_slot(&self) -> Option<Slot> {
        Slot::ALL
            .into_iter()
            .find(|slot| slot.partition().offset == self.booted)
    }

    /// The layout's partitions, in the data tier and up.
    fn data_spans(&self) -> Option<DataSpans<'f>> {
        let slot = self.booted_slot()?;
        let trusted = self.comparison.matches() && self.flash.capacity() >= layout::FLASH_SIZE;
        trusted.then(|| DataSpans::new(self.flash, slot))
    }

    /// The running app's name and version.
    fn app(&self) -> Result<AppId, ImageError> {
        self.running_app.and_then(|region| app_id(&region))
    }

    /// The first check that keeps the tier below full.
    fn limit(&self) -> Option<Limit> {
        if !self.comparison.matches() {
            Some(Limit::Table)
        } else if self.flash.capacity() < layout::FLASH_SIZE {
            Some(Limit::FlashSize)
        } else if self.booted_slot().is_none() {
            Some(Limit::NotASlot)
        } else if self.bootloader.is_err() {
            Some(Limit::BootloaderInvalid)
        } else if self.bootloader.is_ok_and(|id| id.name.is_none()) {
            Some(Limit::BootloaderUnknown)
        } else if self.app().is_err() {
            Some(Limit::AppDescriptor)
        } else {
            None
        }
    }

    fn storage(&self) -> Storage<'f> {
        Storage {
            report: self.report(),
            boot: self.boot_chain(),
            data: self.data(),
        }
    }

    fn report(&self) -> Report {
        let spans = self.data_spans();
        Report {
            table: self.comparison,
            found_table: self.found,
            bootloader: self.bootloader,
            booted: self.booted,
            running: spans.as_ref().map(|spans| spans.slot),
            app: self.app(),
            boot_state: spans
                .as_ref()
                .map(|spans| otadata::read(&spans.otadata).boot_state(spans.slot)),
            limit: self.limit(),
        }
    }

    fn boot_chain(&self) -> BootChain<'f> {
        BootChain {
            bootloader: Partition::new(self.bootloader_region()),
            table: self
                .table_offset()
                .map(|offset| Partition::new(Region::clamped(self.flash, offset, table::MAX_LEN))),
            running_app: self.running_app.ok().map(Partition::new),
        }
    }

    /// The data tier's handles, with the full tier's when it was reached.
    fn data(&self) -> Option<Data<'f>> {
        let spans = self.data_spans()?;
        Some(Data {
            config: Partition::new(spans.config),
            other_config: Partition::new(spans.other_config),
            diag: Partition::new(spans.diag),
            other_app: Partition::new(spans.other_app),
            otadata: Partition::new(spans.otadata),
            ota: self.ota(&spans),
        })
    }

    /// The OTA mechanism, in the full tier.
    fn ota(&self, spans: &DataSpans<'f>) -> Option<Ota<'f>> {
        if self.limit().is_some() {
            return None;
        }
        let app = self.app().ok()?;
        Some(Ota {
            other: spans.other_app,
            otadata: spans.otadata,
            running: spans.slot,
            project: app.project_name,
        })
    }
}

/// The name and version of the app image in `region`.
fn app_id(region: &Region<'_>) -> Result<AppId, ImageError> {
    let descriptor = image::app_descriptor(region)?;
    Ok(AppId {
        project_name: descriptor.project_name(),
        version: descriptor.version(),
    })
}

/// The running app's span in the untrusted tier: its entry in the table at
/// `table_offset`, else the length of the image at `booted`.
fn untrusted_app(
    flash: &dyn Flash,
    table_offset: Option<u32>,
    booted: u32,
) -> Result<Region<'_>, ImageError> {
    let whole = Region::clamped(flash, 0, flash.capacity());
    let entry = table_offset.and_then(|offset| {
        table::entries(&whole, offset)
            .map_while(Result::ok)
            .find(|entry| entry.kind == TYPE_APP && entry.offset == booted)
    });
    if let Some(region) = entry.and_then(|entry| Region::new(flash, booted, entry.size)) {
        return Ok(region);
    }
    let rest = flash
        .capacity()
        .checked_sub(booted)
        .and_then(|len| Region::new(flash, booted, len))
        .ok_or(ImageError::Read(ReadError::OutOfRange))?;
    let info = image::inspect_app(&rest)?;
    Region::new(flash, booted, info.size()).ok_or(ImageError::Read(ReadError::OutOfRange))
}

/// The layout's partitions for the app running from `slot`.
struct DataSpans<'f> {
    slot: Slot,
    running: Region<'f>,
    config: Region<'f>,
    other_config: Region<'f>,
    diag: Region<'f>,
    other_app: Region<'f>,
    otadata: Region<'f>,
}

impl<'f> DataSpans<'f> {
    /// The spans, on a flash at least [`layout::FLASH_SIZE`] long, where every
    /// partition of the layout lies.
    fn new(flash: &'f dyn Flash, slot: Slot) -> Self {
        let region = |entry: Entry| Region::clamped(flash, entry.offset, entry.size);
        Self {
            slot,
            running: region(slot.partition()),
            config: region(slot.config()),
            other_config: region(slot.other().config()),
            diag: region(layout::DIAG),
            other_app: region(slot.other().partition()),
            otadata: region(layout::OTADATA),
        }
    }
}

const _: () = {
    let mut i = 0;
    while i < layout::PARTITIONS.len() {
        assert!(layout::PARTITIONS[i].end() <= layout::FLASH_SIZE);
        i += 1;
    }
};

#[cfg(test)]
mod tests;
