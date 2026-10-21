//! Requested reboots, for resets that aren't caused by a fault.
//!
//! Faults (a panic, a missed task watchdog deadline, a hung executor) reset the
//! chip at once, because the system's state can't be trusted. Other causes,
//! such as the Ethernet link staying down or a fresh OTA update being ready,
//! [`request`] a reboot instead, with a [`Reason`] and a [`Mode`].
//!
//! A graceful reboot waits until no [`RebootHold`] is alive, so a task can hold
//! it off while it has work it must finish, such as flushing logged data. The
//! wait is bounded: after [`DEADLINE`] the chip resets anyway, and if the reboot
//! task itself stops, its task watchdog resets the chip after
//! [`WATCHDOG_TIMEOUT`]. An immediate reboot ignores holds and resets the chip
//! during the request.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};

use crate::watchdog::WatchdogHandle;
use crate::wd::Reboot;

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

/// How a requested reboot treats work in progress.
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(crate) enum Mode {
    /// Waits, in the reboot task, until no [`RebootHold`] is alive or
    /// [`DEADLINE`] has passed.
    Graceful,
    /// Resets the chip during the request, ignoring holds.
    #[expect(dead_code, reason = "nothing reboots immediately yet")]
    Immediate,
}

static REQUEST: Signal<CriticalSectionRawMutex, Reason> = Signal::new();
static PENDING: AtomicBool = AtomicBool::new(false);
static HOLDS: AtomicUsize = AtomicUsize::new(0);
static RELEASED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Requests a reboot.
///
/// A graceful request returns, and the reboot task reboots later; only the
/// first graceful request takes effect. An immediate request does not return,
/// even while a graceful reboot is waiting for holds.
pub(crate) fn request(reason: Reason, mode: Mode) {
    match mode {
        Mode::Graceful => {
            if !PENDING.swap(true, Ordering::AcqRel) {
                REQUEST.signal(reason);
            }
        }
        Mode::Immediate => reset(reason, mode),
    }
}

/// Whether a graceful reboot has been requested.
pub(crate) fn is_pending() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// Holds off a graceful reboot until the returned guard is dropped.
#[expect(dead_code, reason = "no task holds off reboots yet")]
pub(crate) fn hold() -> RebootHold {
    HOLDS.fetch_add(1, Ordering::AcqRel);
    RebootHold(())
}

/// Holds off a graceful reboot while alive; see [`hold`].
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

/// Waits for a graceful reboot request, then for every hold to be released or
/// [`DEADLINE`] to pass, and resets the chip.
#[embassy_executor::task]
pub(crate) async fn reboot_task(wd: WatchdogHandle<Reboot>) -> ! {
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
