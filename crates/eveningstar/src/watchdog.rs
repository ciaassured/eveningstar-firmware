//! Task and hardware watchdogs.
//!
//! Resets the chip if a supervised task stops making progress, or if the
//! executor itself hangs.
//!
//! There are three layers, each a backstop for the one above it:
//!
//! | Layer            | Catches                              | Action                   |
//! | ---------------- | ------------------------------------ | ------------------------ |
//! | [`Supervisor`]   | a task that stops feeding its handle | log, then software reset |
//! | MWDT0 (`TIMG0`)  | the supervisor or executor hanging   | digital core reset       |
//! | RWDT (LP domain) | the MWDT path failing (e.g. clocks)  | full system reset        |
//!
//! Each task gets a [`WatchdogHandle`] and calls [`WatchdogHandle::feed`]
//! regularly. The supervisor polls every [`POLL_INTERVAL`]: it records which
//! handles were fed, resets the chip if any task has gone longer than its
//! timeout, and otherwise feeds both hardware watchdogs.
//!
//! A task that waits for an outside event can [pause](WatchdogHandle::pause)
//! its handle: its deadline isn't checked until its next feed, so a hang while
//! paused isn't caught. A task that waits for a known time longer than its
//! timeout can instead [sleep](WatchdogHandle::sleep) for that time, which
//! replaces its timeout until its next feed. A task can also
//! [retire](WatchdogHandle::retire) its handle, which ends its supervision for
//! the rest of the boot.
//!
//! # Usage
//!
//! Declare the supervised tasks once with [`watchdog!`], inside a module:
//!
//! ```ignore
//! mod wd {
//!     use embassy_time::Duration;
//!
//!     crate::watchdog::watchdog! {
//!         worker: Worker = Duration::from_secs(10),
//!     }
//! }
//!
//! let (supervisor, handles) = wd::supervisor(rtc.rwdt, timg0.wdt);
//! spawner.spawn(wd::supervise(supervisor)?);
//! spawner.spawn(worker(handles.worker)?); // takes a `WatchdogHandle<wd::Worker>`
//! ```
//!
//! Only one set of handles can exist: building the supervisor consumes the
//! `Rwdt` and `Wdt<TIMG0>`, which the HAL hands out once.

use core::marker::PhantomData;
use defmt::{error, info};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Ticker};
use esp_hal::{
    peripherals::TIMG0,
    rtc_cntl::{Rwdt, RwdtStage, RwdtStageAction},
    timer::timg::{MwdtStage, MwdtStageAction, Wdt},
};

/// How often the supervisor checks task mailboxes and feeds the hardware watchdogs.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// MWDT0 catches a hung supervisor/executor, so it only needs to cover a few missed polls.
const TIMER0_WD_TIMEOUT: esp_hal::time::Duration =
    esp_hal::time::Duration::from_millis(POLL_INTERVAL.as_millis() * 5);

/// RWDT is the backstop for the MWDT path, so it must outlast it.
const RTC_WD_TIMEOUT: esp_hal::time::Duration =
    esp_hal::time::Duration::from_millis(TIMER0_WD_TIMEOUT.as_millis() * 3);

/// What a task's [`WatchdogHandle`] last told the [`Supervisor`].
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(crate) enum Message {
    /// The task is still making progress.
    Fed,
    /// The task is waiting for an outside event; its deadline isn't checked
    /// until it feeds again.
    Pause,
    /// The task is waiting for this long; until it feeds again, this is its
    /// deadline instead of its timeout.
    Sleep(Duration),
    /// The task is no longer supervised.
    Retired,
}

/// Carries a task's latest [`Message`] to the [`Supervisor`].
///
/// A [`Signal`] only keeps the latest message since the last poll, not how
/// many were sent, which is all the supervisor needs. A later message replaces
/// an earlier one: a pause or sleep sent after a feed leaves the task paused
/// or asleep, a feed sent after either supervises it again from that poll, and
/// a retire replaces any.
pub(crate) type Mailbox = Signal<CriticalSectionRawMutex, Message>;

/// Marker for a supervised task. [`watchdog!`] generates one type per task, so
/// each task's handle has a distinct type and can't be passed to another task.
pub(crate) trait WatchdogType {}

/// A task's connection to the supervisor. Unless the handle is paused or
/// retired, the task must call [`feed`](Self::feed) at least once per its
/// timeout, or the chip resets; once asleep, at least once per the time it
/// slept for.
///
/// Not `Clone` or `Copy`: each task owns exactly one handle.
pub(crate) struct WatchdogHandle<T: WatchdogType> {
    _t: PhantomData<T>,
    mailbox: &'static Mailbox,
}

