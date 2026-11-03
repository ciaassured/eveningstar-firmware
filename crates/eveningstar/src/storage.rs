//! The internal flash, reached through `eveningstar-storage`.
//!
//! [`start`] runs once per boot. It reads the booted offset from the flash
//! cache's MMU, runs the storage crate's verification chain, which splits the
//! flash into the handles of the trust tier the board reaches
//! ([`eveningstar_storage::chain::split`]), and logs what it found. Storage
//! never stops the boot: the worst outcome is the unverified tier, in which
//! the firmware runs on its default configuration without saving it.
//!
//! An image the bootloader started as `PendingVerify` can only be confirmed in
//! the boot tier. In the layout tier, [`start`] requests a graceful reboot, so
//! the bootloader rolls the image back to the previous one. Nothing reboots in
//! the unverified tier, where the otadata isn't read.
//!
//! [`InternalFlash`] is the one `FlashStorage`: every read, erase and program
//! of the internal flash goes through it, one at a time. Its state table
//! lives in a `static`.

use core::cell::RefCell;

use defmt::{Debug2Format, Format, Formatter, error, info, warn};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use esp_hal::peripherals::{FLASH, SPI0};
use esp_storage::{FlashStorage, FlashStorageError};
use eveningstar_storage::chain::{
    BootloaderReport, Handles, LayoutComparison, Located, Report, Step, Storage, TableReport,
    TableSource, Tier, split,
};
use eveningstar_storage::nor::{NorDevice, Reader, State as StateTable};
use eveningstar_storage::otadata::{self, Sector, State, Validity};
use eveningstar_storage::roles::{FoundTable, PartitionTable};
use eveningstar_storage::roles::{Internal, Otadata, Role};
use eveningstar_storage::table::{self, Fields, Found, Match};
use eveningstar_storage::{
    DeviceError, Entry, SECTOR_SIZE, Slot, SlotPartitions as _, Span, Text, layout,
};
use static_cell::{ConstStaticCell, StaticCell};

use crate::services::reboot::{GracefulReboot, Reason};

/// The booted offset `split` is given when MMU entry 0 maps no page: past the
/// end of any flash, so no partition starts there and the tier is
/// unverified.
const UNKNOWN_BOOTED: u32 = u32::MAX;

/// The MMU's largest page size, which a page size code of 0 selects. Each
/// step of the code halves it.
const MMU_MAX_PAGE_SIZE: u32 = 0x1_0000;

/// Splits the internal flash into handles, logs the report, and returns the
/// handles.
///
/// When the running image is `PendingVerify` in the layout tier, it requests
/// a graceful reboot through `reboot`, so the bootloader rolls the image
/// back.
///
/// # Panics
///
/// If called more than once: the flash is split once per boot, so each
/// handle exists once.
#[expect(
    clippy::large_stack_frames,
    reason = "runs once at boot; the frame holds split's future and the handles on their way to the caller"
)]
pub(crate) async fn start(
    flash: FLASH<'static>,
    cache: SPI0<'static>,
    reboot: GracefulReboot,
) -> Storage<'static> {
    static INTERNAL_FLASH: StaticCell<InternalFlash> = StaticCell::new();
    static STATE: ConstStaticCell<StateTable> = ConstStaticCell::new(StateTable::new());

    let started = Instant::now();
    let flash = INTERNAL_FLASH.init(InternalFlash::new(flash, cache));
    let booted = flash.booted_offset().unwrap_or_else(|| {
        error!("MMU entry 0 maps no flash page: the booted offset is unknown");
        UNKNOWN_BOOTED
    });
    let storage = split(flash, STATE.take(), booted).await;
    let report = &storage.report;
    info!(
        "Storage split in {} ms: {} bytes of flash, app booted from {=u32:#x}",
        started.elapsed().as_millis(),
        report.capacity,
        booted,
    );
    log(&storage);
    if let Handles::Layout(handles) = &storage.handles {
        reboot_unconfirmed(&handles.otadata, report, reboot);
    }
    storage
}

