//! The internal flash, reached through `eveningstar-storage`.
//!
//! [`start`] runs once per boot. It reads the booted offset from the flash
//! cache's MMU, splits the flash into the handles of the trust tier the board
//! reaches ([`eveningstar_storage::split`]), and logs what it found. Storage
//! never stops the boot: the worst outcome is the untrusted tier, in which the
//! firmware runs without persistent storage.
//!
//! An image the bootloader started as `PendingVerify` can only be confirmed in
//! the full tier. Below it, [`start`] requests a graceful reboot, so the
//! bootloader rolls the image back to the previous one.
//!
//! [`InternalFlash`] is the one `FlashStorage`: every read, erase and program
//! of the internal flash goes through it, one at a time.

use core::cell::RefCell;

use defmt::{Debug2Format, Format, Formatter, error, info, warn};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use esp_hal::peripherals::{FLASH, SPI0};
use esp_storage::{FlashStorage, FlashStorageError};
use eveningstar_storage::otadata::{self, Sector, State, Validity};
use eveningstar_storage::partitions::{Partition, PartitionTable};
use eveningstar_storage::table::{self, Fields, Match};
use eveningstar_storage::{
    DeviceError, Entry, Flash, Limit, Report, SECTOR_SIZE, Slot, Storage, Text, Tier, layout, split,
};
use static_cell::StaticCell;

use crate::services::reboot::{GracefulReboot, Reason};

/// The booted offset `split` is given when MMU entry 0 maps no page: past the
/// end of any flash, so no partition starts there and the tier is untrusted.
const UNKNOWN_BOOTED: u32 = u32::MAX;

/// The MMU's largest page size, which a page size code of 0 selects. Each
/// step of the code halves it.
const MMU_MAX_PAGE_SIZE: u32 = 0x1_0000;

/// Splits the internal flash into handles, logs the report, and returns the
/// handles.
///
/// When the running image is `PendingVerify` and the tier isn't full
/// ([`Report::cannot_confirm`]), it requests a graceful reboot through
/// `reboot`, so the bootloader rolls the image back.
///
/// # Panics
///
/// If called more than once: the flash is split once per boot, so each
/// unique handle exists once.
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

    let started = Instant::now();
    let flash = INTERNAL_FLASH.init(InternalFlash::new(flash, cache));
    let booted = flash.booted_offset().unwrap_or_else(|| {
        error!("MMU entry 0 maps no flash page: the booted offset is unknown");
        UNKNOWN_BOOTED
    });
    let capacity = flash.capacity;
    let storage = split(flash, booted).await;
    info!(
        "Storage split in {} ms: {} bytes of flash, app booted from {=u32:#x}",
        started.elapsed().as_millis(),
        capacity,
        booted,
    );
    log(&storage, capacity);
    confirm_or_reboot(&storage.report, reboot);
    storage
}

