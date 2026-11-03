//! The internal flash, reached through `eveningstar-storage`, and the
//! content modules of its boot regions, from `eveningstar-boot`.
//!
//! [`start`] runs once per boot. It reads the booted offset from the flash
//! cache's MMU, runs the storage crate's verification chain, which splits the
//! flash into the handles of the trust tier the board reaches
//! ([`eveningstar_storage::chain::split`]), and gives the readers to the
//! content modules, each in a `static`: the bootloader, the partition table,
//! the otadata and the app slots. It loads the bootloader's model, parses the
//! table and the otadata, logs what they hold, and spawns the app check task
//! ([`check`]), which verifies the slots in the background. Storage never
//! stops the boot: the worst outcome is the unverified tier, in which the
//! firmware runs on its default configuration without saving it.
//!
//! In the boot tier, [`start`] also returns the parts of the OTA mechanism
//! ([`OtaParts`]): the boot writers and the modules [`Ota`] reads. Building
//! it ([`OtaParts::start`]) waits for the running slot's check, so it runs
//! after the check task is spawned, and places it in a `static`.
//!
//! An image the bootloader started as `PendingVerify` can only be confirmed in
//! the boot tier. In the layout tier, [`start`] logs it and requests no
//! reboot: that tier's bootloader isn't known to roll back, and one that
//! doesn't would start the same image after every reboot, so a reboot there
//! could repeat for good on a board with no reset button. A rollback
//! bootloader rolls the image back at the next boot, whatever causes it.
//! Nothing is requested in the unverified tier.
//!
//! [`InternalFlash`] is the one `FlashStorage`: every read, erase and program
//! of the internal flash goes through it, one at a time. Its state table
//! lives in a `static`.

use core::cell::RefCell;

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use esp_hal::peripherals::{FLASH, SPI0};
use esp_storage::{FlashStorage, FlashStorageError};
use eveningstar_boot::{App, AppSlot, Bootloader, Otadata, PartitionTable};
use eveningstar_esp_formats::pace::Yield;
use eveningstar_ota::Ota;
use eveningstar_storage::chain::{
    BootWriters, Handles, LayoutHandles, Located, Report, Storage, Tier, split,
};
use eveningstar_storage::nor::{NorDevice, Reader, State as StateTable};
use eveningstar_storage::otadata::State;
use eveningstar_storage::roles::{self, AlternateApp, CurrentApp, FoundApp, FoundTable};
use eveningstar_storage::{DeviceError, SECTOR_SIZE, Slot, Span};
use static_cell::{ConstStaticCell, StaticCell};

use crate::sha::Sha;
use crate::watchdog::WatchdogHandle;
use crate::wd;

mod check;
mod log;

pub(crate) use check::WATCHDOG_TIMEOUT;

/// The booted offset `split` is given when MMU entry 0 maps no page: past the
/// end of any flash, so no partition starts there and the tier is
/// unverified.
const UNKNOWN_BOOTED: u32 = u32::MAX;

/// The MMU's largest page size, which a page size code of 0 selects. Each
/// step of the code halves it.
const MMU_MAX_PAGE_SIZE: u32 = 0x1_0000;

