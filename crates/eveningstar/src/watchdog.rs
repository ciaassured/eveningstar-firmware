//! The task watchdog supervisor ([`eveningstar_system::watchdog`]) on the
//! ESP32-C6.
//!
//! The supervisor's main watchdog is MWDT0 (`TIMG0`), which resets the digital
//! core, and its backstop is RWDT, in the LP domain, which resets the whole
//! system:
//!
//! | Layer            | Catches                              | Action                   |
//! | ---------------- | ------------------------------------ | ------------------------ |
//! | `Supervisor`     | a task that stops feeding its handle | log, then software reset |
//! | MWDT0 (`TIMG0`)  | the supervisor or executor hanging   | digital core reset       |
//! | RWDT (LP domain) | the MWDT path failing (e.g. clocks)  | full system reset        |
//!
//! [`supervisor`] consumes the `Rwdt` and `Wdt<TIMG0>`, which the HAL hands out
//! once, so one supervisor exists. The supervised tasks are in `crate::wd`.

use esp_hal::peripherals::TIMG0;
use esp_hal::rtc_cntl::{Rwdt, RwdtStage, RwdtStageAction};
use esp_hal::timer::timg::{MwdtStage, MwdtStageAction, Wdt};
use eveningstar_system::Reset;
use eveningstar_system::watchdog::{self, HardwareWatchdogs, Mailboxes};
use static_cell::ConstStaticCell;

use crate::wd;

/// The supervisor, with a slot per supervised task.
pub(crate) type Supervisor = watchdog::Supervisor<{ wd::SLOTS }, Watchdogs>;

/// MWDT0, the main watchdog, and RWDT, the backstop.
pub(crate) struct Watchdogs {
    timer0: Wdt<TIMG0<'static>>,
    rtc: Rwdt,
}

impl HardwareWatchdogs for Watchdogs {
    fn enable(&mut self, main: embassy_time::Duration, backstop: embassy_time::Duration) {
        self.timer0.enable();
        self.timer0
            .set_stage_action(MwdtStage::Stage0, MwdtStageAction::ResetSystem);
        self.timer0
            .set_timeout(MwdtStage::Stage0, hal_duration(main));
        self.timer0.feed();

        self.rtc.enable();
        self.rtc
            .set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
        self.rtc
            .set_timeout(RwdtStage::Stage0, hal_duration(backstop));
        self.rtc.feed();
    }

    fn feed(&mut self) {
        self.timer0.feed();
        self.rtc.feed();
    }
}

/// `duration` as the HAL's duration.
fn hal_duration(duration: embassy_time::Duration) -> esp_hal::time::Duration {
    esp_hal::time::Duration::from_micros(duration.as_micros())
}

/// Builds the supervisor, which enables the hardware watchdogs: from here the
/// chip resets unless [`supervise`] runs within
/// [`MAIN_TIMEOUT`](watchdog::MAIN_TIMEOUT). A missed deadline resets the chip
/// through `reset`.
pub(crate) fn supervisor(
    rwdt: Rwdt,
    wdt: Wdt<TIMG0<'static>>,
    reset: &'static dyn Reset,
) -> Supervisor {
    static MAILBOXES: ConstStaticCell<Mailboxes<{ wd::SLOTS }>> =
        ConstStaticCell::new(Mailboxes::new());
    let hardware = Watchdogs {
        timer0: wdt,
        rtc: rwdt,
    };
    Supervisor::new(MAILBOXES.take(), hardware, reset)
}

/// Runs the supervisor.
#[embassy_executor::task]
pub(crate) async fn supervise(supervisor: Supervisor) -> ! {
    supervisor.run().await
}
