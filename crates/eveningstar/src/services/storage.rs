//! The storage service: what `split` found on the internal flash, its public
//! handles, and the results of the boot-time checks of the app slots.
//!
//! [`start`] takes what [`storage::start`](crate::storage::start) hands out,
//! spawns the check task and returns the one [`StorageView`], a `Copy` handle
//! to all of it. The service holds public handles only, those that implement
//! [`Read`]; the sensitive ones never reach it.
//!
//! The check task verifies each app slot the firmware holds a handle for, one
//! after the other: the running app ([`AppRole::Current`]), and in the data
//! and full tiers the other slot ([`AppRole::Alternate`]). A check is
//! [`verify_app`]: the checksum and the digest the image stores, from one pass
//! that also gives the SHA-256 of the whole image, yielding every 1 KiB
//! hashed. Its result is valid with both digests, empty (no image header) or
//! invalid with the error, and is logged.
//!
//! A result is stored with the write generation of the slot noted before the
//! check, and only when the generation is unchanged after it. It is current
//! while that generation is the live one: a write to the slot makes it stale.
//! After a write, the task checks the slots whose results are stale again
//! once the flash has gone [`QUIET`].
//!
//! The task is supervised by its own watchdog handle, fed on every read of a
//! check, never on a timer, so a check that hangs while the rest of the
//! system runs is caught. The handle is paused while the task waits for writes.

use core::cell::RefCell;

use defmt::{Debug2Format, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer};
use eveningstar_storage::image::{self, App, HEADER_LEN, ImageError, MAGIC};
use eveningstar_storage::partitions::{Read, SensitiveRead};
use eveningstar_storage::{ReadError, Report, Slot, Source};
use static_cell::StaticCell;

use crate::storage::{Generations, Hex, Public, Started, Writes};
use crate::watchdog::WatchdogHandle;
use crate::wd;

/// Task watchdog timeout of the check task. A check feeds on every read, many
/// times a millisecond, so this only has to outlast the supervisor's polls.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the flash has to go without a write before the task checks the
/// slots the writes made stale, so that an update written over a minute
/// costs one check, not hundreds.
pub(crate) const QUIET: Duration = Duration::from_secs(3);

/// The byte an erased flash reads as.
const ERASED: u8 = 0xFF;

/// An app slot, by its role this boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub(crate) enum AppRole {
    /// The running app: its slot, or its image when it didn't boot from one.
    Current,
    /// The slot that isn't running, in the data and full tiers.
    Alternate,
}

impl AppRole {
    /// Every role, in the order they are checked.
    const ALL: [Self; 2] = [Self::Current, Self::Alternate];

    /// The role's region name.
    const fn name(self) -> &'static str {
        match self {
            Self::Current => "app_current",
            Self::Alternate => "app_alternate",
        }
    }
}

/// What a check found in an app slot.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Image {
    /// The image verifies: its size and both digests.
    Valid(App),
    /// The slot holds no image header: its first bytes are erased.
    Empty,
    /// The image doesn't verify.
    Invalid(ImageError),
}

/// The result of a check of an app slot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Check {
    /// What the check found.
    pub(crate) image: Image,
    /// When the check finished.
    pub(crate) finished: Instant,
    /// The slot's write generation the check read.
    generation: u32,
}

/// The state of the checks of the app slots the firmware holds a handle for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Progress {
    /// A slot has no current result: its check hasn't run, or a write made
    /// its result stale.
    Running,
    /// Every slot has a current result.
    Done {
        /// When the last of them finished.
        finished: Instant,
    },
}

/// The service's fixed state, made once by [`start`].
#[derive(Debug)]
struct Service {
    report: Report,
    public: Public,
    generations: Generations,
    boot: u64,
}

/// The latest result per [`AppRole`], indexed by the role's value.
static CHECKS: Mutex<CriticalSectionRawMutex, RefCell<[Option<Check>; AppRole::ALL.len()]>> =
    Mutex::new(RefCell::new([None; AppRole::ALL.len()]));

