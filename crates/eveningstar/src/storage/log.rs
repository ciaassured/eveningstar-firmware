//! What the chain and the content modules found, in the log.
//!
//! The functions that take a model are never inlined, so each model lives in
//! their frames only, one at a time.

use defmt::{Debug2Format, Format, Formatter, info, warn};
use eveningstar_boot::app::AppRole;
use eveningstar_boot::bootloader::BootloaderModel;
use eveningstar_boot::table::{Coverage, PartitionRow, TableModel, Valid};
use eveningstar_boot::{AppSlot, Bootloader, LoadError, Otadata, PartitionTable};
use eveningstar_ota::Ota;
use eveningstar_storage::chain::{LayoutComparison, Report, Step, TableSource, Tier};
use eveningstar_storage::otadata::{self, BootState, Sector, State, Validity};
use eveningstar_storage::table::{Fields, Found, Label, Match};
use eveningstar_storage::{
    Entry, Epoch, PublicRead as _, Slot, SlotPartitions as _, Span, Text, layout,
};

use super::{LocatedSpans, Modules};

/// Logs what the chain, whose report is `report`, and the modules found,
/// `loaded` being the result of loading the bootloader's model: the
/// bootloader, the table, the flash and the slot booted, the tier, what the
/// unverified tier located, and the otadata.
pub(super) fn modules(modules: &Modules, report: &Report, loaded: Result<(), LoadError>) {
    bootloader(modules.bootloader, loaded, report.bootloader.fits);
    table(modules.table);
    flash_and_slot(report);
    tier(report);
    if let Some(spans) = &modules.located {
        located(spans);
    }
    if let Some(module) = modules.otadata {
        otadata(module, report.slot);
    }
}

/// Logs the bootloader's model: its header, identity and known entry, and
/// whether its image is published.
#[inline(never)]
fn bootloader(module: &Bootloader<'_>, loaded: Result<(), LoadError>, fits: Option<bool>) {
    if let Err(err) = loaded {
        warn!("Bootloader not loaded: {}", Debug2Format(&err));
    }
    let Some(model) = module.model() else {
        return;
    };
    if let (Some(code), Some(size)) = (model.flash_size_code(), model.chip_size()) {
        info!(
            "Bootloader header: flash size code {=u8}, {=u32} bytes, the chip size the bootloader checks the table against",
            code, size
        );
    } else {
        warn!("Bootloader header doesn't read");
    }
    if let Err(failure) = model.header() {
        warn!("Bootloader header isn't valid: {}", Debug2Format(&failure));
    }
    match model.image() {
        Ok(image) => {
            if let Some(known) = image.known() {
                info!(
                    "Bootloader: {=str}, {=u32} bytes, file SHA-256 {}, reads the table at {=u32:#x}, rollback {=bool}",
                    known.name,
                    image.size(),
                    Hex(image.file_sha256()),
                    known.table_offset,
                    known.rollback
                );
            } else {
                warn!(
                    "Bootloader: unknown, {=u32} bytes, digest {}, file SHA-256 {}",
                    image.size(),
                    Hex(image.digest()),
                    Hex(image.file_sha256()),
                );
            }
        }
        Err(failure) => warn!(
            "Bootloader doesn't verify within its region: {}",
            Debug2Format(&failure)
        ),
    }
    if fits == Some(false) {
        warn!("Bootloader image runs past the table's offset");
    }
    bootloader_view(module, &model);
}

/// Logs whether the bootloader's image is published.
fn bootloader_view(module: &Bootloader<'_>, model: &BootloaderModel) {
    match module.published(model) {
        Ok(view) => info!("Bootloader image published: {=u32} bytes", view.len()),
        Err(err) => warn!("Bootloader image isn't published: {}", Debug2Format(&err)),
    }
}

/// Logs how the table was located, its validity, its partitions compared
/// with the layout, and what of it is published.
#[inline(never)]
fn table(module: &PartitionTable<'_>) {
    let location = module.location();
    match (location.source(), location.offset()) {
        (Some(TableSource::KnownBootloader), Some(offset)) => info!(
            "Partition table at {=u32:#x}, where the known bootloader reads it",
            offset
        ),
        (Some(TableSource::Searched), Some(offset)) => info!(
            "Partition table at {=u32:#x}, the one table the search found with an app at the booted offset",
            offset
        ),
        _ => match location.search() {
            Some(Found::Several(first, second)) => warn!(
                "Partition tables at {=u32:#x} and {=u32:#x} both have an app at the booted offset: no table is located",
                first, second
            ),
            Some(Found::Nothing | Found::One(_)) => {
                warn!("No partition table found with an app at the booted offset");
            }
            None => warn!("No partition table located: none searched for"),
        },
    }
    match module.parse() {
        Ok(model) => table_model(module, &model),
        Err(LoadError::Absent) => {}
        Err(err) => warn!("Partition table not parsed: {}", Debug2Format(&err)),
    }
}

