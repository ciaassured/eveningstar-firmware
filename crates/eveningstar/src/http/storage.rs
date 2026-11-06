//! `/storage`: the internal flash as a medium. The handlers map the storage
//! and boot crates' types to the API's, and read the flash only through the
//! content modules' models and published views.
//!
//! Every document and element is built by a function that isn't `async`
//! and isn't inlined, so each is assembled in a frame of its own, and only
//! writing the body awaits. `/storage/layout` and `/storage/regions` are
//! written in parts ([`JsonParts`]), each element built as it is written.

use core::convert::Infallible;

use defmt::{Debug2Format, warn};
use eveningstar_api::v1::storage::{self as api, Label};
use eveningstar_boot::app::AppRole;
use eveningstar_boot::table::{
    Coverage, EXPECTED, PartitionRow, Rows, TableFailure, TableModel, Valid,
};
use eveningstar_boot::{AppSlot, ImageFailure, LoadError, Otadata, Unpublished};
use eveningstar_storage::chain::{Step, TableSource, Tier};
use eveningstar_storage::otadata::Sector;
use eveningstar_storage::table::{Fields, Found, Match};
use eveningstar_storage::{
    Epoch, PublicRead, PublishError, ReadError, SlotPartitions as _, Span, WriteState, known,
    layout,
};

use super::api::{Incomplete, JsonParts, Part, Parts};
use crate::services::storage::Storage;

// The API's bounds hold what the storage crates give.
const _: () = assert!(eveningstar_storage::MAX_SANITIZED <= api::SANITIZED_MAX);
const _: () = assert!(EXPECTED.partitions.len() <= api::EXPECTED_MAX);
const _: () = assert!(eveningstar_storage::table::MAX_ENTRIES - 1 <= api::FOUND_MAX);
const _: () = {
    let mut i = 0;
    while i < known::BOOTLOADERS.len() {
        assert!(known::BOOTLOADERS[i].name.len() <= api::BOOTLOADER_NAME_MAX_LEN);
        i += 1;
    }
};

/// [`api::GetStorage`].
pub(super) async fn get_storage(
    storage: &'static Storage,
    (): (),
    (): (),
) -> Result<api::Storage, Infallible> {
    Ok(storage_document(storage))
}

#[inline(never)]
fn storage_document(storage: &Storage) -> api::Storage {
    let report = storage.report();
    let devices = [
        api::Device {
            device: api::DeviceName::Internal,
            kind: api::DeviceKind::NorFlash,
            status: api::DeviceStatus::Available,
            // 0 when the JEDEC ID isn't one the driver knows.
            capacity: (report.capacity != 0).then_some(report.capacity),
        },
        api::Device {
            device: api::DeviceName::External,
            kind: api::DeviceKind::NandFlash,
            status: api::DeviceStatus::NotSupported,
            capacity: None,
        },
    ];
    let bootloader = &report.bootloader;
    let known = bootloader.known.filter(|_| bootloader.image.is_ok());
    let booted_label = report
        .table
        .partitions
        .and_then(|partitions| partitions.booted)
        .and_then(|span| located_entry(storage, span))
        .and_then(|entry| entry.label);
    api::Storage {
        devices: heapless::Vec::from_iter(devices),
        chain: api::Chain {
            tier: tier(report.tier),
            failed: report.failed.map(step),
            steps: api::Steps {
                bootloader: report.passed(Step::Bootloader),
                table_offset: report.passed(Step::TableOffset),
                table: report.passed(Step::Table),
                layout: report.passed(Step::Layout),
                flash_size: report.passed(Step::FlashSize),
                booted_slot: report.passed(Step::BootedSlot),
            },
            bootloader: api::ChainBootloader {
                status: match (&bootloader.image, known) {
                    (Ok(_), Some(_)) => api::BootloaderStatus::Known,
                    (Ok(_), None) => api::BootloaderStatus::Unlisted,
                    (Err(_), _) => api::BootloaderStatus::Invalid,
                },
                // Every known name fits, as asserted above.
                name: known.and_then(|known| heapless::String::try_from(known.name).ok()),
                rollback: known.map(|known| known.rollback),
                fits: bootloader.fits,
            },
            // `split` is given `u32::MAX` when MMU entry 0 maps no page.
            booted_offset: (report.booted != u32::MAX).then_some(report.booted),
            booted_label,
        },
        allows: api::Allows {
            persistent_config: report.tier >= Tier::Layout,
            updates: report.tier == Tier::Boot,
        },
    }
}

