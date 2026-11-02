//! `/storage`: the board's storage devices, the internal flash, its regions
//! and partition table, and the content of its public regions.
//!
//! The handlers map what the [storage service](crate::services::storage)
//! reports to the API's types. Text read from the flash is sent as printable
//! ASCII, any other byte as `?`.
//!
//! The larger responses are [`Shared`] bodies, filled in place, each part by
//! a function of its own, so no frame holds a whole response.

use core::convert::Infallible;
use core::fmt::{self, Write as _};

use eveningstar_api::v1;
use eveningstar_storage::image::{AppDescriptor, ImageError};
use eveningstar_storage::otadata::{Sector, State, Validity};
use eveningstar_storage::partitions::{Partition, PartitionTable, SensitiveRead};
use eveningstar_storage::table::{self, Fields, Found, Match, RawEntry, TableError};
use eveningstar_storage::{Limit, ReadError, Slot, Text, Tier, layout};
use picoserve::extract::FromRef;

use super::AppState;
use super::api::{Download, Shared, SharedBody, Source};
use crate::services::storage::{
    AppRole, AppSlot, Buffer, Content, ContentError, ContentReadError, Contents, Image, Part,
    Progress, Region, RegionName, StorageView,
};
use crate::services::system::{NAME_MAX_LEN, System};

/// [`v1::GetStorage`]. The board has an external flash
/// ([`Board::external_flash`](eveningstar_board::Board::external_flash)),
/// which this firmware doesn't support.
pub(super) async fn get_storage(
    storage: StorageView,
    (): (),
    (): (),
) -> Result<v1::StorageDevices, Infallible> {
    let capacity = storage.capacity();
    let devices = [
        v1::StorageDevice {
            id: v1::StorageId::Internal,
            kind: v1::StorageKind::NorFlash,
            status: v1::StorageStatus::Available,
            capacity: (capacity != 0).then_some(capacity.into()),
        },
        v1::StorageDevice {
            id: v1::StorageId::External,
            kind: v1::StorageKind::NandFlash,
            status: v1::StorageStatus::NotSupported,
            capacity: None,
        },
    ];
    Ok(v1::StorageDevices {
        devices: devices.into_iter().collect(),
    })
}

/// The body of [`v1::GetInternalFlash`].
static INTERNAL_FLASH: Shared<v1::InternalFlash> = Shared::new(v1::InternalFlash {
    capacity: 0,
    layout_size: 0,
    tier: v1::Tier::Untrusted,
    limit: None,
    persistent_config: false,
    updates: false,
    booted_offset: None,
    table: v1::TableSummary {
        offset: 0,
        matches: false,
        md5: None,
        board_offset: None,
    },
    bootloader: v1::BootloaderInfo {
        status: v1::BootloaderStatus::Invalid,
        name: None,
        error: None,
        size: None,
        digest: None,
        file_sha256: None,
        version: None,
        idf_version: None,
        built: None,
    },
    boot: None,
    otadata: heapless::Vec::new(),
    apps: heapless::Vec::new(),
    checks: v1::Checks {
        status: v1::CheckStatus::Running,
        finished_uptime_s: None,
    },
});

/// [`v1::GetInternalFlash`].
pub(super) async fn get_internal_flash(
    storage: StorageView,
    (): (),
    (): (),
) -> Result<SharedBody<v1::InternalFlash>, Infallible> {
    let mut body = INTERNAL_FLASH.lock().await;
    internal_flash(&mut body, storage);
    Ok(body)
}

