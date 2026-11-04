//! The confirmation window: the time from boot the running image has to be
//! confirmed `Valid` before the board reboots.
//!
//! At boot, [`start`] reads the running image's boot state from the
//! otadata's module: the compiled layout's in the layout and boot tiers, the
//! located table's in the unverified tier. An image confirmed `Valid` opens
//! no window. Any other awaits a confirmation ([`awaits_confirmation`]), and
//! so does one with no boot state: no otadata, one that doesn't read, or an
//! unknown running slot, as in the unverified tier. For those, [`start`]
//! warns and spawns [`task`], which waits until [`WINDOW`] after boot, reads
//! the boot state again and requests a graceful reboot
//! ([`Reason::Unconfirmed`]) unless the image is `Valid` by then, as
//! [`Ota::confirm`](eveningstar_ota::Ota::confirm) makes it.
//!
//! A rollback bootloader turns a `PendingVerify` image `Aborted` at the
//! reboot and starts the previous image. Any other image that awaits a
//! confirmation boots again, so the board reboots once per window until an
//! image is confirmed.
//!
//! The task's watchdog handle sleeps through the window, and its timeout
//! bounds the read and the request after it. The handle retires once the
//! task is done, and at boot when no window opens.

use defmt::{Debug2Format, Format, Formatter, info, warn};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use eveningstar_boot::otadata::awaits_confirmation;
use eveningstar_boot::{LoadError, Otadata};
use eveningstar_storage::Slot;
use eveningstar_storage::otadata::BootState;

use super::log::Fmt;
use crate::services::reboot::{GracefulReboot, Reason};
use crate::watchdog::WatchdogHandle;
use crate::wd;

/// How long from boot the running image has to be confirmed by default:
/// 10 minutes.
const DEFAULT_WINDOW: Duration = Duration::from_secs(10 * 60);

/// How long from boot the running image has to be confirmed: the
/// [`DEFAULT_WINDOW`], or, as a test aid, the number of seconds in
/// `EVENINGSTAR_CONFIRM_WINDOW_S` when the firmware is built. A value that
/// isn't a decimal number above 0 fails the build.
pub(super) const WINDOW: Duration = match option_env!("EVENINGSTAR_CONFIRM_WINDOW_S") {
    Some(secs) => Duration::from_secs(secs_of(secs)),
    None => DEFAULT_WINDOW,
};

/// Task watchdog timeout of the confirmation task, once its window has
/// passed: the read of the boot state and the request take well under it.
pub(crate) const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

/// How many times the task reads the otadata while a write session holds it
/// or a write changes it during the read.
const READ_ATTEMPTS: u32 = 10;

/// The wait between those reads.
const READ_RETRY: Duration = Duration::from_millis(100);

/// The seconds `text` gives in decimal.
///
/// # Panics
///
/// When `text` isn't a decimal number above 0, at compile time for a
/// constant.
const fn secs_of(text: &str) -> u64 {
    match u64::from_str_radix(text, 10) {
        Ok(secs) if secs > 0 => secs,
        _ => panic!("EVENINGSTAR_CONFIRM_WINDOW_S isn't a decimal number of seconds above 0"),
    }
}

/// Reads the boot state of the image running from `slot` through `otadata`,
/// the otadata's module, and spawns the confirmation task on `spawner`, with
/// `reboot` and supervised by `wd`, unless the image is confirmed `Valid`.
pub(super) fn start(
    spawner: Spawner,
    otadata: Option<&'static Otadata<'static>>,
    slot: Option<Slot>,
    reboot: GracefulReboot,
    wd: WatchdogHandle<wd::Confirmation>,
) {
    let found = read(otadata, slot);
    if !awaits_confirmation(found.boot_state()) {
        wd.retire();
        return;
    }
    warn!(
        "Running image not confirmed ({}): rebooting {=u64} s after boot unless it is confirmed",
        found,
        WINDOW.as_secs(),
    );
    spawner.spawn(defmt::unwrap!(task(otadata, slot, reboot, wd)));
}