const fn tier(tier: Tier) -> api::Tier {
    match tier {
        Tier::Unverified => api::Tier::Unverified,
        Tier::Layout => api::Tier::Layout,
        Tier::Boot => api::Tier::Boot,
    }
}

const fn step(step: Step) -> api::Step {
    match step {
        Step::Bootloader => api::Step::Bootloader,
        Step::TableOffset => api::Step::TableOffset,
        Step::Table => api::Step::Table,
        Step::Layout => api::Step::Layout,
        Step::FlashSize => api::Step::FlashSize,
        Step::BootedSlot => api::Step::BootedSlot,
    }
}

/// [`api::GetLayout`], written in parts: the found rows are read from the
/// flash as they are written.
pub(super) async fn get_layout(
    storage: &'static Storage,
    (): (),
    (): (),
) -> Result<Parts<LayoutParts>, Infallible> {
    Ok(Parts(LayoutParts(storage)))
}

/// The body of [`api::GetLayout`].
pub(super) struct LayoutParts(&'static Storage);

impl JsonParts for LayoutParts {
    type Document = api::Layout;
    type Element = LayoutElement;

    fn parts(self) -> impl Iterator<Item = Result<Part<LayoutElement>, Incomplete>> {
        let module = self.0.table();
        LayoutCursor {
            storage: self.0,
            model: module.parse(),
            rows: None,
            stage: LayoutStage::Start,
        }
    }
}

/// An element of the layout document's lists, or its table.
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(super) enum LayoutElement {
    Table(api::Table),
    Expected(api::ExpectedPartition),
    Found(api::FoundRow),
}

/// The parts of the layout document, in order, each built when it is asked
/// for.
struct LayoutCursor {
    storage: &'static Storage,
    model: Result<TableModel, LoadError>,
    /// The found rows, once they are reached, when the table is valid.
    rows: Option<Rows<'static>>,
    stage: LayoutStage,
}

/// The part a [`LayoutCursor`] gives next.
#[derive(Clone, Copy)]
enum LayoutStage {
    Start,
    Table,
    ExpectedStart,
    Expected(usize),
    FoundStart,
    Found,
    Done,
}

/// A part of the layout document, or `None` after the last.
type LayoutItem = Option<Result<Part<LayoutElement>, Incomplete>>;

impl Iterator for LayoutCursor {
    type Item = Result<Part<LayoutElement>, Incomplete>;

    // Each part is built by a function of its own, straight into the
    // returned item, so no part passes through this frame.
    fn next(&mut self) -> LayoutItem {
        match self.stage {
            LayoutStage::Start => Some(Ok(self.literal(api::Layout::START, LayoutStage::Table))),
            LayoutStage::Table => Some(Ok(self.table())),
            LayoutStage::ExpectedStart => Some(Ok(
                self.literal(api::Layout::EXPECTED, LayoutStage::Expected(0))
            )),
            LayoutStage::Expected(index) if index < EXPECTED.partitions.len() => {
                Some(Ok(self.expected(index)))
            }
            LayoutStage::Expected(_) => {
                Some(Ok(self.literal(api::Layout::FOUND, LayoutStage::FoundStart)))
            }
            LayoutStage::FoundStart => Some(self.found_start()),
            LayoutStage::Found => Some(self.found()),
            LayoutStage::Done => None,
        }
    }
}

impl LayoutCursor {
    /// `literal`, then the part of `next`.
    const fn literal(&mut self, literal: &'static str, next: LayoutStage) -> Part<LayoutElement> {
        self.stage = next;
        Part::Literal(literal)
    }

    #[inline(never)]
    fn table(&mut self) -> Part<LayoutElement> {
        self.stage = LayoutStage::ExpectedStart;
        Part::Element(LayoutElement::Table(table_document(
            self.storage,
            &self.model,
        )))
    }

    #[inline(never)]
    fn expected(&mut self, index: usize) -> Part<LayoutElement> {
        self.stage = LayoutStage::Expected(index + 1);
        Part::Element(LayoutElement::Expected(expected_partition(
            &self.model,
            index,
        )))
    }

    #[inline(never)]
    fn found_start(&mut self) -> Result<Part<LayoutElement>, Incomplete> {
        match found_rows(self.storage, &self.model) {
            Ok(rows) => {
                self.rows = rows;
                self.stage = LayoutStage::Found;
                self.found()
            }
            Err(Incomplete) => Err(self.fail()),
        }
    }

    #[inline(never)]
    fn found(&mut self) -> Result<Part<LayoutElement>, Incomplete> {
        match self.rows.as_mut().and_then(Iterator::next) {
            Some(Ok(row)) => Ok(Part::Element(LayoutElement::Found(found_row(row)))),
            Some(Err(_)) => {
                warn!("The table's partitions failed to read");
                Err(self.fail())
            }
            None => Ok(self.literal(api::Layout::END, LayoutStage::Done)),
        }
    }

    /// Ends the parts with an element that can't be built.
    const fn fail(&mut self) -> Incomplete {
        self.stage = LayoutStage::Done;
        Incomplete
    }
}

/// The rows the bootloader loads from the table `model` describes, when it
/// is valid.
#[inline(never)]
fn found_rows(
    storage: &'static Storage,
    model: &Result<TableModel, LoadError>,
) -> Result<Option<Rows<'static>>, Incomplete> {
    let Ok(model) = model else {
        return Ok(None);
    };
    if model.verdict().is_err() {
        return Ok(None);
    }
    match storage.table().partitions(model) {
        Ok(rows) => Ok(Some(rows)),
        Err(error) => {
            warn!(
                "The table's partitions can't be read again: {}",
                Debug2Format(&error)
            );
            Err(Incomplete)
        }
    }
}