#[inline(never)]
fn internal_flash(flash: &mut v1::InternalFlash, storage: StorageView) {
    let report = storage.report();
    flash.capacity = storage.capacity();
    flash.layout_size = layout::FLASH_SIZE;
    flash.tier = tier(report.tier());
    flash.limit = report.limit.map(limit);
    flash.persistent_config = storage.persistent_config();
    flash.updates = storage.updates();
    flash.booted_offset = storage.booted();
    flash.table = v1::TableSummary {
        offset: layout::TABLE_OFFSET,
        matches: report.table.matches(),
        md5: table_md5(report.table.end),
        board_offset: report.table_offset(),
    };
    bootloader(&mut flash.bootloader, storage);
    flash.boot = report.boot_state.map(|state| v1::BootState {
        running: slot(state.running),
        selected: state.selected.map(slot),
        state: state.state.map(ota_state),
        rollback_target: state.rollback_target,
        needs_confirmation: state.needs_confirmation,
    });
    otadata(&mut flash.otadata, storage);
    flash.apps.clear();
    for role in [AppRole::Current, AppRole::Alternate] {
        app_slot(&mut flash.apps, role, storage);
    }
    flash.checks = match storage.progress() {
        Progress::Running => v1::Checks {
            status: v1::CheckStatus::Running,
            finished_uptime_s: None,
        },
        Progress::Done { finished } => v1::Checks {
            status: v1::CheckStatus::Done,
            finished_uptime_s: Some(finished.as_secs()),
        },
    };
}

#[inline(never)]
fn bootloader(info: &mut v1::BootloaderInfo, storage: StorageView) {
    match storage.report().bootloader {
        Ok(id) => {
            info.status = if id.name.is_some() {
                v1::BootloaderStatus::Known
            } else {
                v1::BootloaderStatus::Unlisted
            };
            info.name = id
                .name
                .and_then(|name| heapless::String::try_from(name).ok());
            info.error = None;
            info.size = Some(id.size);
            info.digest = Some(v1::Sha256(id.digest));
            info.file_sha256 = Some(v1::Sha256(id.file_sha256));
            bootloader_descriptor(info, storage);
        }
        Err(err) => {
            info.status = v1::BootloaderStatus::Invalid;
            info.name = None;
            info.error = Some(fault(err));
            info.size = None;
            info.digest = None;
            info.file_sha256 = None;
            info.version = None;
            info.idf_version = None;
            info.built = None;
        }
    }
}

/// The fields of `info` the bootloader's descriptor gives.
#[inline(never)]
fn bootloader_descriptor(info: &mut v1::BootloaderInfo, storage: StorageView) {
    let Some(descriptor) = storage.bootloader_descriptor() else {
        info.version = None;
        info.idf_version = None;
        info.built = None;
        return;
    };
    info.version = Some(descriptor.version());
    info.idf_version = Some(text(&descriptor.idf_version()));
    let built: v1::FlashText = text(&descriptor.date_time());
    // A build that leaves the date out writes it empty.
    info.built = (!built.is_empty()).then_some(built);
}

#[inline(never)]
fn otadata(entries: &mut heapless::Vec<v1::OtadataEntry, 2>, storage: StorageView) {
    entries.clear();
    let Some(otadata) = storage.otadata() else {
        return;
    };
    for sector in Sector::ALL {
        let entry = otadata.entry(sector).ok();
        let validity = entry.map_or(v1::EntryValidity::Unreadable, |entry| {
            match entry.validity() {
                Validity::Valid => v1::EntryValidity::Valid,
                Validity::Blank => v1::EntryValidity::Blank,
                Validity::Corrupt => v1::EntryValidity::Corrupt,
                Validity::Invalid => v1::EntryValidity::Invalid,
                Validity::Aborted => v1::EntryValidity::Aborted,
            }
        });
        let stored = entry.filter(|entry| entry.validity() != Validity::Blank);
        let _ = entries.push(v1::OtadataEntry {
            // 0 or 1.
            sector: u8::try_from(sector.index()).unwrap_or(u8::MAX),
            validity,
            seq: stored.map(|entry| entry.seq),
            state: stored.map(|entry| ota_state(entry.state)),
            selects: stored.map(|entry| slot(entry.slot())),
        });
    }
}

/// Adds the app slot in `role` to `apps`, when the firmware holds a handle
/// for it.
#[inline(never)]
fn app_slot(apps: &mut heapless::Vec<v1::AppSlot, 2>, role: AppRole, storage: StorageView) {
    let Some(app) = storage.app(role) else {
        return;
    };
    let Some(fields) = push_app_slot(apps) else {
        return;
    };
    fields.region = region(RegionName::App(role));
    fields.slot = app.slot.map(slot);
    fields.offset = app.offset;
    fields.size = app.size;
    app_image(&mut fields.image, &app);
    match storage.app_descriptor(role) {
        Some(descriptor) => {
            let target = fields.image.descriptor.get_or_insert_with(empty_descriptor);
            app_descriptor(target, &descriptor);
        }
        None => clear(&mut fields.image.descriptor),
    }
}

