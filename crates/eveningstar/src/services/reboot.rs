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

#![expect(
    clippy::unused_self,
    reason = "a handle's methods take it as proof of permission, not for data"
)]

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};

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
}

/// The kind of a reboot, for the log.
#[derive(Clone, Copy, Debug, defmt::Format)]
enum Mode {
    Graceful,
    #[expect(dead_code, reason = "nothing reboots immediately yet")]
    Immediate,
}

static REQUEST: Signal<CriticalSectionRawMutex, Reason> = Signal::new();
static PENDING: AtomicBool = AtomicBool::new(false);
static HOLDS: AtomicUsize = AtomicUsize::new(0);
static RELEASED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

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
    #[expect(dead_code, reason = "nothing reboots with the full handle yet")]
    pub(crate) fn graceful(self, reason: Reason) {
        request_graceful(reason);
    }

    /// Reboots now, ignoring holds, even while a graceful reboot is waiting
    /// for them.
    #[expect(dead_code, reason = "nothing reboots immediately yet")]
    pub(crate) fn immediate(self, reason: Reason) -> ! {
        reset(reason, Mode::Immediate)
    }

    /// Whether a graceful reboot has been requested.
    #[expect(dead_code, reason = "nothing reboots with the full handle yet")]
    pub(crate) fn is_pending(self) -> bool {
        is_pending()
    }

    /// A handle that can only request graceful reboots.
    pub(crate) fn graceful_only(self) -> GracefulReboot {
        GracefulReboot(())
    }

    /// A handle that can only hold off graceful reboots.
    #[expect(dead_code, reason = "no task holds off reboots yet")]
    pub(crate) fn holder(self) -> RebootHolder {
        RebootHolder(())
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

/// Permission to hold off graceful reboots only; see [`Reboot::holder`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct RebootHolder(());

impl RebootHolder {
    /// Holds off a graceful reboot until the returned guard is dropped.
    #[expect(dead_code, reason = "no task holds off reboots yet")]
    pub(crate) fn hold(self) -> RebootHold {
        HOLDS.fetch_add(1, Ordering::AcqRel);
        RebootHold(())
    }
}

/// Holds off a graceful reboot while alive; see [`RebootHolder::hold`].
#[must_use = "the reboot is only held off while the guard is alive"]
#[derive(Debug)]
#[expect(dead_code, reason = "no task holds off reboots yet")]
pub(crate) struct RebootHold(());

impl Drop for RebootHold {
    fn drop(&mut self) {
        HOLDS.fetch_sub(1, Ordering::AcqRel);
        RELEASED.signal(());
    }
}

/// Signals the reboot task, unless a graceful reboot is already pending.
fn request_graceful(reason: Reason) {
    if !PENDING.swap(true, Ordering::AcqRel) {
        REQUEST.signal(reason);
    }
}

fn is_pending() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// Waits for a graceful reboot request, then for every hold to be released or
/// [`DEADLINE`] to pass, and resets the chip.
#[embassy_executor::task]
async fn reboot_task(wd: WatchdogHandle<wd::Reboot>) -> ! {
    let reason = loop {
        wd.feed();
        if let Either::First(reason) = select(REQUEST.wait(), Timer::after(FEED_INTERVAL)).await {
            break reason;
        }
    };

    info!(
        "Reboot requested ({}), waiting up to {} s for {} holds",
        reason,
        DEADLINE.as_secs(),
        HOLDS.load(Ordering::Acquire)
    );
    let released = with_timeout(DEADLINE, async {
        while HOLDS.load(Ordering::Acquire) > 0 {
            RELEASED.wait().await;
        }
    })
    .await;
    if released.is_err() {
        warn!("Reboot holds not released within {} s", DEADLINE.as_secs());
    }
    reset(reason, Mode::Graceful)
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
