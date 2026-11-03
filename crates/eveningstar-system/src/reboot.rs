//! Requested reboots, for resets that aren't caused by a fault.
//!
//! Faults (a panic, a missed task watchdog deadline, a hung executor) reset the
//! chip at once, because the system's state can't be trusted. Other causes
//! request a reboot instead, with a reason of type `R`.
//!
//! The service's state is a [`RebootService`], in a `static`. Requesting a
//! reboot takes a handle. [`RebootService::start`] returns the one [`Reboot`],
//! which requests graceful and immediate reboots, and the [`RebootTask`] that
//! performs graceful ones. The narrower handles derive from it: a
//! [`GracefulReboot`] only requests graceful reboots, and a [`RebootHolder`]
//! only holds them off. A narrower handle's methods match the [`Reboot`]
//! methods they keep, so the handle types are interchangeable wherever only
//! those methods are used.
//!
//! [`RebootRequest`] requests graceful reboots with reasons of another type,
//! which convert to `R` with `From`. It is object-safe, so a crate with
//! reasons of its own takes a `&dyn RebootRequest` of its reason type and
//! names neither `R` nor the service.
//!
//! A graceful reboot waits until no [`RebootHold`] is alive, so a task can hold
//! it off while it has work it must finish, such as flushing logged data. The
//! wait is bounded: after [`DEADLINE`] the chip resets anyway, and if the reboot
//! task itself stops, its task watchdog ([`WATCHDOG`]) resets the chip after
//! [`WATCHDOG_TIMEOUT`]. An immediate reboot ignores holds and resets the chip
//! during the request.
//!
//! Each hold is taken through a [`RebootHolder`], registered for a holder of
//! type `H` in one of the service's `N` slots, so the reboot [`Status`] names
//! what a graceful reboot is waiting for. Each holder is registered once.

use core::cell::RefCell;
use core::fmt;

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_deadline};

use crate::fmt::debug;
use crate::watchdog::{TaskWatchdog, WatchdogHandle};
use crate::{Reset, ResetCause};

/// Longest a graceful reboot waits for holds to be released.
pub const DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Task watchdog timeout of the reboot task. The task stops feeding when a
/// graceful reboot is requested, so this bounds the reboot if the task itself
/// stops.
pub const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(6 * 60);

/// Marker type of the reboot task's watchdog.
#[derive(Debug)]
pub enum RebootWatchdog {}

/// The reboot task's watchdog, named `reboot`, with [`WATCHDOG_TIMEOUT`].
pub const WATCHDOG: TaskWatchdog<RebootWatchdog> = TaskWatchdog::new("reboot", WATCHDOG_TIMEOUT);

/// How often the reboot task feeds its watchdog while no graceful reboot is
/// requested.
const FEED_INTERVAL: Duration = Duration::from_secs(30);

/// The kind of a reboot, for the log.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum Mode {
    Graceful,
    Immediate,
}

/// A requested graceful reboot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending<R> {
    /// Why it was requested.
    pub reason: R,
    /// When the chip resets, even if holds remain.
    pub deadline: Instant,
}

/// The reboot service's state, shared by the handles and the reboot task.
struct State<R, H, const N: usize> {
    /// The requested graceful reboot. Once set, it stays set until the reset.
    pending: Option<Pending<R>>,
    /// The holders, in the order they were registered.
    holders: [Option<H>; N],
    /// Live holds per slot of `holders`.
    holds: [u16; N],
    /// What the service resets the chip through, from its start.
    reset: Option<&'static dyn Reset>,
}

impl<R, H, const N: usize> State<R, H, N> {
    fn is_held(&self) -> bool {
        self.holds.iter().any(|&count| count > 0)
    }
}