/// Requests a reboot when the running image is `PendingVerify` in the layout
/// tier, where it can't be confirmed: the bootloader then rolls it back.
fn reboot_unconfirmed(otadata: &Reader<'_, Otadata>, report: &Report, reboot: GracefulReboot) {
    let Some(slot) = report.slot else {
        return;
    };
    let Some(otadata) = read_otadata(otadata) else {
        return;
    };
    if otadata.boot_state(slot).state == Some(State::PendingVerify) {
        warn!(
            "The running image is unconfirmed and can't be confirmed in the layout tier; \
             requesting a reboot, so the bootloader rolls it back"
        );
        reboot.graceful(Reason::CannotConfirm);
    }
}

/// The otadata, read through a snapshot; `None`, logged, when it can't be
/// taken.
fn read_otadata<R: Role<Device = Internal>>(reader: &Reader<'_, R>) -> Option<otadata::Otadata> {
    match reader.snapshot(Span::new(0, reader.len())) {
        Ok(snapshot) => Some(otadata::read(&snapshot)),
        Err(err) => {
            warn!("Otadata snapshot refused: {}", Debug2Format(&err));
            None
        }
    }
}

/// The internal flash, behind a critical-section mutex: each operation runs
/// with interrupts disabled, which a ROM flash call needs anyway while code
/// runs from the flash it changes.
struct InternalFlash {
    storage: Mutex<CriticalSectionRawMutex, RefCell<FlashStorage<'static>>>,
    /// The flash cache controller, read for MMU entry 0.
    cache: SPI0<'static>,
    /// The flash's size, from its JEDEC ID; 0 when the ID isn't one
    /// `esp-storage` knows.
    capacity: u32,
}

impl InternalFlash {
    #[expect(
        clippy::disallowed_methods,
        reason = "the storage glue holds the one FlashStorage"
    )]
    fn new(flash: FLASH<'static>, cache: SPI0<'static>) -> Self {
        let storage = FlashStorage::new(flash);
        let capacity = u32::try_from(storage.capacity()).unwrap_or(u32::MAX);
        Self {
            storage: Mutex::new(RefCell::new(storage)),
            cache,
            capacity,
        }
    }

    /// Runs `operation` on the flash, alone.
    fn with<T>(&self, operation: impl FnOnce(&mut FlashStorage<'static>) -> T) -> T {
        self.storage
            .lock(|storage| operation(&mut storage.borrow_mut()))
    }

    /// The flash offset of the page MMU entry 0 maps: the start of the image
    /// the bootloader started, as entry 0 maps the start of the app's flash
    /// address range. `None` when the entry isn't valid.
    ///
    /// The MMU's entries are read through an index register, which selects an
    /// entry, and a content register. `esp-storage` uses the same index
    /// register, so the read runs inside the flash's critical section, and
    /// the index is put back as it was.
    fn booted_offset(&self) -> Option<u32> {
        let regs = self.cache.register_block();
        let select = |index: u32| {
            regs.mmu_item_index().write(|w| {
                // SAFETY: selecting an entry changes no mapping, only which
                // entry the content register shows; nothing is written to the
                // content register. The other user of the index register,
                // `esp-storage`, selects the entry it needs before reading or
                // writing the content, and `select` runs only inside
                // the flash's critical section, so no interrupt runs between a
                // selection here and the read that follows it.
                unsafe { w.mmu_item_index().bits(index) }
            });
        };
        let (entry, page_size_code) = self.storage.lock(|_| {
            let previous = regs.mmu_item_index().read().mmu_item_index().bits();
            select(0);
            let entry = regs.mmu_item_content().read();
            select(previous);
            (
                entry,
                regs.mmu_power_ctrl().read().spi_mmu_page_size().bits(),
            )
        });
        let page_size = MMU_MAX_PAGE_SIZE >> page_size_code;
        entry
            .valid()
            .bit()
            .then(|| u32::from(entry.paddr().bits()) * page_size)
    }
}

impl NorDevice for InternalFlash {
    fn capacity(&self) -> u32 {
        self.capacity
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        let result = self.with(|storage| storage.read(address, buf));
        checked("read", address, result)
    }

