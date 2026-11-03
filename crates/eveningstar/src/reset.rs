//! The chip's software reset, as the [`Reset`] that the reboot service and the
//! task watchdog supervisor take.
//!
//! [`take`] hands it out once per boot. Besides the panic handler, it is the
//! firmware's only call of `esp_hal::system::software_reset`.

use eveningstar_system::{Reset, ResetCause};
use static_cell::StaticCell;

/// The software reset; [`take`] makes the one value.
pub(crate) struct SoftwareReset(());

impl Reset for SoftwareReset {
    #[expect(
        clippy::disallowed_methods,
        reason = "the reboot service and the task watchdog supervisor reset through this"
    )]
    fn reset(&self, _cause: ResetCause) -> ! {
        esp_hal::system::software_reset()
    }
}

/// The software reset.
///
/// # Panics
///
/// If taken before.
pub(crate) fn take() -> &'static SoftwareReset {
    static RESET: StaticCell<SoftwareReset> = StaticCell::new();
    RESET.init(SoftwareReset(()))
}