/// The reboot service, with reasons of type `R`, holders of type `H` and `N`
/// holder slots.
pub struct RebootService<R, H, const N: usize> {
    state: Mutex<CriticalSectionRawMutex, RefCell<State<R, H, N>>>,
    /// Signalled once, when a graceful reboot is first requested.
    requested: Signal<CriticalSectionRawMutex, ()>,
    /// Signalled whenever a hold is released.
    released: Signal<CriticalSectionRawMutex, ()>,
}

impl<R, H, const N: usize> RebootService<R, H, N> {
    /// A service that hasn't started, with no reboot requested and no holder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(State {
                pending: None,
                holders: [const { None }; N],
                holds: [0; N],
                reset: None,
            })),
            requested: Signal::new(),
            released: Signal::new(),
        }
    }
}

impl<R, H, const N: usize> Default for RebootService<R, H, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R, H, const N: usize> fmt::Debug for RebootService<R, H, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RebootService").finish_non_exhaustive()
    }
}

impl<R, H, const N: usize> RebootService<R, H, N>
where
    R: Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    /// Starts the service, which resets the chip through `reset`, and returns
    /// the handle that requests reboots, and the reboot task, which feeds `wd`
    /// until a graceful reboot is requested.
    ///
    /// The service starts once, so one [`Reboot`] exists per service. Until
    /// its [`RebootTask`] runs, a graceful reboot waits for ever; `wd` resets
    /// the chip if it doesn't run.
    ///
    /// # Panics
    ///
    /// If the service has started already.
    pub fn start(
        &'static self,
        reset: &'static dyn Reset,
        wd: WatchdogHandle<RebootWatchdog>,
    ) -> (Reboot<R, H, N>, RebootTask<R, H, N>) {
        let first = self.with_state(|state| state.reset.replace(reset).is_none());
        assert!(first, "the reboot service has started already");
        (Reboot { service: self }, RebootTask { service: self, wd })
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut State<R, H, N>) -> T) -> T {
        self.state.lock(|state| f(&mut state.borrow_mut()))
    }

    /// Records a graceful reboot and signals the reboot task, unless one is
    /// already pending.
    fn request_graceful(&self, reason: R) {
        let first = self.with_state(|state| {
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
            self.requested.signal(());
        }
    }

    fn is_pending(&self) -> bool {
        self.with_state(|state| state.pending.is_some())
    }

    fn status(&self) -> Status<R, H, N> {
        self.with_state(|state| Status {
            pending: state.pending,
            holders: state.holders,
            holds: state.holds,
        })
    }

    /// Registers `holder` in the next free slot.
    fn register_holder(&'static self, holder: H) -> RebootHolder<R, H, N> {
        let slot = self.with_state(|state| {
            if state.holders.contains(&Some(holder)) {
                return Err("the reboot holder is registered already");
            }
            let slot = state
                .holders
                .iter()
                .position(Option::is_none)
                .ok_or("every reboot holder slot is taken")?;
            state.holders[slot] = Some(holder);
            Ok(slot)
        });
        match slot {
            Ok(slot) => RebootHolder {
                service: self,
                slot,
            },
            Err(message) => panic!("{message}: {holder:?}"),
        }
    }

    /// Logs each holder with live holds, after `prefix`.
    fn log_holds(&self, prefix: &str) {
        for (holder, count) in self.status().holds() {
            info!("{} {}: {} holds", prefix, debug(&holder), count);
        }
    }

    /// Logs the reboot and resets the chip.
    fn reset(&self, reason: R, mode: Mode) -> ! {
        info!("Rebooting ({}, {})", debug(&reason), mode);
        // Set by `start`, which made every handle and the reboot task.
        let Some(reset) = self.with_state(|state| state.reset) else {
            unreachable!("the reboot service resets only once started");
        };
        reset.reset(match mode {
            Mode::Graceful => ResetCause::GracefulReboot,
            Mode::Immediate => ResetCause::ImmediateReboot,
        })
    }
}

/// Graceful reboots at one moment: the pending one, if any, and the holds a
/// graceful reboot waits for.
#[derive(Clone, Copy, Debug)]
pub struct Status<R, H, const N: usize> {
    /// The requested graceful reboot, if one is pending.
    pub pending: Option<Pending<R>>,
    holders: [Option<H>; N],
    holds: [u16; N],
}

