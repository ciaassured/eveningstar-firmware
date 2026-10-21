//! Requested reboots, for resets that aren't caused by a fault.
//!
//! Faults (a panic, a missed task watchdog deadline, a hung executor) reset the
//! chip at once, because the system's state can't be trusted. Other causes,
//! such as the Ethernet link staying down or a fresh OTA update being ready,
//! request a reboot instead, with a [`Reason`].
//!
//! Requesting a reboot takes a handle. [`start`] spawns the reboot task and
//! returns the one [`Reboot`], which requests graceful and immediate reboots.
//! The narrower handles derive from it: a [`GracefulReboot`] only requests
//! graceful reboots, and a [`RebootHolder`] only holds them off. A narrower
//! handle's methods match the [`Reboot`] methods they keep, so the handle
//! types are interchangeable wherever only those methods are used.
//!
//! A graceful reboot waits until no [`RebootHold`] is alive, so a task can hold
//! it off while it has work it must finish, such as flushing logged data. The
//! wait is bounded: after [`DEADLINE`] the chip resets anyway, and if the reboot
//! task itself stops, its task watchdog resets the chip after
//! [`WATCHDOG_TIMEOUT`]. An immediate reboot ignores holds and resets the chip
//! during the request.
//!
//! Each hold is taken for a [`Holder`], so the reboot [`Status`] names what a
//! graceful reboot is waiting for.

#![expect(
    clippy::unused_self,
    reason = "a handle's methods take it as proof of permission, not for data"
)]

use core::cell::RefCell;

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_deadline};

use crate::watchdog::WatchdogHandle;
use crate::wd;

/// Longest a graceful reboot waits for holds to be released.
pub(crate) const DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Task watchdog timeout of the reboot task. The task stops feeding when a
/// graceful reboot is requested, so this bounds the reboot if the task itself
/// stops.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(6 * 60);

/// How often the reboot task feeds its watchdog while no graceful reboot is
/// requested.
const FEED_INTERVAL: Duration = Duration::from_secs(30);

/// Why a reboot was requested.
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(crate) enum Reason {
    /// The Ethernet link was down, or had no IPv4 address, for too long.
    LinkDown,
    /// A client requested it through the API.
    Api,
}

/// What holds off a graceful reboot, as named in the [`Status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub(crate) enum Holder {
    /// The HTTP server, while it answers a reboot request.
    Http,
}

impl Holder {
    /// Every holder, in the order of their values.
    const ALL: [Self; 1] = [Self::Http];
}

/// Number of [`Holder`]s.
pub(crate) const HOLDERS: usize = Holder::ALL.len();

/// The kind of a reboot, for the log.
#[derive(Clone, Copy, Debug, defmt::Format)]
enum Mode {
    Graceful,
    Immediate,
}

/// A requested graceful reboot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pending {
    pub(crate) reason: Reason,
    /// When the chip resets, even if holds remain.
    pub(crate) deadline: Instant,
}

/// The reboot service's state, shared by the handles and the reboot task.
struct State {
    /// The requested graceful reboot. Once set, it stays set until the reset.
    pending: Option<Pending>,
    /// Live holds per holder, indexed by the holder's value.
    holds: [u16; HOLDERS],
}

impl State {
    fn is_held(&self) -> bool {
        self.holds.iter().any(|&count| count > 0)
    }
}

static STATE: Mutex<CriticalSectionRawMutex, RefCell<State>> = Mutex::new(RefCell::new(State {
    pending: None,
    holds: [0; HOLDERS],
}));

/// Signalled once, when a graceful reboot is first requested.
static REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Signalled whenever a hold is released.
static RELEASED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.lock(|state| f(&mut state.borrow_mut()))
}

/// Graceful reboots at one moment: the pending one, if any, and the holds a
/// graceful reboot waits for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Status {
    pub(crate) pending: Option<Pending>,
    holds: [u16; HOLDERS],
}

impl Status {
    /// Each holder with live holds, and how many.
    pub(crate) fn holds(&self) -> impl Iterator<Item = (Holder, u16)> + use<> {
        Holder::ALL
            .into_iter()
            .zip(self.holds)
            .filter(|&(_, count)| count > 0)
    }
}

/// Spawns the reboot task and returns the handle that requests reboots.
///
/// Takes the reboot task's watchdog handle, of which there is one, so there is
/// one call and a [`Reboot`] exists only while the task runs.
pub(crate) fn start(spawner: Spawner, wd: WatchdogHandle<wd::Reboot>) -> Reboot {
    spawner.spawn(defmt::unwrap!(reboot_task(wd)));
    Reboot(())
}

