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

/// The board's pins and peripherals, named by role.
#[derive(Debug)]
pub struct Board {
    /// Status LED, active high.
    pub status_led: AnyPin<'static>,
}