    fn erase_sector(&self, address: u32) -> Result<(), DeviceError> {
        // A range of one sector is erased with a sector erase, even when it
        // starts on a 64 KiB block boundary: `erase` uses a block erase only
        // for a whole block.
        let result =
            self.with(|storage| storage.erase(address, address.saturating_add(SECTOR_SIZE)));
        checked("erase", address, result)
    }

    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError> {
        let result = self.with(|storage| storage.write_nor(address, bytes));
        checked("program", address, result)
    }
}

/// `result` as the storage crate's error, logging the failure.
fn checked(
    operation: &str,
    address: u32,
    result: Result<(), FlashStorageError>,
) -> Result<(), DeviceError> {
    result.map_err(|err| {
        warn!(
            "Internal flash {=str} at {=u32:#x} failed: {}",
            operation, address, err
        );
        DeviceError
    })
}

/// Logs what the chain found: each step, the tier and the handles of the
/// unverified tier, or the otadata in the layout and boot tiers.
fn log(storage: &Storage<'_>) {
    let report = &storage.report;
    log_bootloader(&report.bootloader);
    log_table(&report.table, table_reader(storage));
    log_flash_and_slot(report);
    log_tier(report);
    match &storage.handles {
        Handles::Unverified(located) => log_located(located),
        Handles::Layout(handles) | Handles::Boot(handles, _) => {
            if let (Some(slot), Some(otadata)) = (report.slot, read_otadata(&handles.otadata)) {
                log_otadata(&otadata, slot);
            }
        }
    }
}

/// The reader that covers the located table, to log its partitions: the
/// compiled table's sector when the table lies there, else the found table's.
fn table_reader<'s>(storage: &'s Storage<'_>) -> Option<TableBytes<'s>> {
    let offset = storage.report.table.offset?;
    if offset == layout::TABLE_OFFSET {
        return storage.partition_table.as_ref().map(TableBytes::Compiled);
    }
    match &storage.handles {
        Handles::Unverified(Located {
            table: Some(table), ..
        }) if table.span().offset == offset => Some(TableBytes::Found(table)),
        _ => None,
    }
}

/// A reader of the located table.
enum TableBytes<'s> {
    Compiled(&'s Reader<'s, PartitionTable>),
    Found(&'s Reader<'s, FoundTable>),
}

/// Logs step 1: the bootloader's header, image and known entry.
fn log_bootloader(bootloader: &BootloaderReport) {
    if let (Some(code), Some(size)) = (bootloader.flash_size_code, bootloader.chip_size()) {
        info!(
            "Bootloader header: flash size code {=u8}, {=u32} bytes, the chip size the bootloader checks the table against",
            code, size
        );
    } else {
        warn!("Bootloader header doesn't read");
    }
    if let Err(err) = bootloader.header {
        warn!("Bootloader header isn't valid: {}", Debug2Format(&err));
    }
    match (bootloader.image, bootloader.known) {
        (Ok(id), Some(known)) => info!(
            "Bootloader: {=str}, {=u32} bytes, reads the table at {=u32:#x}, rollback {=bool}",
            known.name, id.size, known.table_offset, known.rollback
        ),
        (Ok(id), None) => warn!(
            "Bootloader: unknown, {=u32} bytes, digest {}",
            id.size,
            Hex(&id.digest)
        ),
        (Err(err), _) => warn!("Bootloader doesn't verify: {}", Debug2Format(&err)),
    }
    if bootloader.fits == Some(false) {
        warn!("Bootloader image runs past the table's offset: it is reported, not published");
    }
}