/// Permission to request graceful and immediate reboots. Only [`start`]
/// creates one, and the narrower handles derive from it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reboot(());

impl Reboot {
    /// Requests a graceful reboot: the reboot task reboots once no hold is
    /// alive or [`DEADLINE`] has passed. Only the first request takes effect.
    pub(crate) fn graceful(self, reason: Reason) {
        request_graceful(reason);
    }

    /// Reboots now, ignoring holds, even while a graceful reboot is waiting
    /// for them.
    pub(crate) fn immediate(self, reason: Reason) -> ! {
        reset(reason, Mode::Immediate)
    }

    /// Whether a graceful reboot has been requested.
    #[expect(
        dead_code,
        reason = "nothing checks for a pending reboot with this handle yet"
    )]
    pub(crate) fn is_pending(self) -> bool {
        is_pending()
    }

    /// The pending graceful reboot and the holds it waits for.
    pub(crate) fn status(self) -> Status {
        status()
    }

    /// A handle that can only request graceful reboots.
    pub(crate) fn graceful_only(self) -> GracefulReboot {
        GracefulReboot(())
    }

    /// A handle that can only hold off graceful reboots, for `holder`.
    pub(crate) fn holder(self, holder: Holder) -> RebootHolder {
        RebootHolder(holder)
    }
}

/// Permission to request graceful reboots only; see [`Reboot::graceful_only`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct GracefulReboot(());

impl GracefulReboot {
    /// See [`Reboot::graceful`].
    pub(crate) fn graceful(self, reason: Reason) {
        request_graceful(reason);
    }

    /// See [`Reboot::is_pending`].
    pub(crate) fn is_pending(self) -> bool {
        is_pending()
    }
}

/// Permission to hold off graceful reboots only, for one [`Holder`]; see
/// [`Reboot::holder`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct RebootHolder(Holder);

impl RebootHolder {
    /// Holds off a graceful reboot until the returned guard is dropped.
    pub(crate) fn hold(self) -> RebootHold {
        with_state(|state| state.holds[self.0 as usize] += 1);
        RebootHold(self.0)
    }
}

/// Holds off a graceful reboot while alive; see [`RebootHolder::hold`].
#[must_use = "the reboot is only held off while the guard is alive"]
#[derive(Debug)]
pub(crate) struct RebootHold(Holder);

impl Drop for RebootHold {
    fn drop(&mut self) {
        with_state(|state| state.holds[self.0 as usize] -= 1);
        RELEASED.signal(());
    }
}

/// Records a graceful reboot and signals the reboot task, unless one is
/// already pending.
fn request_graceful(reason: Reason) {
    let first = with_state(|state| {
        let first = state.pending.is_none();
        if first {
            state.pending = Some(Pending {
                reason,
                deadline: Instant::now() + DEADLINE,
            });
        }
        first
    });
    if first {
        REQUESTED.signal(());
    }
}

fn is_pending() -> bool {
    with_state(|state| state.pending.is_some())
}

fn status() -> Status {
    with_state(|state| Status {
        pending: state.pending,
        holds: state.holds,
    })
}

/// Waits for a graceful reboot request, then for every hold to be released or
/// the deadline to pass, and resets the chip.
#[embassy_executor::task]
async fn reboot_task(wd: WatchdogHandle<wd::Reboot>) -> ! {
    loop {
        wd.feed();
        if let Either::First(()) = select(REQUESTED.wait(), Timer::after(FEED_INTERVAL)).await {
            break;
        }
    }

    // Set before the request is signalled, and never cleared.
    let Status { pending, .. } = status();
    let pending = defmt::unwrap!(pending);
    info!(
        "Reboot requested ({}), waiting up to {} s",
        pending.reason,
        DEADLINE.as_secs(),
    );
    log_holds("Waiting for");

    let released = with_deadline(pending.deadline, async {
        while with_state(|state| state.is_held()) {
            RELEASED.wait().await;
        }
    })
    .await;
    if released.is_err() {
        warn!("Reboot holds not released within {} s", DEADLINE.as_secs());
        log_holds("Still held by");
    }
    reset(pending.reason, Mode::Graceful)
}

/// Logs each holder with live holds, after `prefix`.
fn log_holds(prefix: &str) {
    for (holder, count) in status().holds() {
        info!("{} {}: {} holds", prefix, holder, count);
    }
}

/// Logs the reboot and resets the chip.
#[expect(
    clippy::disallowed_methods,
    reason = "the reboot service performs requested reboots"
)]
fn reset(reason: Reason, mode: Mode) -> ! {
    info!("Rebooting ({}, {})", reason, mode);
    esp_hal::system::software_reset()
}