/// Splits the internal flash, places the content modules in `static`s,
/// loads and logs what they hold, and spawns the app check task on
/// `spawner`, hashing with `sha` and supervised by `wd`. Returns the parts of
/// the OTA mechanism in the boot tier.
///
/// When the running image is `PendingVerify` in the layout tier, where it
/// can't be confirmed, it logs a warning.
///
/// # Panics
///
/// If called more than once: the flash is split once per boot, so each
/// handle exists once.
#[expect(
    clippy::large_stack_frames,
    reason = "runs once at boot; the frame holds split's future and the handles on their way to the modules"
)]
pub(crate) async fn start(
    spawner: Spawner,
    flash: FLASH<'static>,
    cache: SPI0<'static>,
    sha: Sha,
    wd: WatchdogHandle<wd::AppCheck>,
) -> Option<OtaParts> {
    static INTERNAL_FLASH: StaticCell<InternalFlash> = StaticCell::new();
    static STATE: ConstStaticCell<StateTable> = ConstStaticCell::new(StateTable::new());

    let started = Instant::now();
    let flash = INTERNAL_FLASH.init(InternalFlash::new(flash, cache));
    let booted = flash.booted_offset().unwrap_or_else(|| {
        error!("MMU entry 0 maps no flash page: the booted offset is unknown");
        UNKNOWN_BOOTED
    });
    let storage = split(flash, STATE.take(), booted).await;
    info!(
        "Storage split in {} ms: {} bytes of flash, app booted from {=u32:#x}",
        started.elapsed().as_millis(),
        storage.report.capacity,
        booted,
    );

    let started = Instant::now();
    let Storage {
        report,
        bootloader,
        partition_table,
        handles,
    } = storage;
    // The configuration and diag writers have no owner yet, so they are
    // dropped here; they can't be made again, as the flash is split once per
    // boot.
    let placed = match handles {
        Handles::Unverified(located) => place_located(located),
        Handles::Layout(handles) => place_layout(handles, None),
        Handles::Boot(handles, writers) => place_layout(handles, Some(writers)),
    };
    let modules = place(bootloader, partition_table, placed, &report);
    let loaded = modules
        .bootloader
        .load(sha.sha256(), &mut Yield, &mut || {})
        .await;
    log::modules(&modules, &report, loaded);
    info!(
        "Content modules loaded in {} ms",
        started.elapsed().as_millis()
    );
    if report.tier == Tier::Layout {
        warn_unconfirmed(modules.otadata, report.slot);
    }

    spawner.spawn(defmt::unwrap!(check::task(
        modules.app,
        report.slot,
        sha,
        wd
    )));
    modules.ota
}

/// The content modules, each in its `static`.
struct Modules {
    bootloader: &'static Bootloader<'static>,
    table: &'static PartitionTable<'static>,
    /// The otadata's module: the compiled layout's, or the located table's in
    /// the unverified tier, when the chain located one.
    otadata: Option<&'static Otadata<'static>>,
    app: App<'static, 'static>,
    /// The spans the unverified tier located, for the log.
    located: Option<LocatedSpans>,
    /// The parts of the OTA mechanism, in the boot tier.
    ota: Option<OtaParts>,
}

/// The spans of the readers the unverified tier located.
struct LocatedSpans {
    table: Option<Span>,
    app: Option<Span>,
    otadata: Option<Span>,
}

