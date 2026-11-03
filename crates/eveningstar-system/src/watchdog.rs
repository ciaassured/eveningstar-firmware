//! Task and hardware watchdogs.
//!
//! Resets the chip if a supervised task stops making progress, or if the
//! executor itself hangs.
//!
//! There are three layers, each a backstop for the one above it:
//!
//! | Layer               | Catches                              | Action                     |
//! |---------------------|--------------------------------------|----------------------------|
//! | [`Supervisor`]      | a task that stops feeding its handle | log, then [`Reset::reset`] |
//! | main watchdog       | the supervisor or executor hanging   | chip reset                 |
//! | backstop watchdog   | the main watchdog's path failing     | full system reset          |
//!
//! The main and backstop watchdogs are the chip's, behind
//! [`HardwareWatchdogs`]. The supervisor enables them when it is made, with
//! [`MAIN_TIMEOUT`] and [`BACKSTOP_TIMEOUT`], and feeds both on every poll.
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
//! Each supervised task is a [`TaskWatchdog<T>`], a name and a timeout for the
//! marker type `T`, which [`Supervisor::register`] takes in one of the
//! supervisor's `N` slots. The handle it returns is a [`WatchdogHandle<T>`],
//! so each task's handle has a type of its own and one task's handle isn't
//! another's. A marker is registered once per supervisor. The slots' mailboxes
//! are a [`Mailboxes`], which the supervisor takes as `&'static mut`, so one
//! supervisor exists per [`Mailboxes`].
//!
//! ```ignore
//! enum Worker {}
//! const WORKER: TaskWatchdog<Worker> = TaskWatchdog::new("worker", Duration::from_secs(10));
//!
//! static MAILBOXES: ConstStaticCell<Mailboxes<1>> = ConstStaticCell::new(Mailboxes::new());
//!
//! let mut supervisor = Supervisor::new(MAILBOXES.take(), hardware, reset);
//! let worker: WatchdogHandle<Worker> = supervisor.register(WORKER);
//! // Spawned as tasks, which call `supervisor.run()` and `worker.feed()`.
//! ```

use core::any::TypeId;
use core::fmt;
use core::marker::PhantomData;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker};

use crate::{Reset, ResetCause};

/// How often the supervisor checks task mailboxes and feeds the hardware
/// watchdogs.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Timeout of the main hardware watchdog. It catches a hung supervisor or
/// executor, so it only needs to cover a few missed polls.
pub const MAIN_TIMEOUT: Duration = Duration::from_ticks(POLL_INTERVAL.as_ticks() * 5);

/// Timeout of the backstop hardware watchdog. It is the backstop for the main
/// watchdog's path, so it outlasts it.
pub const BACKSTOP_TIMEOUT: Duration = Duration::from_ticks(MAIN_TIMEOUT.as_ticks() * 3);

/// The chip's hardware watchdogs, which the [`Supervisor`] enables and feeds.
pub trait HardwareWatchdogs {
    /// Enables both watchdogs and feeds them: the main one resets the chip
    /// unless fed within `main`, the backstop resets the whole system unless
    /// fed within `backstop`.
    fn enable(&mut self, main: Duration, backstop: Duration);

    /// Feeds both watchdogs.
    fn feed(&mut self);
}

/// What a task's [`WatchdogHandle`] last told the [`Supervisor`].
#[derive(Clone, Copy, Debug)]
enum Message {
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
type Mailbox = Signal<CriticalSectionRawMutex, Message>;

/// The mailboxes of a [`Supervisor`]'s `N` slots, one per supervised task.
pub struct Mailboxes<const N: usize>([Mailbox; N]);

impl<const N: usize> Mailboxes<N> {
    /// `N` empty mailboxes.
    #[must_use]
    pub const fn new() -> Self {
        Self([const { Mailbox::new() }; N])
    }
}

impl<const N: usize> Default for Mailboxes<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Debug for Mailboxes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailboxes").finish_non_exhaustive()
    }
}

/// A supervised task, of marker type `T`: its name, for the log, and its
/// timeout, the longest allowed gap between feeds.
pub struct TaskWatchdog<T> {
    name: &'static str,
    timeout: Duration,
    marker: PhantomData<fn() -> T>,
}

impl<T> TaskWatchdog<T> {
    /// A task named `name` with `timeout`.
    ///
    /// # Panics
    ///
    /// If `timeout` is less than 2× [`POLL_INTERVAL`]. In a `const` item, that
    /// fails the build.
    #[must_use]
    pub const fn new(name: &'static str, timeout: Duration) -> Self {
        assert!(
            timeout.as_ticks() >= 2 * POLL_INTERVAL.as_ticks(),
            "a task watchdog's timeout must be at least 2x POLL_INTERVAL",
        );
        Self {
            name,
            timeout,
            marker: PhantomData,
        }
    }

    /// The task's name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The longest allowed gap between feeds.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl<T> Clone for TaskWatchdog<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for TaskWatchdog<T> {}

impl<T> fmt::Debug for TaskWatchdog<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskWatchdog")
            .field("name", &self.name)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A task's connection to the supervisor, for the task of marker type `T`.
/// Unless the handle is paused or retired, the task must call
/// [`feed`](Self::feed) at least once per its timeout, or the chip resets;
/// once asleep, at least once per the time it slept for.
///
/// Not `Clone` or `Copy`: each registered task has exactly one handle, and
/// each marker is registered once. A handle of one marker isn't a handle of
/// another:
///
/// ```compile_fail,E0308
/// use eveningstar_system::watchdog::WatchdogHandle;
///
/// enum Blinky {}
/// enum Ethernet {}
///
/// fn blink(_wd: WatchdogHandle<Blinky>) {}
///
/// fn ethernet(wd: WatchdogHandle<Ethernet>) {
///     blink(wd);
/// }
/// ```
pub struct WatchdogHandle<T> {
    mailbox: &'static Mailbox,
    marker: PhantomData<fn() -> T>,
}

impl<T> WatchdogHandle<T> {
    /// Tells the supervisor the task is making progress, and ends a pause or
    /// a sleep: the timeout runs again from here.
    pub fn feed(&self) {
        self.mailbox.signal(Message::Fed);
    }