/// The table's part of the layout document, from `model`, the located
/// table parsed.
#[inline(never)]
fn table_document(storage: &Storage, model: &Result<TableModel, LoadError>) -> api::Table {
    let location = storage.table().location();
    let (search, candidates) = match location.search() {
        None => (api::Search::NotRun, heapless::Vec::new()),
        Some(Found::Nothing) => (api::Search::None, heapless::Vec::new()),
        Some(Found::One(_)) => (api::Search::One, heapless::Vec::new()),
        Some(Found::Several(first, second)) => (
            api::Search::Several,
            heapless::Vec::from_iter([first, second]),
        ),
    };
    let verdict = match model {
        _ if location.offset().is_none() => None,
        Ok(model) => Some(model.verdict()),
        Err(error) => {
            // `Busy` and `Stale` need a writer, which the table has none of.
            if !matches!(error, LoadError::Absent) {
                warn!("The located table can't be parsed: {}", Debug2Format(error));
            }
            None
        }
    };
    let status = match (location.offset(), &verdict) {
        (None, _) => api::TableStatus::NotLocated,
        (Some(_), None) => api::TableStatus::Unread,
        (Some(_), Some(Ok(_))) => api::TableStatus::Valid,
        (Some(_), Some(Err(_))) => api::TableStatus::Refused,
    };
    let valid = verdict.and_then(Result::ok);
    api::Table {
        status,
        expected_offset: layout::TABLE_OFFSET,
        offset: location.offset(),
        source: location.source().map(|source| match source {
            TableSource::KnownBootloader => api::TableSource::KnownBootloader,
            TableSource::Searched => api::TableSource::Searched,
        }),
        search,
        candidates,
        failure: verdict.and_then(Result::err).map(failure),
        coverage: valid.map(|valid| match valid.coverage() {
            Coverage::Layout => api::Coverage::Layout,
            Coverage::Md5 => api::Coverage::Md5,
            Coverage::Uncovered => api::Coverage::Uncovered,
        }),
        md5_read_as_partition: valid.map(Valid::md5_read_as_partition),
    }
}

