//! Graceful reboots, for resets that aren't caused by a fault.
//!
//! Faults (a panic, a missed task watchdog deadline, a hung executor) reset the
//! chip immediately, because the system's state can't be trusted. Other causes,
//! such as the Ethernet link staying down, or a fresh OTA update ready [request](request) a reboot instead.
//!
//! A requested reboot waits until no [`RebootHold`] is alive, so a task can hold
//! it off while it has work it must finish, such as flushing logged data. The
//! wait is bounded: after [`DEADLINE`] the chip resets anyway, and if the reboot
//! task itself stops, its task watchdog resets the chip after
//! [`WATCHDOG_TIMEOUT`].

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};

use crate::watchdog::WatchdogHandle;
use crate::wd::Reboot;

/// Longest a requested reboot waits for holds to be released.
pub(crate) const DEADLINE: Duration = Duration::from_secs(5 * 60);

/// Task watchdog timeout of the reboot task. The task stops feeding when a
/// reboot is requested, so this bounds the reboot if the task itself stops.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(6 * 60);

/// How often the reboot task feeds its watchdog while no reboot is requested.
const FEED_INTERVAL: Duration = Duration::from_secs(30);

/// Why a graceful reboot was requested.
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(crate) enum Reason {
    /// The Ethernet link was down, or had no IPv4 address, for too long.
    LinkDown,
}

static REQUEST: Signal<CriticalSectionRawMutex, Reason> = Signal::new();
static PENDING: AtomicBool = AtomicBool::new(false);
static HOLDS: AtomicUsize = AtomicUsize::new(0);
static RELEASED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Requests a graceful reboot. Only the first request takes effect.
pub(crate) fn request(reason: Reason) {
    if !PENDING.swap(true, Ordering::AcqRel) {
        REQUEST.signal(reason);
    }
}

/// Whether a graceful reboot has been requested.
pub(crate) fn is_pending() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// Holds off a requested reboot until the returned guard is dropped.
#[expect(dead_code, reason = "no task holds off reboots yet")]
pub(crate) fn hold() -> RebootHold {
    HOLDS.fetch_add(1, Ordering::AcqRel);
    RebootHold(())
}

/// Holds off a requested reboot while alive; see [`hold`].
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

/// Waits for a reboot request, then for every hold to be released or
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
    if released.is_ok() {
        info!("Rebooting ({})", reason);
    } else {
        warn!(
            "Reboot holds not released within {} s, rebooting ({})",
            DEADLINE.as_secs(),
            reason
        );
    }
    esp_hal::system::software_reset()
}