/// Spawns the check task and returns the handle to the service.
///
/// `boot` is a random value drawn once per boot. Takes the check task's
/// watchdog handle, of which there is one, so there is one call.
///
/// # Panics
///
/// If called more than once.
#[expect(
    clippy::large_stack_frames,
    reason = "runs once at boot; the frame holds the report and the handles on their way to the service's static"
)]
pub(crate) fn start(
    spawner: Spawner,
    started: Started,
    boot: u64,
    wd: WatchdogHandle<wd::StorageCheck>,
) -> StorageView {
    static SERVICE: StaticCell<Service> = StaticCell::new();

    let Started {
        report,
        public,
        generations,
        writes,
    } = started;
    let service: &'static Service = SERVICE.init(Service {
        report,
        public,
        generations,
        boot,
    });
    spawner.spawn(defmt::unwrap!(check_task(service, writes, wd)));
    StorageView(service)
}

/// Permission to read the storage service: the report, the public handles,
/// the write generations and the checks' results. Only [`start`] creates
/// one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StorageView(&'static Service);

#[expect(dead_code, reason = "nothing reads the storage view yet")]
impl StorageView {
    /// What `split` found, and the tier reached.
    pub(crate) fn report(self) -> &'static Report {
        &self.0.report
    }

    /// The public handles.
    pub(crate) fn public(self) -> Public {
        self.0.public
    }

    /// The write generations.
    pub(crate) fn generations(self) -> Generations {
        self.0.generations
    }

    /// The random value drawn once this boot.
    pub(crate) fn boot(self) -> u64 {
        self.0.boot
    }

    /// The current result of the check of the app slot in `role`: `None`
    /// when the firmware holds no handle for it, its check hasn't run yet,
    /// or a write has made the result stale.
    pub(crate) fn image(self, role: AppRole) -> Option<Check> {
        self.0.current(role)
    }

    /// Whether every slot the firmware holds a handle for has a current
    /// result.
    pub(crate) fn progress(self) -> Progress {
        self.0.progress()
    }
}

impl Service {
    /// The offset and length of the handle for `role`, when the firmware
    /// holds one.
    fn span(&self, role: AppRole) -> Option<(u32, u32)> {
        match role {
            AppRole::Current => self
                .public
                .boot
                .running_app
                .map(|app| (app.offset(), app.len())),
            AppRole::Alternate => self.public.other_app.map(|app| (app.offset(), app.len())),
        }
    }

    /// The live write generation of the slot in `role`.
    fn generation(&self, role: AppRole) -> Option<u32> {
        let (offset, len) = self.span(role)?;
        Some(self.generations.of(offset, len))
    }

    /// The stored result for `role`, while its generation is the live one.
    fn current(&self, role: AppRole) -> Option<Check> {
        let generation = self.generation(role)?;
        CHECKS
            .lock(|checks| checks.borrow()[role as usize])
            .filter(|check| check.generation == generation)
    }

    fn progress(&self) -> Progress {
        let mut finished = Instant::MIN;
        for role in AppRole::ALL {
            if self.span(role).is_none() {
                continue;
            }
            match self.current(role) {
                Some(check) => finished = finished.max(check.finished),
                None => return Progress::Running,
            }
        }
        Progress::Done { finished }
    }

    /// Stores `check` for `role`, unless the slot's generation has moved
    /// since the check read it. Returns whether it was stored.
    fn store(&self, role: AppRole, check: Check) -> bool {
        CHECKS.lock(|checks| {
            let fresh = self.generation(role) == Some(check.generation);
            if fresh {
                checks.borrow_mut()[role as usize] = Some(check);
            }
            fresh
        })
    }

    /// The slot `role` names this boot, when the app booted from one.
    fn slot(&self, role: AppRole) -> Option<Slot> {
        let running = self.report.running?;
        Some(match role {
            AppRole::Current => running,
            AppRole::Alternate => running.other(),
        })
    }