/// Logs the table `model` describes.
#[inline(never)]
fn table_model(module: &PartitionTable<'_>, model: &TableModel) {
    let valid = match model.verdict() {
        Ok(valid) => valid,
        Err(failure) => {
            warn!(
                "Partition table isn't valid as the bootloader judges it: {}",
                Debug2Format(&failure)
            );
            return;
        }
    };
    if let Some(md5) = valid.md5() {
        info!(
            "Partition table valid: end entry at {=usize}, MD5 entry at {=usize} matches",
            valid.end(),
            md5.index
        );
    } else {
        info!(
            "Partition table valid: end entry at {=usize}, no MD5 entry",
            valid.end()
        );
    }
    info!(
        "The bootloader loads {=usize} partitions from it",
        valid.partitions()
    );
    if valid.md5_read_as_partition() {
        warn!(
            "Entries follow the MD5 entry: the bootloader reads the MD5 entry as a partition and leaves out the last entry"
        );
    }
    partitions(module, model);
    comparison(valid.comparison());
    table_view(module, model, valid);
}

/// Logs the partitions the bootloader loads from the table `model`
/// describes.
#[inline(never)]
fn partitions(module: &PartitionTable<'_>, model: &TableModel) {
    let rows = match module.partitions(model) {
        Ok(rows) => rows,
        Err(err) => {
            warn!("  partitions not read: {}", Debug2Format(&err));
            return;
        }
    };
    for row in rows {
        match row {
            Ok(row) => partition(&row),
            Err(err) => {
                warn!("  read failed: {}", Debug2Format(&err));
                return;
            }
        }
    }
}

/// Logs one of the partitions the bootloader loads.
#[inline(never)]
fn partition(row: &PartitionRow) {
    match row {
        PartitionRow::Partition(partition) => info!(
            "  {=usize}: {}, type {=u8:#x}, subtype {=u8:#x}, {=u32:#x}..{=u32:#x}, flags {=u32:#x}",
            partition.index(),
            Fmt(partition.label().copied()),
            partition.kind(),
            partition.subtype(),
            partition.offset(),
            partition.offset().saturating_add(partition.size()),
            partition.flags(),
        ),
        PartitionRow::Md5 { index } => {
            info!("  {=usize}: the MD5 entry, read as a partition", index);
        }
    }
}

/// Logs how the partitions compare with the layout, by label.
fn comparison(comparison: &LayoutComparison) {
    /// The layout's partitions, read in place rather than copied.
    static PARTITIONS: [Entry; layout::PARTITIONS.len()] = layout::PARTITIONS;

    if comparison.matches() {
        info!("Its partitions are the layout's");
        return;
    }
    warn!("Its partitions aren't the layout's:");
    for (expected, result) in PARTITIONS.iter().zip(&comparison.expected) {
        match *result {
            Match::Matches => info!("  {=str}: matches", expected.label),
            Match::Differs(fields) => {
                warn!("  {=str}: differs in {}", expected.label, Fmt(fields));
            }
            Match::Missing => warn!("  {=str}: missing", expected.label),
        }
    }
    if comparison.extra > 0 {
        warn!(
            "  {=usize} partitions with no label of the layout, or a label repeated",
            comparison.extra
        );
    }
}

/// Logs what covers the table `model` describes, and what of it is
/// published.
fn table_view(module: &PartitionTable<'_>, model: &TableModel, valid: &Valid) {
    match module.published(model) {
        Ok(view) => info!(
            "Partition table published: {=u32} bytes, covered by {}, {=usize} spans sanitized",
            view.len(),
            Fmt(valid.coverage()),
            view.sanitized().len(),
        ),
        Err(err) => warn!(
            "Partition table isn't published, covered by {}: {}",
            Fmt(valid.coverage()),
            Debug2Format(&err)
        ),
    }
}

/// Logs step 6: the flash's size and the slot booted.
fn flash_and_slot(report: &Report) {
    if !report.passed(Step::FlashSize) {
        warn!(
            "The flash holds {=u32} bytes, less than the layout's {=u32}",
            report.capacity,
            layout::FLASH_SIZE
        );
    }
    if let Some(slot) = report.slot {
        info!("Running from {}", Fmt(slot));
    } else if !report.passed(Step::BootedSlot) {
        warn!(
            "Running from {=u32:#x}, not as one of the layout's slots",
            report.booted
        );
    }
}

