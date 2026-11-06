//! The reboot service ([`eveningstar_system::reboot`]) with the firmware's
//! reasons and holders.
//!
//! [`start`] spawns the reboot task and returns the one [`Reboot`]; the other
//! handles derive from it, as `eveningstar_system::reboot` documents. Each
//! [`Holder`] is registered once.

use embassy_executor::Spawner;
use eveningstar_system::Reset;
use eveningstar_system::reboot::{self, RebootService, RebootTask, RebootWatchdog};
use eveningstar_system::watchdog::WatchdogHandle;

/// Why a reboot was requested.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Reason {
    /// The Ethernet link was down, or had no IPv4 address, for too long.
    LinkDown,
    /// A client requested it through the API.
    Api,
    /// The running image wasn't confirmed `Valid` within the confirmation
    /// window after boot.
    Unconfirmed,
}

/// What holds off a graceful reboot, as the [`Status`](reboot::Status) lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Holder {
    /// The HTTP server, while it answers a reboot request or serves a
    /// download.
    Http,
}

impl Holder {
    /// Every holder.
    const ALL: [Self; 1] = [Self::Http];
}

/// Number of [`Holder`]s, and of the service's holder slots.
pub(crate) const HOLDERS: usize = Holder::ALL.len();

/// Permission to request graceful and immediate reboots.
pub(crate) type Reboot = reboot::Reboot<Reason, Holder, HOLDERS>;

/// Permission to request graceful reboots only.
pub(crate) type GracefulReboot = reboot::GracefulReboot<Reason, Holder, HOLDERS>;

/// Permission to hold off graceful reboots only, for one [`Holder`].
pub(crate) type RebootHolder = reboot::RebootHolder<Reason, Holder, HOLDERS>;

/// A hold off graceful reboots, alive until dropped.
pub(crate) type RebootHold = reboot::RebootHold<Reason, Holder, HOLDERS>;

static SERVICE: RebootService<Reason, Holder, HOLDERS> = RebootService::new();

/// Spawns the reboot task, which feeds `wd`, and returns the handle that
/// requests reboots. The service resets the chip through `reset`.
///
/// # Panics
///
/// If called more than once: the service starts once per boot, so one
/// [`Reboot`] exists.
pub(crate) fn start(
    spawner: Spawner,
    reset: &'static dyn Reset,
    wd: WatchdogHandle<RebootWatchdog>,
) -> Reboot {
    let (reboot, task) = SERVICE.start(reset, wd);
    spawner.spawn(defmt::unwrap!(reboot_task(task)));
    reboot
}

#[embassy_executor::task]
async fn reboot_task(task: RebootTask<Reason, Holder, HOLDERS>) -> ! {
    task.run().await
}