/// Adds an app slot to `apps` and returns it, for its fields to be filled.
#[inline(never)]
fn push_app_slot(apps: &mut heapless::Vec<v1::AppSlot, 2>) -> Option<&mut v1::AppSlot> {
    apps.push(empty_app_slot()).ok()?;
    apps.last_mut()
}

#[inline(never)]
fn empty_app_slot() -> v1::AppSlot {
    v1::AppSlot {
        region: v1::Region::AppCurrent,
        slot: None,
        offset: 0,
        size: 0,
        image: v1::AppImage {
            status: v1::ImageStatus::Checking,
            checked_uptime_s: None,
            error: None,
            size: None,
            digest: None,
            file_sha256: None,
            descriptor: None,
        },
    }
}

#[inline(never)]
fn empty_descriptor() -> v1::AppDescriptor {
    v1::AppDescriptor {
        project_name: heapless::String::new(),
        version: heapless::String::new(),
        date: heapless::String::new(),
        time: heapless::String::new(),
        idf_version: heapless::String::new(),
        secure_version: 0,
        elf_sha256: v1::Sha256([0; 32]),
    }
}

/// Sets `value` to `None`. Never inlined, so a large `T` stays out of its
/// callers' frames.
#[inline(never)]
fn clear<T>(value: &mut Option<T>) {
    *value = None;
}

/// The fields of `image` its check and headers give.
#[inline(never)]
fn app_image(image: &mut v1::AppImage, app: &AppSlot) {
    let found = app.check.map(|check| check.image);
    image.status = match found {
        None => v1::ImageStatus::Checking,
        Some(Image::Valid(_)) => v1::ImageStatus::Valid,
        Some(Image::Empty) => v1::ImageStatus::Empty,
        Some(Image::Invalid(_)) => v1::ImageStatus::Invalid,
    };
    image.checked_uptime_s = app.check.map(|check| check.finished.as_secs());
    image.error = match found {
        Some(Image::Invalid(err)) => Some(fault(err)),
        _ => None,
    };
    image.size = app.info.ok().map(|info| info.size());
    (image.digest, image.file_sha256) = match found {
        Some(Image::Valid(valid)) => (
            Some(v1::Sha256(*valid.digest())),
            Some(v1::Sha256(*valid.file_sha256())),
        ),
        _ => (None, None),
    };
}

#[inline(never)]
fn app_descriptor(fields: &mut v1::AppDescriptor, descriptor: &AppDescriptor) {
    fields.project_name = text(&descriptor.project_name());
    fields.version = text(&descriptor.version());
    fields.date = text(&descriptor.date());
    fields.time = text(&descriptor.time());
    fields.idf_version = text(&descriptor.idf_version());
    fields.secure_version = descriptor.secure_version();
    fields.elf_sha256 = v1::Sha256(descriptor.elf_sha256());
}

/// The body of [`v1::GetRegions`].
static REGIONS: Shared<v1::Regions> = Shared::new(v1::Regions {
    regions: heapless::Vec::new(),
});

/// [`v1::GetRegions`].
pub(super) async fn get_regions(
    storage: StorageView,
    (): (),
    (): (),
) -> Result<SharedBody<v1::Regions>, Infallible> {
    let mut body = REGIONS.lock().await;
    regions(&mut body.regions, storage);
    Ok(body)
}

#[inline(never)]
fn regions(regions: &mut heapless::Vec<v1::RegionInfo, { v1::REGIONS_MAX }>, storage: StorageView) {
    regions.clear();
    for region in storage.regions() {
        let _ = regions.push(region_info(&region));
    }
    // A table found at another offset can lie past a partition of the layout.
    regions.sort_unstable_by_key(|region| (region.offset, region.size));
}