fn failure(failure: TableFailure) -> api::TableFailure {
    let (code, index) = match failure {
        TableFailure::Magic { index } => (api::TableFailureCode::Magic, Some(index)),
        TableFailure::Empty => (api::TableFailureCode::Empty, None),
        TableFailure::Unterminated => (api::TableFailureCode::Unterminated, None),
        TableFailure::PastChip { index } => (api::TableFailureCode::PastChip, Some(index)),
        TableFailure::SecondMd5 { index } => (api::TableFailureCode::SecondMd5, Some(index)),
        TableFailure::Md5 { index } => (api::TableFailureCode::Md5, Some(index)),
        TableFailure::Read { index, .. } => (api::TableFailureCode::Read, Some(index)),
    };
    api::TableFailure {
        code,
        // A table has at most 96 entries.
        index: index.and_then(|index| u16::try_from(index).ok()),
    }
}

/// The compiled layout's partition at `index`, compared with the located
/// table's partition of its label when `model` is valid.
#[inline(never)]
fn expected_partition(
    model: &Result<TableModel, LoadError>,
    index: usize,
) -> api::ExpectedPartition {
    let comparison = model
        .as_ref()
        .ok()
        .and_then(|model| model.verdict().ok())
        .and_then(|valid| valid.comparison().expected.get(index).copied())
        .unwrap_or(Match::Missing);
    let entry = EXPECTED
        .partitions
        .get(index)
        .copied()
        .unwrap_or(layout::RESERVED);
    api::ExpectedPartition {
        label: label_of(entry.label),
        partition_type: entry.kind,
        subtype: entry.subtype,
        offset: entry.offset,
        size: entry.size,
        flags: entry.flags,
        comparison: compared(comparison),
    }
}

fn compared(comparison: Match) -> api::Comparison {
    let (status, fields) = match comparison {
        Match::Matches => (api::ComparisonStatus::Matches, Fields::NONE),
        Match::Differs(fields) => (api::ComparisonStatus::Differs, fields),
        Match::Missing => (api::ComparisonStatus::Missing, Fields::NONE),
    };
    let names = [
        (Fields::KIND, api::Field::Type),
        (Fields::SUBTYPE, api::Field::Subtype),
        (Fields::OFFSET, api::Field::Offset),
        (Fields::SIZE, api::Field::Size),
        (Fields::FLAGS, api::Field::Flags),
    ];
    api::Comparison {
        status,
        fields: names
            .into_iter()
            .filter(|&(field, _)| fields.contains(field))
            .map(|(_, name)| name)
            .collect(),
    }
}

#[inline(never)]
fn found_row(row: PartitionRow) -> api::FoundRow {
    match row {
        PartitionRow::Partition(partition) => {
            let label = partition.label();
            api::FoundRow {
                kind: api::RowKind::Partition,
                label: label.map(|label| Label::from_raw(label.raw())),
                partition_type: Some(partition.kind()),
                subtype: Some(partition.subtype()),
                offset: Some(partition.offset()),
                size: Some(partition.size()),
                flags: Some(partition.flags()),
                extra: label.map(|label| {
                    !EXPECTED
                        .partitions
                        .iter()
                        .any(|expected| label.is_padded(expected.label))
                }),
            }
        }
        PartitionRow::Md5 { .. } => api::FoundRow {
            kind: api::RowKind::Md5,
            label: None,
            partition_type: None,
            subtype: None,
            offset: None,
            size: None,
            flags: None,
            extra: None,
        },
    }
}

/// [`api::GetRegions`], written in parts, each region's document built as it
/// is written.
pub(super) async fn get_regions(
    storage: &'static Storage,
    (): (),
    (): (),
) -> Result<Parts<RegionsParts>, Infallible> {
    Ok(Parts(RegionsParts(storage)))
}