/// Logs steps 2 to 4: how the table was located, its validity, and its
/// partitions compared with the layout.
fn log_table(report: &TableReport, reader: Option<TableBytes<'_>>) {
    match (report.source, report.offset) {
        (Some(TableSource::KnownBootloader), Some(offset)) => {
            info!(
                "Partition table at {=u32:#x}, where the known bootloader reads it",
                offset
            );
        }
        (Some(TableSource::Searched), Some(offset)) => info!(
            "Partition table at {=u32:#x}, the one table the search found with an app at the booted offset",
            offset
        ),
        _ => match report.search {
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
    match report.verdict {
        Some(Ok(verified)) => {
            if let Some(md5) = verified.md5 {
                info!(
                    "Partition table valid: end entry at {=usize}, MD5 entry at {=usize} matches",
                    verified.end, md5.index
                );
            } else {
                info!(
                    "Partition table valid: end entry at {=usize}, no MD5 entry",
                    verified.end
                );
            }
        }
        Some(Err(err)) => warn!(
            "Partition table isn't valid as the bootloader judges it: {}",
            Debug2Format(&err)
        ),
        None => {}
    }
    let Some(partitions) = report.partitions else {
        return;
    };
    info!(
        "The bootloader loads {=usize} partitions from it",
        partitions.count
    );
    if partitions.md5_read_as_partition {
        warn!(
            "Entries follow the MD5 entry: the bootloader reads the MD5 entry as a partition and leaves out the last entry"
        );
    }
    if let (Some(reader), Some(Ok(verified))) = (reader, report.verdict)
        && let Some(offset) = report.offset
    {
        match reader {
            TableBytes::Compiled(reader) => log_partitions(reader, offset, &verified),
            TableBytes::Found(reader) => log_partitions(reader, offset, &verified),
        }
    }
    log_comparison(&partitions.comparison);
}

/// Logs the partitions the bootloader loads from the table `reader` holds,
/// at `offset` of the flash.
#[inline(never)]
fn log_partitions<R: Role<Device = Internal>>(
    reader: &Reader<'_, R>,
    offset: u32,
    verified: &table::Verified,
) {
    let Ok(snapshot) = reader.snapshot(Span::new(0, reader.len())) else {
        return;
    };
    // The located table starts at the start of its reader's span.
    let at = offset.saturating_sub(reader.span().offset);
    for partition in table::partitions(&snapshot, at, verified) {
        match partition {
            Ok((index, entry)) => info!(
                "  {=usize}: {}, type {=u8:#x}, subtype {=u8:#x}, {=u32:#x}..{=u32:#x}, flags {=u32:#x}",
                index,
                Fmt(entry.label),
                entry.kind,
                entry.subtype,
                entry.offset,
                entry.offset.saturating_add(entry.size),
                entry.flags,
            ),
            Err(err) => {
                warn!("  read failed: {}", Debug2Format(&err));
                return;
            }
        }
    }
}

/// Logs how the partitions compare with the layout, by label.
fn log_comparison(comparison: &LayoutComparison) {
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

/// Logs step 6: the flash's size and the slot booted.
fn log_flash_and_slot(report: &Report) {
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

/// Logs the tier and the first step that failed.
fn log_tier(report: &Report) {
    match report.failed {
        None => info!("Storage: boot tier"),
        Some(step) => {
            warn!(
                "Storage: {} tier; first step failed: {}",
                Fmt(report.tier),
                Fmt(step)
            );
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

/// Logs the readers of what the unverified tier located.
fn log_located(located: &Located<'_>) {
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
    span(located.table.as_ref().map(Reader::span), "table");
    span(located.app.as_ref().map(Reader::span), "app");
    span(located.otadata.as_ref().map(Reader::span), "otadata");
}

/// Logs both otadata entries and the boot state of the app running from
/// `slot`, and warns when the bootloader started another slot than the one
/// selected.
fn log_otadata(otadata: &otadata::Otadata, slot: Slot) {
    info!("Otadata:");
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
    let state = otadata.boot_state(slot);
    info!(
        "Boot state: running {}, selected {}, state {}, rollback target {=bool}",
        Fmt(state.running),
        state.selected.map(Fmt),
        state.state.map(Fmt),
        state.rollback_target,
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

/// A digest as hex digits.
struct Hex<'a>(&'a [u8; 32]);

impl Format for Hex<'_> {
    fn format(&self, f: Formatter<'_>) {
        defmt::write!(f, "{=[u8]:02x}", self.0[..]);
    }
}

/// Formats the storage crate's types, which don't implement
/// [`defmt::Format`] as the crate also builds on the host.
struct Fmt<T>(T);

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

impl<const N: usize> Format for Fmt<Text<N>> {
    fn format(&self, f: Formatter<'_>) {
        if let Some(text) = self.0.to_str() {
            defmt::write!(f, "{=str}", text);
        } else {
            defmt::write!(f, "{=[u8]:02x}", self.0.as_bytes());
        }
    }
}