// Every region fits in the response: the boot chain's two, and the layout's
// partitions.
const _: () = assert!(2 + layout::PARTITIONS.len() <= v1::REGIONS_MAX);

#[inline(never)]
fn region_info(region: &Region) -> v1::RegionInfo {
    let roles = [
        (region.roles.running, v1::Role::Running),
        (region.roles.selected, v1::Role::Selected),
        (region.roles.rollback_target, v1::Role::RollbackTarget),
    ];
    v1::RegionInfo {
        region: self::region(region.name),
        label: region.entry.map(|entry| label(entry.label)),
        partition: region.entry.map(|entry| v1::PartitionKind {
            kind: entry.kind,
            subtype: entry.subtype,
            flags: entry.flags,
        }),
        offset: region.offset,
        size: region.size,
        expected: region
            .expected
            .map(|(offset, size)| v1::ExpectedSpan { offset, size }),
        contents: match region.name.contents() {
            Contents::Public => v1::Contents::Public,
            Contents::Sensitive => v1::Contents::Sensitive,
            Contents::Reserved => v1::Contents::Reserved,
        },
        available: region.available,
        layout: region.layout.map(layout_match),
        roles: roles
            .into_iter()
            .filter_map(|(has, role)| has.then_some(role))
            .collect(),
    }
}

fn layout_match(comparison: Match) -> v1::LayoutMatch {
    let (status, differing) = match comparison {
        Match::Matches => (v1::MatchStatus::Matches, Fields::NONE),
        Match::Differs(fields) => (v1::MatchStatus::Differs, fields),
        Match::Missing => (v1::MatchStatus::Missing, Fields::NONE),
    };
    let fields = [
        (Fields::LABEL, v1::EntryField::Label),
        (Fields::KIND, v1::EntryField::Type),
        (Fields::SUBTYPE, v1::EntryField::Subtype),
        (Fields::OFFSET, v1::EntryField::Offset),
        (Fields::SIZE, v1::EntryField::Size),
        (Fields::FLAGS, v1::EntryField::Flags),
    ];
    v1::LayoutMatch {
        status,
        fields: fields
            .into_iter()
            .filter_map(|(field, name)| differing.contains(field).then_some(name))
            .collect(),
    }
}

/// The body of [`v1::GetPartitionTable`].
static BOARD_TABLE: Shared<v1::BoardTable> = Shared::new(v1::BoardTable {
    search: v1::TableSearch::None,
    offsets: heapless::Vec::new(),
    table: Some(v1::TableRead {
        offset: 0,
        entries: heapless::Vec::new(),
        truncated: false,
        end: v1::TableEnd::Unterminated,
        md5: None,
    }),
});

/// [`v1::GetPartitionTable`].
pub(super) async fn get_partition_table(
    storage: StorageView,
    (): (),
    (): (),
) -> Result<SharedBody<v1::BoardTable>, Infallible> {
    let mut body = BOARD_TABLE.lock().await;
    board_table(&mut body, storage);
    Ok(body)
}

#[inline(never)]
fn board_table(board: &mut v1::BoardTable, storage: StorageView) {
    let report = storage.report();
    board.offsets.clear();
    board.search = match report.found_table {
        // Not searched: the table at the compiled offset matches and the
        // bootloader is known to read it, or the app booted from no offset
        // the flash has.
        None if report.table.matches() => {
            let _ = board.offsets.push(layout::TABLE_OFFSET);
            v1::TableSearch::Compiled
        }
        None | Some(Found::Nothing) => v1::TableSearch::None,
        Some(Found::One(offset)) => {
            let _ = board.offsets.push(offset);
            v1::TableSearch::Found
        }
        Some(Found::Several(first, second)) => {
            let _ = board.offsets.push(first);
            let _ = board.offsets.push(second);
            v1::TableSearch::Several
        }
    };
    // The boot chain's table is the same for the whole boot, so a body
    // without one never has one again.
    match (storage.public().boot.table, board.table.as_mut()) {
        (Some(handle), Some(read)) => table_read(read, &handle),
        (Some(_), None) => {}
        (None, _) => clear(&mut board.table),
    }
}