    /// Stops the deadline until the next [`feed`](Self::feed), while the task
    /// waits for an outside event.
    pub fn pause(&self) {
        self.mailbox.signal(Message::Pause);
    }

    /// Replaces the timeout with `time` until the next [`feed`](Self::feed),
    /// while the task waits for that long. The chip resets as for a missed
    /// deadline if no feed comes within `time`.
    pub fn sleep(&self, time: Duration) {
        self.mailbox.signal(Message::Sleep(time));
    }

    /// Ends supervision of this task for the rest of the boot. The handle is
    /// consumed, so the task can't feed again.
    pub fn retire(self) {
        self.mailbox.signal(Message::Retired);
    }
}

impl<T> fmt::Debug for WatchdogHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchdogHandle").finish_non_exhaustive()
    }
}

/// How long the supervisor lets a task go without a feed.
#[derive(Clone, Copy, Debug)]
enum Wait {
    /// The task's timeout.
    Timeout,
    /// This long, instead of the timeout.
    Sleep(Duration),
    /// For ever.
    Paused,
}

/// The supervisor's view of one task.
struct Task {
    name: &'static str,
    timeout: Duration,
    /// The task's marker type, which no other task has.
    marker: TypeId,
    /// When the supervisor last saw a feed, or the task began to sleep.
    /// Detection is per poll, so this can lag the real feed by up to one
    /// [`POLL_INTERVAL`].
    last_fed: Instant,
    /// What the task is waiting for, from its pause or sleep until its next
    /// feed.
    wait: Wait,
    /// Set once the task retires its handle; its deadline is no longer checked.
    retired: bool,
    mailbox: &'static Mailbox,
}

impl Task {
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
pub struct Supervisor<const N: usize, W> {
    mailboxes: &'static [Mailbox; N],
    tasks: heapless::Vec<Task, N>,
    hardware: W,
    reset: &'static dyn Reset,
}

impl<const N: usize, W: HardwareWatchdogs> Supervisor<N, W> {
    /// Takes the hardware watchdogs and enables them, with [`MAIN_TIMEOUT`]
    /// and [`BACKSTOP_TIMEOUT`]. From here the chip resets unless
    /// [`run`](Self::run) starts within [`MAIN_TIMEOUT`]. A missed deadline
    /// resets the chip through `reset`.
    pub fn new(
        mailboxes: &'static mut Mailboxes<N>,
        mut hardware: W,
        reset: &'static dyn Reset,
    ) -> Self {
        hardware.enable(MAIN_TIMEOUT, BACKSTOP_TIMEOUT);
        Self {
            mailboxes: &mailboxes.0,
            tasks: heapless::Vec::new(),
            hardware,
            reset,
        }
    }

    /// Supervises `watchdog`'s task in the next free slot, and returns the
    /// task's handle. Its timeout runs from the start of [`run`](Self::run).
    ///
    /// # Panics
    ///
    /// If all `N` slots are taken, or a task of marker type `T` is registered
    /// already.
    pub fn register<T: 'static>(&mut self, watchdog: TaskWatchdog<T>) -> WatchdogHandle<T> {
        let marker = TypeId::of::<T>();
        assert!(
            self.tasks.iter().all(|task| task.marker != marker),
            "the task watchdog `{}`'s marker type is registered already",
            watchdog.name,
        );
        let Some(mailbox) = self.mailboxes.get(self.tasks.len()) else {
            panic!("every task watchdog slot is taken");
        };
        // Fits: `tasks` holds `N`, as many as there are mailboxes.
        let _ = self.tasks.push(Task {
            name: watchdog.name,
            timeout: watchdog.timeout,
            marker,
            last_fed: Instant::MIN,
            wait: Wait::Timeout,
            retired: false,
            mailbox,
        });
        WatchdogHandle {
            mailbox,
            marker: PhantomData,
        }
    }

    /// Polls forever and monitors each task for feed timeouts.
    pub async fn run(mut self) -> ! {
        // Reset all task watchdog timers to the start of the supervisor. A
        // pause, sleep or retire sent before now is kept, and a feed counts from
        // the start.
        let start = Instant::now();
        for task in &mut self.tasks {
            if let Some(message) = task.mailbox.try_take() {
                task.receive(message, start);
            }
            task.last_fed = start;
        }

        info!("Watchdog supervisor started");

        let mut ticker = Ticker::every(POLL_INTERVAL);
        loop {
            self.hardware.feed();

            ticker.next().await;

            let now = Instant::now();
            for task in &mut self.tasks {
                if let Some(message) = task.mailbox.try_take() {
                    task.receive(message, now);
                }

                if task.missed(now) {
                    let name = task.name;
                    error!("Task watchdog `{}` missed its deadline", name);
                    self.reset.reset(ResetCause::MissedDeadline { task: name });
                }
            }
        }
    }
}

impl<const N: usize, W> fmt::Debug for Supervisor<N, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Supervisor")
            .field("tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