    /// Checks each slot whose result isn't current, one after the other.
    async fn check_stale(&self, wd: &WatchdogHandle<wd::StorageCheck>) {
        for role in AppRole::ALL {
            if self.current(role).is_some() {
                continue;
            }
            match role {
                AppRole::Current => {
                    if let Some(app) = self.public.boot.running_app {
                        self.check(role, &app, wd).await;
                    }
                }
                AppRole::Alternate => {
                    if let Some(app) = self.public.other_app {
                        self.check(role, &app, wd).await;
                    }
                }
            }
        }
    }

    /// Checks the slot in `role` through `app`, its handle, and stores and
    /// logs the result. A result taken while the slot was written is
    /// dropped; the write wakes the task to check again.
    async fn check<P: Read + Source>(
        &self,
        role: AppRole,
        app: &P,
        wd: &WatchdogHandle<wd::StorageCheck>,
    ) {
        let generation = self.generations.of(app.offset(), app.len());
        let started = Instant::now();
        let image = verify(&Fed { app, wd }).await;
        let finished = Instant::now();
        let check = Check {
            image,
            finished,
            generation,
        };
        if self.store(role, check) {
            self.log(role, app.offset(), &check, finished - started);
        } else {
            info!(
                "Storage check of {=str}: written while checked; checking again once writes stop",
                role.name()
            );
        }
    }

    /// Logs the result of a check.
    fn log(&self, role: AppRole, offset: u32, check: &Check, took: Duration) {
        let slot = self
            .slot(role)
            .map_or("no slot", |slot| slot.partition().label);
        let ms = took.as_millis();
        match check.image {
            Image::Valid(app) => info!(
                "Storage check of {=str} ({=str} at {=u32:#x}): valid, {=u32} bytes, digest {}, file SHA-256 {}, in {=u64} ms",
                role.name(),
                slot,
                offset,
                app.size(),
                Hex(app.digest()),
                Hex(app.file_sha256()),
                ms,
            ),
            Image::Empty => info!(
                "Storage check of {=str} ({=str} at {=u32:#x}): empty, in {=u64} ms",
                role.name(),
                slot,
                offset,
                ms,
            ),
            Image::Invalid(err) => warn!(
                "Storage check of {=str} ({=str} at {=u32:#x}): invalid, {}, in {=u64} ms",
                role.name(),
                slot,
                offset,
                Debug2Format(&err),
                ms,
            ),
        }
    }
}

/// Verifies the app image in `source`. A source whose header is erased is
/// empty rather than invalid.
async fn verify<S: Source>(source: &S) -> Image {
    match image::verify_app(source).await {
        Ok(app) => Image::Valid(app),
        Err(ImageError::Magic(ERASED)) if erased_header(source) => Image::Empty,
        Err(err) => Image::Invalid(err),
    }
}

/// Whether the first [`HEADER_LEN`] bytes of `source` are erased, as no image
/// starts with them: its first byte is [`MAGIC`].
fn erased_header<S: Source>(source: &S) -> bool {
    const { assert!(MAGIC != ERASED) };
    let mut header = [0; HEADER_LEN as usize];
    source.read(0, &mut header).is_ok() && header.iter().all(|&byte| byte == ERASED)
}

/// A public handle whose reads feed the check task's watchdog: each read is
/// progress, so a check that stops reading misses its deadline.
struct Fed<'a, P> {
    app: &'a P,
    wd: &'a WatchdogHandle<wd::StorageCheck>,
}

impl<P: Read + Source> Source for Fed<'_, P> {
    fn size(&self) -> u32 {
        self.app.size()
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        self.wd.feed();
        Source::read(self.app, offset, buf)
    }
}

/// Checks the slots, then waits for writes; after each burst of writes,
/// once the flash has gone [`QUIET`], checks the slots they made stale.
#[embassy_executor::task]
async fn check_task(
    service: &'static Service,
    mut writes: Writes,
    wd: WatchdogHandle<wd::StorageCheck>,
) -> ! {
    loop {
        wd.feed();
        service.check_stale(&wd).await;
        if let Progress::Done { finished } = service.progress() {
            info!(
                "Storage checks done, the last at {=u64} ms after boot",
                finished.as_millis()
            );
        }
        wd.pause();
        writes.wait().await;
        while let Either::First(()) = select(writes.wait(), Timer::after(QUIET)).await {}
    }
}
