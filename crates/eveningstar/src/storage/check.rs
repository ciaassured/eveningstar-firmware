//! The app check task: [`App::run`] over the slots of the tier, in the
//! background of everything else.
//!
//! The checks hash with the SHA accelerator ([`crate::sha`]) and pause for
//! [`PAUSE`] after every KiB hashed. The Ethernet controller's driver yields
//! after every SPI transaction, so a job that holds the executor for
//! milliseconds at a time takes it from the network; with the pause, HTTP
//! requests keep their pace while a slot is checked.
//!
//! The task has its own watchdog handle, fed after every KiB hashed, never on
//! a timer, so a check that hangs while the rest of the system runs is
//! caught. The handle is paused while the checks wait for a slot to change.
//! Before each wait, the results kept since the last one are logged.

use core::future::Future;

use defmt::info;
use embassy_time::{Duration, Instant, Timer};
use eveningstar_boot::{App, Progress};
use eveningstar_esp_formats::pace::Pace;
use eveningstar_storage::{Epoch, Slot};

use super::log;
use crate::sha::Sha;
use crate::wd;
use eveningstar_system::watchdog::WatchdogHandle;

/// Task watchdog timeout of the check task. A check feeds after every KiB
/// hashed, many times a second, so this only has to outlast the
/// supervisor's polls.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a check pauses after each KiB hashed, so the tasks that yield
/// often, such as the Ethernet controller's driver after each SPI
/// transaction, get the executor to themselves for a while.
const PAUSE: Duration = Duration::from_millis(4);

/// Runs the checks of `app`'s slots for good, hashing with `sha`, with `wd`
/// fed while they hash and paused while they wait. `running` is the slot
/// that booted, in the layout and boot tiers.
#[embassy_executor::task]
pub(super) async fn task(
    app: App<'static, 'static>,
    running: Option<Slot>,
    sha: Sha,
    wd: WatchdogHandle<wd::AppCheck>,
) -> ! {
    let mut progress = Supervised {
        wd,
        app,
        running,
        logged: Logged::default(),
    };
    app.run(sha.sha256(), &mut Background, &mut progress).await
}

/// Paces a check as a background job: a [`PAUSE`] after each KiB hashed.
struct Background;

impl Pace for Background {
    fn pause(&mut self) -> impl Future<Output = ()> {
        Timer::after(PAUSE)
    }
}

/// The epochs whose results were logged last, per slot.
#[derive(Default)]
struct Logged {
    current: Option<Epoch>,
    alternate: Option<Epoch>,
    found: Option<Epoch>,
}

/// The checks' progress hook: feeds the task's watchdog after each KiB
/// hashed, and before each wait logs the results kept since the last one
/// and pauses the watchdog.
struct Supervised {
    wd: WatchdogHandle<wd::AppCheck>,
    app: App<'static, 'static>,
    running: Option<Slot>,
    logged: Logged,
}

impl Supervised {
    /// Logs each slot's result kept for its epoch now, unless it was logged
    /// already, and the time when any was.
    fn log_new(&mut self) {
        let logged = &mut self.logged;
        let mut any = false;
        if let Some(current) = self.app.current() {
            any |= log::app_slot(current, "current", self.running, &mut logged.current);
        }
        if let Some(alternate) = self.app.alternate() {
            let slot = self.running.map(Slot::other);
            any |= log::app_slot(alternate, "alternate", slot, &mut logged.alternate);
        }
        if let Some(found) = self.app.found_app() {
            any |= log::app_slot(found, "found", None, &mut logged.found);
        }
        if any {
            info!(
                "App checks done at {=u64} ms after boot",
                Instant::now().as_millis()
            );
        }
    }
}

impl Progress for Supervised {
    fn advance(&mut self) {
        self.wd.feed();
    }

    fn idle(&mut self) {
        self.log_new();
        self.wd.pause();
    }
}