impl<T: WatchdogType> WatchdogHandle<T> {
    pub(crate) fn new(mailbox: &'static Mailbox) -> WatchdogHandle<T> {
        WatchdogHandle {
            _t: PhantomData,
            mailbox,
        }
    }

    /// Tells the supervisor the task is making progress, and ends a pause or
    /// a sleep: the timeout runs again from here.
    pub(crate) fn feed(&self) {
        self.mailbox.signal(Message::Fed);
    }

    /// Stops the deadline until the next [`feed`](Self::feed), while the task
    /// waits for an outside event.
    pub(crate) fn pause(&self) {
        self.mailbox.signal(Message::Pause);
    }

    /// Replaces the timeout with `time` until the next [`feed`](Self::feed),
    /// while the task waits for that long. The chip resets as for a missed
    /// deadline if no feed comes within `time`.
    pub(crate) fn sleep(&self, time: Duration) {
        self.mailbox.signal(Message::Sleep(time));
    }

    /// Ends supervision of this task for the rest of the boot. The handle is
    /// consumed, so the task can't feed again.
    pub(crate) fn retire(self) {
        self.mailbox.signal(Message::Retired);
    }
}

/// The supervisor's view of one task.
pub(crate) struct WatchdogInfo {
    /// Task name for the log when its deadline is missed.
    pub name: &'static str,
    /// Longest allowed gap between feeds.
    pub timeout: Duration,
    /// When the supervisor last saw a feed, or the task began to sleep.
    /// Detection is per poll, so this can lag the real feed by up to one
    /// [`POLL_INTERVAL`].
    pub last_fed: Instant,
    /// What the task is waiting for, from its pause or sleep until its next
    /// feed.
    pub wait: Wait,
    /// Set once the task retires its handle; its deadline is no longer checked.
    pub retired: bool,
    pub mailbox: &'static Mailbox,
}

/// How long the supervisor lets a task go without a feed.
#[derive(Clone, Copy)]
pub(crate) enum Wait {
    /// The task's timeout.
    Timeout,
    /// This long, instead of the timeout.
    Sleep(Duration),
    /// For ever.
    Paused,
}

impl WatchdogInfo {
    /// Applies the task's latest message, seen at `now`.
    fn receive(&mut self, message: Message, now: Instant) {
        match message {
            Message::Fed => {
                self.last_fed = now;
                self.wait = Wait::Timeout;
            }
            Message::Pause => self.wait = Wait::Paused,
            Message::Sleep(time) => {
                self.last_fed = now;
                self.wait = Wait::Sleep(time);
            }
            Message::Retired => self.retire(),
        }
    }

    /// Whether the task's deadline has passed at `now`.
    fn missed(&self, now: Instant) -> bool {
        if self.retired {
            return false;
        }
        let allowed = match self.wait {
            Wait::Timeout => self.timeout,
            Wait::Sleep(time) => time,
            Wait::Paused => return false,
        };
        now.duration_since(self.last_fed) > allowed
    }

    fn retire(&mut self) {
        if !self.retired {
            self.retired = true;
            info!("Task watchdog `{}` retired", self.name);
        }
    }
}

/// Owns the hardware watchdogs and checks every task's deadline.
/// If deadlines are not met, the chip is reset.
pub(crate) struct Supervisor<const N: usize> {
    task_watchdogs: &'static mut [WatchdogInfo; N],
    rtc_watchdog: Rwdt,
    timer0_watchdog: Wdt<TIMG0<'static>>,
}

impl<const N: usize> Supervisor<N> {
    /// Takes ownership of the hardware watchdogs and enables them. From here
    /// the chip resets unless [`run`](Self::run) starts within
    /// `TIMER0_WD_TIMEOUT`.
    pub(crate) fn new(
        task_watchdogs: &'static mut [WatchdogInfo; N],
        mut rtc_watchdog: Rwdt,
        mut timer0_watchdog: Wdt<TIMG0<'static>>,
    ) -> Supervisor<N> {
        timer0_watchdog.enable();
        timer0_watchdog.set_stage_action(MwdtStage::Stage0, MwdtStageAction::ResetSystem);
        timer0_watchdog.set_timeout(MwdtStage::Stage0, TIMER0_WD_TIMEOUT);
        timer0_watchdog.feed();

        rtc_watchdog.enable();
        rtc_watchdog.set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
        rtc_watchdog.set_timeout(RwdtStage::Stage0, RTC_WD_TIMEOUT);
        rtc_watchdog.feed();

        Supervisor {
            task_watchdogs,
            rtc_watchdog,
            timer0_watchdog,
        }
    }