impl<R, H: Copy, const N: usize> Status<R, H, N> {
    /// Each holder with live holds, and how many, in the order the holders
    /// were registered.
    pub fn holds(&self) -> impl Iterator<Item = (H, u16)> + use<R, H, N> {
        self.holders
            .into_iter()
            .zip(self.holds)
            .filter_map(|(holder, count)| holder.map(|holder| (holder, count)))
            .filter(|&(_, count)| count > 0)
    }
}

/// Permission to request graceful and immediate reboots. Only
/// [`RebootService::start`] creates one, and the narrower handles derive from
/// it.
pub struct Reboot<R: 'static, H: 'static, const N: usize> {
    service: &'static RebootService<R, H, N>,
}

impl<R, H, const N: usize> Reboot<R, H, N>
where
    R: Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    /// Requests a graceful reboot: the reboot task reboots once no hold is
    /// alive or [`DEADLINE`] has passed. Only the first request takes effect.
    pub fn graceful(self, reason: R) {
        self.service.request_graceful(reason);
    }

    /// Reboots now, ignoring holds, even while a graceful reboot is waiting
    /// for them.
    pub fn immediate(self, reason: R) -> ! {
        self.service.reset(reason, Mode::Immediate)
    }

    /// Whether a graceful reboot has been requested.
    #[must_use]
    pub fn is_pending(self) -> bool {
        self.service.is_pending()
    }

    /// The pending graceful reboot and the holds it waits for.
    #[must_use]
    pub fn status(self) -> Status<R, H, N> {
        self.service.status()
    }

    /// A handle that can only request graceful reboots.
    #[must_use]
    pub fn graceful_only(self) -> GracefulReboot<R, H, N> {
        GracefulReboot {
            service: self.service,
        }
    }

    /// Registers `holder`, in the next of the `N` slots, and returns the
    /// handle that holds off graceful reboots for it. The [`Status`] lists its
    /// holds under `holder`.
    ///
    /// # Panics
    ///
    /// If `holder` is registered already, or all `N` slots are taken.
    #[must_use]
    pub fn register_holder(self, holder: H) -> RebootHolder<R, H, N> {
        self.service.register_holder(holder)
    }
}

/// Permission to request graceful reboots only; see [`Reboot::graceful_only`].
pub struct GracefulReboot<R: 'static, H: 'static, const N: usize> {
    service: &'static RebootService<R, H, N>,
}

impl<R, H, const N: usize> GracefulReboot<R, H, N>
where
    R: Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    /// See [`Reboot::graceful`].
    pub fn graceful(self, reason: R) {
        self.service.request_graceful(reason);
    }

    /// See [`Reboot::is_pending`].
    #[must_use]
    pub fn is_pending(self) -> bool {
        self.service.is_pending()
    }
}

/// Requests graceful reboots with reasons of type `S`.
///
/// [`Reboot`] and [`GracefulReboot`] implement it for every `S` their reason
/// type converts from, so a `&dyn RebootRequest<S>` names only `S`.
pub trait RebootRequest<S> {
    /// Requests a graceful reboot for `reason`; see [`Reboot::graceful`].
    fn graceful(&self, reason: S);

    /// Whether a graceful reboot has been requested, for any reason.
    fn is_pending(&self) -> bool;
}

impl<S, R, H, const N: usize> RebootRequest<S> for Reboot<R, H, N>
where
    R: From<S> + Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    fn graceful(&self, reason: S) {
        self.service.request_graceful(R::from(reason));
    }

    fn is_pending(&self) -> bool {
        self.service.is_pending()
    }
}

impl<S, R, H, const N: usize> RebootRequest<S> for GracefulReboot<R, H, N>
where
    R: From<S> + Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    fn graceful(&self, reason: S) {
        self.service.request_graceful(R::from(reason));
    }

    fn is_pending(&self) -> bool {
        self.service.is_pending()
    }
}

