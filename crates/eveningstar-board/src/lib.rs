//! Board support for Eveningstar.
//!
//! [`Board::new`] consumes the chip's peripherals and hands out pins named by
//! their role on the board. All knowledge of the board revision lives in this
//! crate; the revision is selected with a cargo feature.

#![no_std]

#[cfg(not(any(feature = "v2")))]
compile_error!("no board revision selected; enable one of the features: `v2`");

#[cfg(feature = "v2")]
mod v2;

use esp_hal::gpio::AnyPin;
use esp_hal::peripherals::{FROM_CPU_INTR0, TIMG0};

/// The board's pins and peripherals, named by role.
#[derive(Debug)]
pub struct Board {
    /// Status LED, active high.
    pub status_led: AnyPin<'static>,
    /// Timer group 0, used by the RTOS scheduler.
    pub timg0: TIMG0<'static>,
    /// Software interrupt 0, used by the RTOS scheduler.
    pub from_cpu_intr0: FROM_CPU_INTR0<'static>,
}