    /// Polls forever and monitors each task for feed timeouts.
    pub(crate) async fn run(mut self) -> ! {
        // Reset all task watchdog timers to the start of the supervisor. A
        // pause, sleep or retire sent before now is kept, and a feed counts from
        // the start.
        let start = Instant::now();
        for wd in self.task_watchdogs.iter_mut() {
            if let Some(message) = wd.mailbox.try_take() {
                wd.receive(message, start);
            }
            wd.last_fed = start;
        }

        info!("Watchdog supervisor started");

        let mut ticker = Ticker::every(POLL_INTERVAL);
        loop {
            self.timer0_watchdog.feed();
            self.rtc_watchdog.feed();

            ticker.next().await;

            let now = Instant::now();
            for wd in self.task_watchdogs.iter_mut() {
                if let Some(message) = wd.mailbox.try_take() {
                    wd.receive(message, now);
                }

                if wd.missed(now) {
                    error!("Task watchdog `{}` missed its deadline", wd.name);
                    #[expect(
                        clippy::disallowed_methods,
                        reason = "a missed deadline resets at once, as the task may be hung"
                    )]
                    esp_hal::system::software_reset();
                }
            }
        }
    }
}

/// Declares the supervised tasks, as `field: Kind = timeout,` per task.
///
/// Call it once, inside a module of its own. It generates in that module:
///
/// - a marker type `Kind` per task, for `WatchdogHandle<Kind>`;
/// - `Handles`, holding one handle per task in field `field`;
/// - `supervisor(rwdt, wdt)`, which builds the [`Supervisor`] and the `Handles`;
/// - `supervise`, the embassy task that runs the supervisor.
///
/// Each `timeout` must be a const expression of at least 2× [`POLL_INTERVAL`];
/// this is checked at compile time.
macro_rules! watchdog {
    ( $( $field:ident: $Kind:ident = $timeout:expr, )+ ) => {
        use $crate::watchdog::{WatchdogType, WatchdogHandle, WatchdogInfo, Supervisor, Mailbox, Wait};
        use esp_hal::{peripherals::TIMG0, rtc_cntl::Rwdt, timer::timg::Wdt};
        use embassy_time::Instant;
        use static_cell::ConstStaticCell;

        $(
            const _: () = assert!(
                $timeout.as_ticks() >= 2 * $crate::watchdog::POLL_INTERVAL.as_ticks(),
                concat!("watchdog `", stringify!($field), "`: timeout must be at least 2x POLL_INTERVAL"),
            );
        )*

        $(
            pub(crate) struct $Kind;
            impl WatchdogType for $Kind {}
        )*

        // Indexes of each watchdog type to allow const sizing.
        enum Index {
            $(
                $Kind,
            )*
        }
        const COUNT: usize = [ $( Index::$Kind, )* ].len();
        static MAILBOXES: [Mailbox; COUNT] = [ const { Mailbox::new() }; COUNT];

        // The supervisor's view of each task, built at compile time so that
        // no frame holds it.
        static INFOS: ConstStaticCell<[WatchdogInfo; COUNT]> = ConstStaticCell::new([
            $(
                WatchdogInfo {
                    name: stringify!($field),
                    timeout: $timeout,
                    last_fed: Instant::MIN,
                    wait: Wait::Timeout,
                    retired: false,
                    mailbox: &MAILBOXES[Index::$Kind as usize],
                },
            )*
        ]);

        // Handles of each task watchdog to pass the caller.
        pub(crate) struct Handles {
            $(
                pub(crate) $field: WatchdogHandle<$Kind>,
            )*
        }

        pub(crate) fn supervisor(rwdt: Rwdt, wdt: Wdt<TIMG0<'static>>) -> (Supervisor<COUNT>, Handles) {
            let supervisor = Supervisor::new(INFOS.take(), rwdt, wdt);

            let handles = Handles {
                $(
                    $field: WatchdogHandle::new(&MAILBOXES[Index::$Kind as usize]),
                )*
            };

            (supervisor, handles)
        }

        #[embassy_executor::task]
        pub(crate) async fn supervise(supervisor: Supervisor<COUNT>) -> ! {
            supervisor.run().await
        }
    };
}

pub(crate) use watchdog;