/// The entries of the table `handle` covers, up to
/// [`v1::TABLE_ENTRIES_MAX`], and how they end.
#[inline(never)]
fn table_read(read: &mut v1::TableRead, handle: &Partition<'static, PartitionTable>) {
    read.offset = handle.offset();
    read.entries.clear();
    read.truncated = false;
    let mut entries = table::entries(handle, 0);
    for entry in entries.by_ref().map_while(Result::ok) {
        read.truncated |= read.entries.push(table_entry(&entry)).is_err();
    }
    let end = entries.finish();
    read.md5 = table_md5(end);
    read.end = match end {
        Ok(table::End::Md5 { .. }) => v1::TableEnd::Md5,
        // An erased first entry: a table of no entries.
        Ok(table::End::Erased { .. }) | Err(TableError::Empty) => v1::TableEnd::Erased,
        Err(TableError::Magic { .. }) => v1::TableEnd::Malformed,
        Err(TableError::Unterminated) => v1::TableEnd::Unterminated,
        Err(TableError::Read { .. }) => v1::TableEnd::ReadFailed,
    };
}

/// The MD5 entry a table's entries end at, when they end at one.
fn table_md5(end: Result<table::End, TableError>) -> Option<v1::TableMd5> {
    match end {
        Ok(table::End::Md5 {
            digest, matches, ..
        }) => Some(v1::TableMd5 {
            stored: v1::Md5(digest),
            matches,
        }),
        _ => None,
    }
}

fn table_entry(entry: &RawEntry) -> v1::TableEntry {
    v1::TableEntry {
        label: text(&entry.label),
        kind: entry.kind,
        subtype: entry.subtype,
        offset: entry.offset,
        size: entry.size,
        flags: entry.flags,
    }
}

/// The state [`get_region_content`] takes: the storage service, and the
/// device's name, which names the file.
#[derive(Clone, Copy)]
pub(super) struct Downloads {
    storage: StorageView,
    system: &'static System,
}

impl FromRef<AppState> for Downloads {
    fn from_ref(state: &AppState) -> Self {
        Self {
            storage: state.storage,
            system: state.system,
        }
    }
}

/// [`v1::GetRegionContent`]: the region's content, with the buffer that holds
/// the download lock until the response is written or abandoned.
pub(super) async fn get_region_content(
    downloads: Downloads,
    region: v1::Region,
    (): (),
) -> Result<Download<RegionContent, Buffer>, v1::RegionContentError> {
    region_content(downloads, region)
}

#[inline(never)]
fn region_content(
    downloads: Downloads,
    region: v1::Region,
) -> Result<Download<RegionContent, Buffer>, v1::RegionContentError> {
    // A region this version doesn't know never reaches a handler; were it to,
    // it is treated as one whose contents never leave the device.
    let name = region_name(region).ok_or(v1::RegionContentError::Sensitive)?;
    let download = downloads.storage.download(name).map_err(|err| match err {
        ContentError::Sensitive => v1::RegionContentError::Sensitive,
        ContentError::Unavailable => v1::RegionContentError::Unavailable,
        ContentError::Busy => v1::RegionContentError::Busy,
    })?;
    Ok(Download {
        source: region_source(download.content, downloads.system),
        buffer: download.buffer,
    })
}

/// The body's source: `content`, tagged, and named for the device.
#[inline(never)]
fn region_source(content: Content, system: &System) -> RegionContent {
    let tag = tag(content.boot(), content.generation());
    RegionContent {
        content,
        device: system.name(),
        tag,
    }
}

/// Longest entity tag of a region's content.
const TAG_MAX_LEN: usize = 32;

/// The strong entity tag `"<boot>-<generation>"`, the boot's random value in
/// 16 hex digits.
#[inline(never)]
fn tag(boot: u64, generation: u32) -> Option<heapless::String<TAG_MAX_LEN>> {
    let mut tag = heapless::String::new();
    // Quotes, 16 digits, a hyphen and at most 10 digits fit.
    write!(tag, "\"{boot:016x}-{generation}\"").ok()?;
    Some(tag)
}