/// The module `make` makes of `reader`, placed in `cell`. Never inlined, so
/// each module passes through its frame alone.
///
/// # Panics
///
/// If `cell` holds a module already.
#[inline(never)]
fn init<R, T>(cell: &'static StaticCell<T>, reader: R, make: fn(R) -> T) -> &'static T {
    cell.init(make(reader))
}

/// The modules of the bootloader's region and of the table, over
/// `bootloader` and `compiled` (the compiled table's sector) and the table
/// the search found, with the modules `placed` of the tier's handles, as
/// `report` describes them.
#[inline(never)]
fn place(
    bootloader: Reader<'static, roles::Bootloader>,
    compiled: Option<Reader<'static, roles::PartitionTable>>,
    placed: Placed,
    report: &Report,
) -> Modules {
    static BOOTLOADER: StaticCell<Bootloader<'static>> = StaticCell::new();
    static TABLE: StaticCell<PartitionTable<'static>> = StaticCell::new();

    let Placed {
        found_table,
        otadata,
        app,
        located,
        ota,
    } = placed;
    Modules {
        bootloader: init(&BOOTLOADER, bootloader, Bootloader::new),
        table: TABLE.init(PartitionTable::new(compiled, found_table, report)),
        otadata,
        app,
        located,
        ota,
    }
}

/// The modules of a tier's handles, and the reader of the table the search
/// found, which goes to the table's module.
struct Placed {
    found_table: Option<Reader<'static, FoundTable>>,
    otadata: Option<&'static Otadata<'static>>,
    app: App<'static, 'static>,
    located: Option<LocatedSpans>,
    ota: Option<OtaParts>,
}

/// The modules of what the unverified tier located, and the spans located.
#[inline(never)]
fn place_located(located: Located<'static>) -> Placed {
    static OTADATA: StaticCell<Otadata<'static>> = StaticCell::new();
    static FOUND: StaticCell<AppSlot<'static, FoundApp>> = StaticCell::new();

    let spans = LocatedSpans {
        table: located.table.as_ref().map(Reader::span),
        app: located.app.as_ref().map(Reader::span),
        otadata: located.otadata.as_ref().map(Reader::span),
    };
    Placed {
        found_table: located.table,
        otadata: located
            .otadata
            .map(|reader| init(&OTADATA, reader, Otadata::found)),
        app: App::found(located.app.map(|reader| init(&FOUND, reader, AppSlot::new))),
        located: Some(spans),
        ota: None,
    }
}

/// The modules of the layout and boot tiers' readers, and the parts of the
/// OTA mechanism when `writers`, the boot writers, are given. The
/// configuration and diag writers are dropped.
#[inline(never)]
fn place_layout(handles: LayoutHandles<'static>, writers: Option<BootWriters<'static>>) -> Placed {
    static OTADATA: StaticCell<Otadata<'static>> = StaticCell::new();
    static CURRENT: StaticCell<AppSlot<'static, CurrentApp>> = StaticCell::new();
    static ALTERNATE: StaticCell<AppSlot<'static, AlternateApp>> = StaticCell::new();

    let otadata = init(&OTADATA, handles.otadata, Otadata::compiled);
    let current = init(&CURRENT, handles.current_app, AppSlot::new);
    let alternate = init(&ALTERNATE, handles.alternate_app, AppSlot::new);
    Placed {
        found_table: None,
        otadata: Some(otadata),
        app: App::layout(current, alternate),
        located: None,
        ota: writers.map(|writers| OtaParts {
            writers,
            current,
            alternate,
            otadata,
        }),
    }
}

/// Warns when the running image is `PendingVerify` in the layout tier, where
/// it can't be confirmed, `otadata` being the otadata's module and `slot`
/// the slot that booted.
#[inline(never)]
fn warn_unconfirmed(otadata: Option<&Otadata<'_>>, slot: Option<Slot>) {
    let (Some(slot), Some(otadata)) = (slot, otadata) else {
        return;
    };
    let Ok(model) = otadata.parse() else {
        return;
    };
    if model.boot_state(slot).state == Some(State::PendingVerify) {
        warn!(
            "The running image is unconfirmed and can't be confirmed in the layout tier; \
             a bootloader that supports rollback starts the previous image at the next boot"
        );
    }
}

/// The parts of the OTA mechanism, in the boot tier: the boot writers, and
/// the modules [`Ota`] reads.
pub(crate) struct OtaParts {
    writers: BootWriters<'static>,
    current: &'static AppSlot<'static, CurrentApp>,
    alternate: &'static AppSlot<'static, AlternateApp>,
    otadata: &'static Otadata<'static>,
}

impl OtaParts {
    /// Builds the OTA mechanism in its `static`, and logs its readiness and
    /// the boot state. Completes once the running slot's check has kept its
    /// result, so the check task has to run.
    ///
    /// # Panics
    ///
    /// If called more than once, which the one [`OtaParts`] of a boot
    /// prevents.
    pub(crate) async fn start(self) -> &'static mut Ota<'static> {
        static OTA: StaticCell<Ota<'static>> = StaticCell::new();

        let Self {
            writers,
            current,
            alternate,
            otadata,
        } = self;
        let ota = OTA.init(Ota::new(writers, current, alternate, otadata).await);
        log::ota(ota);
        ota
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