/// Permission to hold off graceful reboots only, for one registered holder;
/// see [`Reboot::register_holder`].
pub struct RebootHolder<R: 'static, H: 'static, const N: usize> {
    service: &'static RebootService<R, H, N>,
    slot: usize,
}

impl<R, H: Copy, const N: usize> RebootHolder<R, H, N> {
    /// Holds off a graceful reboot until the returned guard is dropped.
    pub fn hold(self) -> RebootHold<R, H, N> {
        self.service
            .state
            .lock(|state| state.borrow_mut().holds[self.slot] += 1);
        RebootHold {
            service: self.service,
            slot: self.slot,
        }
    }

    /// The holder, as registered.
    #[must_use]
    pub fn holder(self) -> H {
        let holder = self
            .service
            .state
            .lock(|state| state.borrow().holders[self.slot]);
        // Registered before the handle was made, and never removed.
        let Some(holder) = holder else {
            unreachable!("a reboot holder's slot is registered");
        };
        holder
    }
}

/// Holds off a graceful reboot while alive; see [`RebootHolder::hold`].
#[must_use = "the reboot is only held off while the guard is alive"]
pub struct RebootHold<R: 'static, H: 'static, const N: usize> {
    service: &'static RebootService<R, H, N>,
    slot: usize,
}

impl<R, H, const N: usize> Drop for RebootHold<R, H, N> {
    fn drop(&mut self) {
        self.service
            .state
            .lock(|state| state.borrow_mut().holds[self.slot] -= 1);
        self.service.released.signal(());
    }
}

/// The reboot task, which performs graceful reboots; see
/// [`RebootService::start`].
pub struct RebootTask<R: 'static, H: 'static, const N: usize> {
    service: &'static RebootService<R, H, N>,
    wd: WatchdogHandle<RebootWatchdog>,
}

impl<R, H, const N: usize> RebootTask<R, H, N>
where
    R: Copy + fmt::Debug,
    H: Copy + Eq + fmt::Debug,
{
    /// Waits for a graceful reboot request, then for every hold to be
    /// released or the deadline to pass, and resets the chip.
    pub async fn run(self) -> ! {
        let service = self.service;
        loop {
            self.wd.feed();
            if let Either::First(()) =
                select(service.requested.wait(), Timer::after(FEED_INTERVAL)).await
            {
                break;
            }
        }

        // Set before the request is signalled, and never cleared.
        let Some(pending) = service.status().pending else {
            unreachable!("a graceful reboot is pending once requested");
        };
        info!(
            "Reboot requested ({}), waiting up to {} s",
            debug(&pending.reason),
            DEADLINE.as_secs(),
        );
        service.log_holds("Waiting for");

        let released = with_deadline(pending.deadline, async {
            while service.with_state(|state| state.is_held()) {
                service.released.wait().await;
            }
        })
        .await;
        if released.is_err() {
            warn!("Reboot holds not released within {} s", DEADLINE.as_secs());
            service.log_holds("Still held by");
        }
        service.reset(pending.reason, Mode::Graceful)
    }
}

// Handles are `Copy` whatever `R` and `H` are, and print no state.

macro_rules! handle_traits {
    ($($handle:ident),*) => {$(
        impl<R, H, const N: usize> Clone for $handle<R, H, N> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<R, H, const N: usize> Copy for $handle<R, H, N> {}
    )*};
}

handle_traits!(Reboot, GracefulReboot, RebootHolder);

macro_rules! handle_debug {
    ($($handle:ident),*) => {$(
        impl<R, H, const N: usize> fmt::Debug for $handle<R, H, N> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($handle)).finish_non_exhaustive()
            }
        }
    )*};
}

handle_debug!(Reboot, GracefulReboot, RebootHolder, RebootHold, RebootTask);

#[cfg(test)]
mod tests;