/// Logs the tier and the step that kept it below the boot tier.
fn tier(report: &Report) {
    match report.failed {
        None => info!("Storage: boot tier"),
        Some(step) => {
            warn!("Storage: {} tier, as {}", Fmt(report.tier), Fmt(step));
            match report.tier {
                Tier::Unverified => warn!("Running on defaults: nothing is written to the flash"),
                Tier::Layout => warn!(
                    "The boot selection isn't written: the bootloader isn't known to read the compiled table offset and roll back"
                ),
                Tier::Boot => {}
            }
        }
    }
}

/// Logs the spans of the readers the unverified tier located.
fn located(spans: &LocatedSpans) {
    let span = |span: Option<Span>, what: &str| {
        if let Some(span) = span {
            info!(
                "Located {=str}: {=u32:#x}..{=u32:#x}",
                what,
                span.offset,
                span.offset.saturating_add(span.len)
            );
        } else {
            info!(
                "Located {=str}: no reader (not located, or over a sensitive region)",
                what
            );
        }
    };
    span(spans.table, "table");
    span(spans.app, "app");
    span(spans.otadata, "otadata");
}

/// Logs both otadata entries, and the boot state of the app running from
/// `slot` when it is known.
#[inline(never)]
fn otadata(module: &Otadata<'_>, slot: Option<Slot>) {
    let model = match module.parse() {
        Ok(model) => model,
        Err(err) => {
            warn!("Otadata not read: {}", Debug2Format(&err));
            return;
        }
    };
    if module.is_found() {
        info!("Otadata, of the located table:");
    } else {
        info!("Otadata:");
    }
    entries(model.otadata());
    if let Some(slot) = slot {
        boot_state(&model.boot_state(slot));
    }
}

/// Logs both entries of `otadata`.
fn entries(otadata: &otadata::Otadata) {
    for sector in Sector::ALL {
        match otadata.entry(sector) {
            Ok(entry) if entry.validity() == Validity::Blank => {
                info!("  otadata[{}]: blank", sector.index());
            }
            Ok(entry) => info!(
                "  otadata[{}]: seq {}, {}, selects {}, {}",
                sector.index(),
                entry.seq,
                Fmt(entry.state),
                Fmt(entry.slot()),
                Fmt(entry.validity()),
            ),
            Err(err) => warn!(
                "  otadata[{}]: read failed: {}",
                sector.index(),
                Debug2Format(&err)
            ),
        }
    }
}

/// Logs `state`, the boot state of the running image, and warns when the
/// bootloader started another slot than the one selected.
pub(super) fn boot_state(state: &BootState) {
    info!(
        "Boot state: running {}, selected {}, state {}, rollback target {=bool}, needs confirmation {=bool}",
        Fmt(state.running),
        state.selected.map(Fmt),
        state.state.map(Fmt),
        state.rollback_target,
        state.needs_confirmation,
    );
    if let Some(selected) = state.selected.filter(|&selected| selected != state.running) {
        warn!(
            "The otadata selects {}, but {} is running: the bootloader fell back, as {} failed to load",
            Fmt(selected),
            Fmt(state.running),
            Fmt(selected),
        );
    }
}

/// Logs the result kept for `slot`, an app slot in `role`, with the slot it
/// is in the layout, `None` for the app the unverified tier located, unless
/// it is of `logged`, the epoch logged last, which it then replaces. Nothing
/// when no result of the slot's epoch now is kept. Returns whether it
/// logged.
#[inline(never)]
pub(super) fn app_slot<R: AppRole>(
    slot: &AppSlot<'_, R>,
    role: &str,
    layout: Option<Slot>,
    logged: &mut Option<Epoch>,
) -> bool {
    let Some(model) = slot.model() else {
        return false;
    };
    if *logged == Some(model.epoch()) {
        return false;
    }
    *logged = Some(model.epoch());
    let label = layout.map_or("no slot", |slot| slot.partition().label);
    let offset = slot.span().offset;
    match model.image() {
        Ok(image) => info!(
            "App check of {=str} ({=str} at {=u32:#x}): valid, {=u32} bytes, digest {}, file SHA-256 {}",
            role,
            label,
            offset,
            image.size(),
            Hex(image.digest()),
            Hex(image.file_sha256()),
        ),
        Err(failure) => warn!(
            "App check of {=str} ({=str} at {=u32:#x}): doesn't verify: {}",
            role,
            label,
            offset,
            Debug2Format(&failure),
        ),
    }
    true
}