/// Requests a reboot when the running image can't be confirmed (T7).
fn confirm_or_reboot(report: &Report, reboot: GracefulReboot) {
    if report.cannot_confirm() {
        warn!(
            "The running image is unconfirmed and can't be confirmed below the full tier; \
             requesting a reboot, so the bootloader rolls it back"
        );
        reboot.graceful(Reason::CannotConfirm);
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

impl Flash for InternalFlash {
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

/// Logs what `split` found: the tier, the table, the bootloader, the running
/// app and the boot state.
fn log(storage: &Storage<'_>, capacity: u32) {
    let report = &storage.report;
    match report.limit {
        None => info!("Storage: full tier"),
        Some(limit) => {
            warn!(
                "Storage: {} tier, as {}",
                Fmt(report.tier()),
                LimitReason { limit, capacity }
            );
            match limit.tier() {
                Tier::Untrusted => warn!("Running without persistent storage"),
                Tier::Data | Tier::Full => warn!("OTA updates are unavailable"),
            }
        }
    }
    if report.table.matches() {
        info!(
            "Partition table at {=u32:#x} matches the layout",
            layout::TABLE_OFFSET
        );
    } else {
        log_comparison(report);
        log_found_table(report.found_table, storage.boot.table);
    }
    log_bootloader(report);
    log_app(report);
    if let Some(data) = &storage.data {
        log_otadata(report, &otadata::read(&data.otadata));
    }
}

/// Logs how the table at the compiled offset differs from the layout.
fn log_comparison(report: &Report) {
    /// The layout's partitions, read in place rather than copied.
    static PARTITIONS: [Entry; layout::PARTITIONS.len()] = layout::PARTITIONS;

    let comparison = &report.table;
    warn!(
        "Partition table at {=u32:#x} doesn't match the layout:",
        layout::TABLE_OFFSET
    );
    for (expected, comparison) in PARTITIONS.iter().zip(&comparison.partitions) {
        match *comparison {
            Match::Matches => info!("  {=str}: matches", expected.label),
            Match::Differs(fields) => {
                warn!("  {=str}: differs in {}", expected.label, Fmt(fields));
            }
            Match::Missing => warn!("  {=str}: missing", expected.label),
        }
    }
    if comparison.extra > 0 {
        warn!("  {} entries past the layout's", comparison.extra);
    }
    if let Err(err) = comparison.end {
        warn!("  not well formed: {}", Debug2Format(&err));
    }
}

/// Logs the entries of the table found on the board, at `offset`.
fn log_found_table(offset: Option<u32>, table: Option<Partition<'_, PartitionTable>>) {
    let (Some(offset), Some(table)) = (offset, table) else {
        warn!("No partition table found with an app at the booted offset");
        return;
    };
    info!("Partition table found at {=u32:#x}:", offset);
    for entry in table::entries(&table, 0).map_while(Result::ok) {
        info!(
            "  {}: type {=u8:#x}, subtype {=u8:#x}, {=u32:#x}..{=u32:#x}, flags {=u32:#x}",
            Fmt(entry.label),
            entry.kind,
            entry.subtype,
            entry.offset,
            entry.offset.saturating_add(entry.size),
            entry.flags,
        );
    }
}

fn log_bootloader(report: &Report) {
    match report.bootloader {
        Ok(id) => {
            if let Some(name) = id.name {
                info!("Bootloader: {=str}", name);
            } else {
                warn!("Bootloader: unknown, digest {=[u8]:02x}", id.digest);
            }
        }
        Err(err) => warn!("Bootloader doesn't verify: {}", Debug2Format(&err)),
    }
}

fn log_app(report: &Report) {
    if let Some(slot) = report.running {
        info!("Running from {}", Fmt(slot));
    } else {
        warn!(
            "Running from {=u32:#x}, not as one of the layout's slots",
            report.booted
        );
    }
    match report.app {
        Ok(app) => info!("App: {} {}", Fmt(app.project_name), Fmt(app.version)),
        Err(err) => warn!("App descriptor doesn't read: {}", Debug2Format(&err)),
    }
}

/// Logs the boot state and both otadata entries, and warns when the
/// bootloader started another slot than the one selected.
fn log_otadata(report: &Report, otadata: &otadata::Otadata) {
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
    let Some(state) = report.boot_state else {
        return;
    };
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

/// Formats the storage crate's types, which don't implement
/// [`defmt::Format`] as the crate also builds on the host.
struct Fmt<T>(T);

impl Format for Fmt<Tier> {
    fn format(&self, f: Formatter<'_>) {
        let name = match self.0 {
            Tier::Untrusted => "untrusted",
            Tier::Data => "data",
            Tier::Full => "full",
        };
        defmt::write!(f, "{=str}", name);
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

/// Why the full tier wasn't reached, in words.
struct LimitReason {
    limit: Limit,
    capacity: u32,
}

impl Format for LimitReason {
    fn format(&self, f: Formatter<'_>) {
        match self.limit {
            Limit::Table => defmt::write!(
                f,
                "the partition table at {=u32:#x} doesn't match the layout",
                layout::TABLE_OFFSET
            ),
            Limit::FlashSize => defmt::write!(
                f,
                "the flash holds {} bytes, less than the layout's {}",
                self.capacity,
                layout::FLASH_SIZE
            ),
            Limit::NotASlot => defmt::write!(f, "the app didn't boot from ota_0 or ota_1"),
            Limit::BootloaderInvalid => defmt::write!(f, "the bootloader doesn't verify"),
            Limit::BootloaderUnknown => {
                defmt::write!(f, "the bootloader isn't known to support rollback");
            }
            Limit::AppDescriptor => defmt::write!(f, "the running app's descriptor doesn't read"),
        }
    }
}