/// The body of [`v1::GetRegionContent`]: a public region's content, tagged
/// with the boot's random value and the region's write generation, so the tag
/// changes on every boot and whenever the region may have.
pub(super) struct RegionContent {
    content: Content,
    device: heapless::String<NAME_MAX_LEN>,
    tag: Option<heapless::String<TAG_MAX_LEN>>,
}

impl Source for RegionContent {
    type Error = ContentReadError;

    fn len(&self) -> u64 {
        self.content.len().into()
    }

    fn tag(&self) -> Option<v1::EntityTag<'_>> {
        v1::EntityTag::new(self.tag.as_deref()?)
    }

    fn filename(&self, selection: v1::Selection) -> impl fmt::Display {
        let len = self.content.len();
        let (first, last) = match selection {
            v1::Selection::Part { first, last } => (
                u32::try_from(first).unwrap_or(u32::MAX),
                u32::try_from(last).unwrap_or(u32::MAX),
            ),
            v1::Selection::Whole | v1::Selection::Unsatisfiable => (0, len.saturating_sub(1)),
        };
        Filename {
            device: &self.device,
            region: region_str(self.content.name()),
            slot: self.content.slot(),
            part: self.content.part(first, last),
        }
    }

    async fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<(), ContentReadError> {
        let offset =
            u32::try_from(offset).map_err(|_| ContentReadError::Read(ReadError::OutOfRange))?;
        self.content.read(offset, buffer).await
    }
}

/// The name a download is saved under:
/// `<device>-<region>[-<slot>][-<what>].bin`, where what is
/// `image-<hash>` for exactly the image, `slot-of-<hash>` for a whole app slot
/// holding it, `0x<first>-0x<last>` for any other range, and nothing for a
/// whole region. The hash is the first 8 hex digits of the image's SHA-256.
struct Filename<'a> {
    device: &'a str,
    region: &'static str,
    slot: Option<Slot>,
    part: Part,
}

impl fmt::Display for Filename<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.device, self.region)?;
        if let Some(slot) = self.slot {
            write!(f, "-{}", slot.partition().label)?;
        }
        let short = |sha256: &[u8; 32]| {
            let [first, second, third, fourth, ..] = *sha256;
            u32::from_be_bytes([first, second, third, fourth])
        };
        match self.part {
            Part::Whole => {}
            Part::Image(sha256) => write!(f, "-image-{:08x}", short(&sha256))?,
            Part::SlotOf(sha256) => write!(f, "-slot-of-{:08x}", short(&sha256))?,
            Part::Range { first, last } => write!(f, "-{first:#x}-{last:#x}")?,
        }
        f.write_str(".bin")
    }
}

fn region_name(region: v1::Region) -> Option<RegionName> {
    Some(match region {
        v1::Region::Bootloader => RegionName::Bootloader,
        v1::Region::PartitionTable => RegionName::PartitionTable,
        v1::Region::Otadata => RegionName::Otadata,
        v1::Region::AppCurrent => RegionName::App(AppRole::Current),
        v1::Region::AppAlternate => RegionName::App(AppRole::Alternate),
        v1::Region::ConfigCurrent => RegionName::Config(AppRole::Current),
        v1::Region::ConfigAlternate => RegionName::Config(AppRole::Alternate),
        v1::Region::Diag => RegionName::Diag,
        v1::Region::Factory => RegionName::Factory,
        v1::Region::Reserved => RegionName::Reserved,
        v1::Region::Unknown => return None,
    })
}

fn region(name: RegionName) -> v1::Region {
    match name {
        RegionName::Bootloader => v1::Region::Bootloader,
        RegionName::PartitionTable => v1::Region::PartitionTable,
        RegionName::Otadata => v1::Region::Otadata,
        RegionName::App(AppRole::Current) => v1::Region::AppCurrent,
        RegionName::App(AppRole::Alternate) => v1::Region::AppAlternate,
        RegionName::Config(AppRole::Current) => v1::Region::ConfigCurrent,
        RegionName::Config(AppRole::Alternate) => v1::Region::ConfigAlternate,
        RegionName::Diag => v1::Region::Diag,
        RegionName::Factory => v1::Region::Factory,
        RegionName::Reserved => v1::Region::Reserved,
    }
}