/// The body of [`api::GetRegions`].
pub(super) struct RegionsParts(&'static Storage);

impl JsonParts for RegionsParts {
    type Document = api::Regions;
    type Element = api::Region;

    fn parts(self) -> impl Iterator<Item = Result<Part<api::Region>, Incomplete>> {
        let storage = self.0;
        let regions = listed(storage);
        let count = regions.len();
        (0..count + 2).map(move |index| {
            Ok(match index.checked_sub(1).map(|index| regions.get(index)) {
                None => Part::Literal(api::Regions::START),
                Some(Some(&(name, span))) => Part::Element(region(storage, name, span)),
                Some(None) => Part::Literal(api::Regions::END),
            })
        })
    }
}

/// [`api::GetRegion`].
pub(super) async fn get_region(
    storage: &'static Storage,
    name: api::RegionName,
    (): (),
) -> Result<api::Region, api::RegionError> {
    region_document(storage, name)
}

/// The document of the region `name`, if it is listed.
#[inline(never)]
fn region_document(
    storage: &Storage,
    name: api::RegionName,
) -> Result<api::Region, api::RegionError> {
    for &(listed, span) in &listed(storage) {
        if listed == name {
            return Ok(region(storage, name, span));
        }
    }
    Err(api::RegionError::NotFound)
}

/// The regions listed this boot, with their spans, in offset order: the
/// publishable ones whose reader the firmware holds, at the reader's span,
/// and the others whose span is known.
#[inline(never)]
fn listed(storage: &Storage) -> heapless::Vec<(api::RegionName, Span), { api::REGIONS_MAX }> {
    use api::RegionName as Name;

    let mut regions = heapless::Vec::new();
    let mut list = |name, span: Option<Span>| {
        if let Some(span) = span {
            // One entry per name, so the list holds them all.
            let _ = regions.push((name, span));
        }
    };
    let app = storage.app();
    list(Name::Bootloader, Some(storage.bootloader().span()));
    list(Name::PartitionTable, storage.table().span());
    list(Name::Otadata, storage.otadata().map(Otadata::span));
    list(
        Name::AppCurrent,
        app.current()
            .map(AppSlot::span)
            .or_else(|| app.found_app().map(AppSlot::span)),
    );
    list(Name::AppAlternate, app.alternate().map(AppSlot::span));
    // The layout's spans are the located table's in the layout and boot
    // tiers, which alone have a slot; below them only what the chain located
    // is listed.
    if let Some(slot) = storage.report().slot {
        let span = |entry: eveningstar_storage::Entry| Some(Span::new(entry.offset, entry.size));
        list(Name::Factory, span(layout::FACTORY));
        list(Name::ConfigCurrent, span(slot.config()));
        list(Name::ConfigAlternate, span(slot.other().config()));
        list(Name::Diag, span(layout::DIAG));
        list(Name::Reserved, span(layout::RESERVED));
    }
    regions.sort_unstable_by_key(|&(_, span)| span.offset);
    regions
}

/// The document of the region `name`, listed at `span`.
#[inline(never)]
fn region(storage: &Storage, name: api::RegionName, span: Span) -> api::Region {
    use api::RegionName as Name;

    let class = match name {
        Name::Bootloader | Name::PartitionTable | Name::AppCurrent | Name::AppAlternate => {
            api::Class::Publishable
        }
        // A located table's otadata: its parser can't pin down an entry of a
        // layout it doesn't know.
        Name::Otadata if storage.otadata().is_some_and(Otadata::is_found) => api::Class::Sensitive,
        Name::Otadata => api::Class::Publishable,
        Name::Factory | Name::ConfigCurrent | Name::ConfigAlternate | Name::Diag => {
            api::Class::Sensitive
        }
        Name::Reserved => api::Class::Reserved,
    };
    let partition = match name {
        Name::Bootloader | Name::PartitionTable => None,
        _ => located_entry(storage, span),
    };
    let mut views = Views::new();
    if class == api::Class::Publishable {
        add_views(storage, name, &mut views);
    }
    api::Region {
        region: name,
        device: api::DeviceName::Internal,
        class,
        span: api::Span {
            offset: span.offset,
            size: span.len,
        },
        partition: partition.map(|entry| api::Partition {
            label: entry.label,
            partition_type: entry.kind,
            subtype: entry.subtype,
            flags: entry.flags,
        }),
        views,
    }
}

/// Adds the views of the publishable region `name` to `views`, in the order
/// its format defines them.
fn add_views(storage: &Storage, name: api::RegionName, views: &mut Views) {
    use api::RegionName as Name;

    let app = storage.app();
    match name {
        Name::Bootloader => bootloader_view(storage, views),
        Name::PartitionTable => table_view(storage, views),
        Name::Otadata => {
            otadata_view(storage, Sector::First, api::ViewName::Entry0, views);
            otadata_view(storage, Sector::Second, api::ViewName::Entry1, views);
        }
        Name::AppCurrent => match (app.current(), app.found_app()) {
            (Some(slot), _) => app_view(storage, slot, views),
            (None, Some(slot)) => app_view(storage, slot, views),
            (None, None) => {}
        },
        Name::AppAlternate => {
            if let Some(slot) = app.alternate() {
                app_view(storage, slot, views);
            }
        }
        Name::Factory
        | Name::ConfigCurrent
        | Name::ConfigAlternate
        | Name::Diag
        | Name::Reserved => {}
    }
}

/// A region's views.
type Views = heapless::Vec<api::View, { api::VIEWS_MAX }>;

/// Adds `view` to `views`. No region has more than [`api::VIEWS_MAX`].
fn add(views: &mut Views, view: api::View) {
    let _ = views.push(view);
}

/// `text`, a label of the compiled layout, as the API writes labels.
fn label_of(text: &str) -> Label {
    let mut raw = [0; api::LABEL_MAX_LEN];
    let len = text.len().min(raw.len());
    raw[..len].copy_from_slice(&text.as_bytes()[..len]);
    Label::from_raw(&raw)
}

/// A partition entry of the located table.
struct LocatedEntry {
    label: Option<Label>,
    kind: u8,
    subtype: u8,
    flags: u32,
}

/// The located table's first partition entry at `span`, when the table is
/// valid and its partitions read.
#[inline(never)]
fn located_entry(storage: &Storage, span: Span) -> Option<LocatedEntry> {
    let module = storage.table();
    let Ok(model) = module.parse() else {
        return None;
    };
    let Ok(mut rows) = module.partitions(&model) else {
        return None;
    };
    rows.find_map(|row| match row {
        Ok(PartitionRow::Partition(entry))
            if entry.offset() == span.offset && entry.size() == span.len =>
        {
            Some(LocatedEntry {
                label: entry.label().map(|label| Label::from_raw(label.raw())),
                kind: entry.kind(),
                subtype: entry.subtype(),
                flags: entry.flags(),
            })
        }
        _ => None,
    })
}

/// A view that isn't published, with `status`.
const fn unpublished(view: api::ViewName, status: api::ViewStatus) -> api::View {
    api::View {
        view,
        status,
        offset: None,
        length: None,
        sanitized: None,
        etag: None,
    }
}

/// A published view: at `span` within its region, with the `sanitized`
/// spans, at `epoch`.
fn published(
    storage: &Storage,
    view: api::ViewName,
    span: Span,
    sanitized: &[Span],
    epoch: Epoch,
) -> api::View {
    api::View {
        view,
        status: api::ViewStatus::Published,
        offset: Some(span.offset),
        length: Some(span.len),
        sanitized: Some(
            sanitized
                .iter()
                .map(|span| api::Span {
                    offset: span.offset,
                    size: span.len,
                })
                .collect(),
        ),
        etag: Some(storage.tag(epoch)),
    }
}

/// The status of a view that `error` kept from being published, `writing`
/// telling whether a session is open on the region now: while one is, the
/// region is being written, and once it is stable at a newer epoch, its
/// contents wait to be checked again.
fn refused(error: Unpublished, writing: impl FnOnce() -> bool) -> api::ViewStatus {
    match error {
        Unpublished::Unverified => api::ViewStatus::Unverified,
        Unpublished::Stale | Unpublished::Refused(PublishError::Stale) => {
            if writing() {
                api::ViewStatus::Writing
            } else {
                api::ViewStatus::Checking
            }
        }
        Unpublished::Refused(PublishError::Device) => api::ViewStatus::Unreadable,
        Unpublished::Refused(error) => {
            warn!("The storage crate refused a view: {}", Debug2Format(&error));
            api::ViewStatus::Unreadable
        }
    }
}

/// The status of a view whose model `error` kept from loading.
fn not_loaded(error: LoadError, writing: impl FnOnce() -> bool) -> api::ViewStatus {
    match error {
        LoadError::Busy => api::ViewStatus::Writing,
        LoadError::Stale => refused(Unpublished::Stale, writing),
        LoadError::Absent => api::ViewStatus::Unverified,
    }
}

/// Whether a model's `image` failed to verify because the flash failed to
/// read.
const fn unreadable<T>(image: &Result<T, ImageFailure>) -> bool {
    matches!(image, Err(ImageFailure::Read(ReadError::Device)))
}

/// Whether a session is open on a region in `state`.
const fn writing(state: WriteState) -> bool {
    matches!(state, WriteState::Writing(_))
}

#[inline(never)]
fn bootloader_view(storage: &Storage, views: &mut Views) {
    add(views, bootloader_status(storage));
}

fn bootloader_status(storage: &Storage) -> api::View {
    const VIEW: api::ViewName = api::ViewName::Image;
    let module = storage.bootloader();
    // The model loaded at boot. The region has no writer, so it stays at
    // that model's epoch, and loading it fails only for a region being
    // written.
    let Some(model) = module.model() else {
        return unpublished(VIEW, api::ViewStatus::Checking);
    };
    if unreadable(&model.image()) {
        return unpublished(VIEW, api::ViewStatus::Unreadable);
    }
    match module.published(&model) {
        Ok(view) => published(storage, VIEW, view.span(), view.sanitized(), view.epoch()),
        Err(error) => unpublished(VIEW, refused(error, || false)),
    }
}

#[inline(never)]
fn table_view(storage: &Storage, views: &mut Views) {
    add(views, table_status(storage));
}

fn table_status(storage: &Storage) -> api::View {
    const VIEW: api::ViewName = api::ViewName::Table;
    let module = storage.table();
    // The table's region has no writer.
    let model = match module.parse() {
        Ok(model) => model,
        Err(error) => return unpublished(VIEW, not_loaded(error, || false)),
    };
    if matches!(
        model.verdict(),
        Err(TableFailure::Read {
            error: ReadError::Device,
            ..
        })
    ) {
        return unpublished(VIEW, api::ViewStatus::Unreadable);
    }
    match module.published(&model) {
        Ok(view) => published(storage, VIEW, view.span(), view.sanitized(), view.epoch()),
        Err(error) => unpublished(VIEW, refused(error, || false)),
    }
}

#[inline(never)]
fn otadata_view(storage: &Storage, sector: Sector, view: api::ViewName, views: &mut Views) {
    add(views, otadata_status(storage, sector, view));
}

fn otadata_status(storage: &Storage, sector: Sector, view: api::ViewName) -> api::View {
    let Some(module) = storage.otadata() else {
        return unpublished(view, api::ViewStatus::Unverified);
    };
    let state = || writing(module.write_state());
    let model = match module.parse() {
        Ok(model) => model,
        Err(error) => return unpublished(view, not_loaded(error, state)),
    };
    let entry = model.otadata().entries.get(sector.index());
    if matches!(entry, Some(Err(ReadError::Device))) {
        return unpublished(view, api::ViewStatus::Unreadable);
    }
    match module.published(&model, sector) {
        Ok(entry) => published(
            storage,
            view,
            entry.span(),
            entry.sanitized(),
            entry.epoch(),
        ),
        Err(error) => unpublished(view, refused(error, state)),
    }
}

#[inline(never)]
fn app_view<R: AppRole>(storage: &Storage, slot: &AppSlot<'_, R>, views: &mut Views) {
    add(views, app_status(storage, slot));
}

fn app_status<R: AppRole>(storage: &Storage, slot: &AppSlot<'_, R>) -> api::View {
    const VIEW: api::ViewName = api::ViewName::Image;
    let state = || writing(slot.write_state());
    if state() {
        return unpublished(VIEW, api::ViewStatus::Writing);
    }
    // No model at the region's epoch yet: the check task hashes the image.
    let Some(model) = slot.model() else {
        return unpublished(VIEW, refused(Unpublished::Stale, state));
    };
    if unreadable(&model.image()) {
        return unpublished(VIEW, api::ViewStatus::Unreadable);
    }
    match slot.published(&model) {
        Ok(view) => published(storage, VIEW, view.span(), view.sanitized(), view.epoch()),
        Err(error) => unpublished(VIEW, refused(error, state)),
    }
}