/// Logs the OTA mechanism's readiness and the boot state it reads.
#[inline(never)]
pub(super) fn ota(ota: &Ota<'_>) {
    match ota.project() {
        Ok(project) => info!("OTA ready: updates of project {}", Fmt(*project)),
        Err(reason) => warn!("OTA unavailable: {}", Debug2Format(&reason)),
    }
    match ota.boot_state() {
        Ok(state) => boot_state(&state),
        Err(err) => warn!("OTA boot state not read: {}", Debug2Format(&err)),
    }
}

/// A digest as hex digits.
pub(super) struct Hex<'a>(pub(super) &'a [u8; 32]);

impl Format for Hex<'_> {
    fn format(&self, f: Formatter<'_>) {
        defmt::write!(f, "{=[u8]:02x}", self.0[..]);
    }
}

/// Formats the storage crates' types, which don't implement
/// [`defmt::Format`] as the crates also build on the host.
pub(super) struct Fmt<T>(pub(super) T);

impl Format for Fmt<Tier> {
    fn format(&self, f: Formatter<'_>) {
        let name = match self.0 {
            Tier::Unverified => "unverified",
            Tier::Layout => "layout",
            Tier::Boot => "boot",
        };
        defmt::write!(f, "{=str}", name);
    }
}

impl Format for Fmt<Step> {
    fn format(&self, f: Formatter<'_>) {
        let what = match self.0 {
            Step::Bootloader => "the bootloader isn't a known one that supports rollback",
            Step::TableOffset => "no single partition table is located",
            Step::Table => "the located partition table isn't valid",
            Step::Layout => "the located partition table isn't the layout at its compiled offset",
            Step::FlashSize => "the flash is smaller than the layout",
            Step::BootedSlot => "the app didn't boot from ota_0 or ota_1",
        };
        defmt::write!(f, "{=str}", what);
    }
}

impl Format for Fmt<Coverage> {
    fn format(&self, f: Formatter<'_>) {
        let what = match self.0 {
            Coverage::Layout => "its equality with the layout",
            Coverage::Md5 => "its MD5 entry",
            Coverage::Uncovered => "nothing",
        };
        defmt::write!(f, "{=str}", what);
    }
}

impl Format for Fmt<Slot> {
    fn format(&self, f: Formatter<'_>) {
        defmt::write!(f, "{=str}", self.0.partition().label);
    }
}

impl Format for Fmt<State> {
    fn format(&self, f: Formatter<'_>) {
        match self.0 {
            State::New => defmt::write!(f, "New"),
            State::PendingVerify => defmt::write!(f, "PendingVerify"),
            State::Valid => defmt::write!(f, "Valid"),
            State::Invalid => defmt::write!(f, "Invalid"),
            State::Aborted => defmt::write!(f, "Aborted"),
            State::Undefined => defmt::write!(f, "Undefined"),
            State::Unknown(raw) => defmt::write!(f, "Unknown({=u32:#x})", raw),
        }
    }
}

impl Format for Fmt<Validity> {
    fn format(&self, f: Formatter<'_>) {
        let name = match self.0 {
            Validity::Valid => "valid",
            Validity::Blank => "blank",
            Validity::Corrupt => "corrupt (CRC mismatch)",
            Validity::Invalid => "not valid (Invalid)",
            Validity::Aborted => "not valid (Aborted)",
        };
        defmt::write!(f, "{=str}", name);
    }
}

impl Format for Fmt<Fields> {
    fn format(&self, f: Formatter<'_>) {
        const NAMES: [(Fields, &str); 6] = [
            (Fields::LABEL, "label"),
            (Fields::KIND, "type"),
            (Fields::SUBTYPE, "subtype"),
            (Fields::OFFSET, "offset"),
            (Fields::SIZE, "size"),
            (Fields::FLAGS, "flags"),
        ];
        let mut first = true;
        for (field, name) in NAMES {
            if self.0.contains(field) {
                if !first {
                    defmt::write!(f, ", ");
                }
                defmt::write!(f, "{=str}", name);
                first = false;
            }
        }
    }
}

impl Format for Fmt<Option<Label>> {
    fn format(&self, f: Formatter<'_>) {
        if let Some(label) = self.0 {
            Fmt(label).format(f);
        } else {
            defmt::write!(f, "(label not covered)");
        }
    }
}

impl<const N: usize> Format for Fmt<Text<N>> {
    fn format(&self, f: Formatter<'_>) {
        if let Some(text) = self.0.to_str() {
            defmt::write!(f, "{=str}", text);
        } else {
            defmt::write!(f, "{=[u8]:02x}", self.0.as_bytes());
        }
    }
}