/// The region's name as its path segment writes it.
fn region_str(name: RegionName) -> &'static str {
    match name {
        RegionName::Bootloader => "bootloader",
        RegionName::PartitionTable => "partition_table",
        RegionName::Otadata => "otadata",
        RegionName::App(AppRole::Current) => "app_current",
        RegionName::App(AppRole::Alternate) => "app_alternate",
        RegionName::Config(AppRole::Current) => "config_current",
        RegionName::Config(AppRole::Alternate) => "config_alternate",
        RegionName::Diag => "diag",
        RegionName::Factory => "factory",
        RegionName::Reserved => "reserved",
    }
}

fn tier(tier: Tier) -> v1::Tier {
    match tier {
        Tier::Untrusted => v1::Tier::Untrusted,
        Tier::Data => v1::Tier::Data,
        Tier::Full => v1::Tier::Full,
    }
}

fn limit(limit: Limit) -> v1::Limit {
    match limit {
        Limit::Table => v1::Limit::Table,
        Limit::FlashSize => v1::Limit::FlashSize,
        Limit::NotASlot => v1::Limit::NotASlot,
        Limit::OtherTable => v1::Limit::OtherTable,
        Limit::BootloaderInvalid => v1::Limit::BootloaderInvalid,
        Limit::BootloaderUnknown => v1::Limit::BootloaderUnlisted,
        Limit::AppDescriptor => v1::Limit::AppDescriptor,
    }
}

fn slot(slot: Slot) -> v1::SlotName {
    match slot {
        Slot::Ota0 => v1::SlotName::Ota0,
        Slot::Ota1 => v1::SlotName::Ota1,
    }
}

fn ota_state(state: State) -> v1::OtaState {
    match state {
        State::New => v1::OtaState::New,
        State::PendingVerify => v1::OtaState::PendingVerify,
        State::Valid => v1::OtaState::Valid,
        State::Invalid => v1::OtaState::Invalid,
        State::Aborted => v1::OtaState::Aborted,
        State::Undefined => v1::OtaState::Undefined,
        State::Unknown(_) => v1::OtaState::Other,
    }
}

fn fault(err: ImageError) -> v1::ImageFault {
    match err {
        ImageError::Read(_) => v1::ImageFault::ReadFailed,
        ImageError::PastEnd => v1::ImageFault::PastEnd,
        ImageError::Magic(_) => v1::ImageFault::Magic,
        ImageError::Chip(_) => v1::ImageFault::Chip,
        ImageError::TooManySegments(_) => v1::ImageFault::TooManySegments,
        ImageError::SegmentLength { .. } => v1::ImageFault::SegmentLength,
        ImageError::SegmentPastEnd { .. } => v1::ImageFault::SegmentPastEnd,
        ImageError::SegmentMapping { .. } => v1::ImageFault::SegmentMapping,
        ImageError::Checksum { .. } => v1::ImageFault::Checksum,
        ImageError::NoDigest => v1::ImageFault::NoDigest,
        ImageError::DigestMismatch => v1::ImageFault::DigestMismatch,
        ImageError::MissingDescriptor => v1::ImageFault::MissingDescriptor,
    }
}

/// `text` as printable ASCII, any other byte written `?`, cut to `N` bytes.
fn text<const M: usize, const N: usize>(text: &Text<M>) -> heapless::String<N> {
    let mut ascii = heapless::String::new();
    for &byte in text.as_bytes() {
        let c = if byte == b' ' || byte.is_ascii_graphic() {
            char::from(byte)
        } else {
            '?'
        };
        if ascii.push(c).is_err() {
            break;
        }
    }
    ascii
}

/// A label of the layout, which is ASCII and fits.
fn label(label: &str) -> v1::Label {
    let mut ascii = heapless::String::new();
    for c in label.chars() {
        if ascii.push(c).is_err() {
            break;
        }
    }
    ascii
}
