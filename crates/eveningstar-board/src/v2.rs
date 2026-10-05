//! Board revision v2, built around the ESP32-C6-WROOM-1 module.
//!
//! Pins not handed out by [`Board::new`] are consumed and unreachable. That
//! includes GPIO24-30, which the module wires to its internal SPI flash;
//! reconfiguring any of them breaks code execution from flash.
//!
//! Strapping pins are sampled at reset and must not be pulled to the wrong
//! level by anything attached to them while the chip boots:
//! - GPIO4, GPIO5: SDIO timing, also JTAG MTMS/MTDI
//! - GPIO8, GPIO9: boot mode (GPIO9 low at reset enters download mode)
//! - GPIO15: JTAG source select

use esp_hal::peripherals::Peripherals;

use crate::Board;

impl Board {
    /// Takes ownership of the chip's peripherals and splits them by role.
    #[must_use]
    pub fn new(peripherals: Peripherals) -> Self {
        let Peripherals { GPIO20, .. } = peripherals;

        Self {
            status_led: GPIO20.into(),
        }
    }
}