/// Waits until [`WINDOW`] after boot, then requests a graceful reboot
/// through `reboot` unless the image running from `slot` is confirmed
/// `Valid` by then, as `otadata`, the otadata's module, gives its boot
/// state. `wd` sleeps through the wait and retires at the end.
#[embassy_executor::task]
async fn task(
    otadata: Option<&'static Otadata<'static>>,
    slot: Option<Slot>,
    reboot: GracefulReboot,
    wd: WatchdogHandle<wd::Confirmation>,
) {
    let end = Instant::MIN + WINDOW;
    wd.sleep(end.saturating_duration_since(Instant::now()) + WATCHDOG_TIMEOUT);
    Timer::at(end).await;
    wd.feed();

    let found = settled(otadata, slot).await;
    if awaits_confirmation(found.boot_state()) {
        warn!(
            "Running image not confirmed within {=u64} s of boot ({}): requesting a reboot",
            WINDOW.as_secs(),
            found,
        );
        reboot.graceful(Reason::Unconfirmed);
    } else {
        info!("Running image confirmed ({}): no reboot", found);
    }
    wd.retire();
}

/// The boot state as [`read`] gives it, read again after a [`READ_RETRY`]
/// while a write session holds the otadata or a write changes it during the
/// read, up to [`READ_ATTEMPTS`] reads in all.
async fn settled(otadata: Option<&Otadata<'_>>, slot: Option<Slot>) -> Found {
    let mut attempts = 1;
    loop {
        let found = read(otadata, slot);
        match found {
            Found::Unread(LoadError::Busy | LoadError::Stale) if attempts < READ_ATTEMPTS => {
                attempts += 1;
                Timer::after(READ_RETRY).await;
            }
            _ => return found,
        }
    }
}

/// The boot state of the image running from `slot`, as `otadata`, the
/// otadata's module, gives it now, or why there is none.
fn read(otadata: Option<&Otadata<'_>>, slot: Option<Slot>) -> Found {
    let Some(otadata) = otadata else {
        return Found::NoOtadata;
    };
    let Some(slot) = slot else {
        return Found::NoSlot;
    };
    match otadata.parse() {
        Ok(model) => Found::Read(model.boot_state(slot)),
        Err(err) => Found::Unread(err),
    }
}

/// The running image's boot state as read, or why there is none.
#[derive(Clone, Copy)]
enum Found {
    /// The chain found no otadata.
    NoOtadata,
    /// The slot that booted is unknown, as in the unverified tier.
    NoSlot,
    /// The otadata didn't read.
    Unread(LoadError),
    /// The boot state.
    Read(BootState),
}

impl Found {
    /// The boot state, when one was read.
    const fn boot_state(&self) -> Option<&BootState> {
        match self {
            Self::Read(boot) => Some(boot),
            Self::NoOtadata | Self::NoSlot | Self::Unread(_) => None,
        }
    }
}

impl Format for Found {
    fn format(&self, f: Formatter<'_>) {
        match self {
            Self::NoOtadata => defmt::write!(f, "no otadata"),
            Self::NoSlot => defmt::write!(f, "running slot unknown"),
            Self::Unread(err) => defmt::write!(f, "otadata not read: {}", Debug2Format(err)),
            Self::Read(boot) => {
                if let Some(state) = boot.state {
                    defmt::write!(f, "state {}", Fmt(state));
                } else {
                    defmt::write!(f, "no state");
                }
                match boot.selected {
                    Some(selected) if selected != boot.running => defmt::write!(
                        f,
                        ", {} running, the otadata selects {}",
                        Fmt(boot.running),
                        Fmt(selected),
                    ),
                    Some(_) => {}
                    None => defmt::write!(f, ", no entry valid"),
                }
            }
        }
    }
}
