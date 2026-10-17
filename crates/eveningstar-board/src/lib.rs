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

// Types whose definition depends on how the selected revision wires the board.
#[cfg(feature = "v2")]
pub use v2::{EthernetSpi, FlashCs, FlashSpi};

use core::fmt;

use esp_hal::gpio::{AnyPin, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::peripherals::{FROM_CPU_INTR0, RTC_TIMER, TIMG0};

#[derive(Debug)]
pub struct Board {
    pub status_led: StatusLed,
    pub cfg_button: CfgButton,
    pub ethernet: Ethernet,
    pub flash: Flash,
    pub timg0: TIMG0<'static>,
    pub rtc_timer: RTC_TIMER<'static>,
    pub from_cpu_intr0: FROM_CPU_INTR0<'static>,
}

/// W5500 Ethernet controller (MAC + PHY), with its SPI device and control
/// lines configured.
pub struct Ethernet {
    /// SPI device, mode 0.
    pub spi: EthernetSpi,
    /// Interrupt request, active low.
    pub int: Input<'static>,
    /// Hardware reset, active low. Starts asserted (low), holding the W5500 in
    /// reset.
    pub reset: Output<'static>,
    /// MAC address assigned to the Ethernet interface. The W5500 has none of its
    /// own.
    pub mac: [u8; 6],
}

impl fmt::Debug for Ethernet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The revision's SPI device type need not implement `Debug`.
        f.debug_struct("Ethernet")
            .field("int", &self.int)
            .field("reset", &self.reset)
            .field("mac", &self.mac)
            .finish_non_exhaustive()
    }
}

/// External SPI NAND flash. The part is documented by the board revision.
pub struct Flash {
    /// SPI device, mode 0.
    pub spi: FlashSpi,
}

impl fmt::Debug for Flash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The revision's SPI device type need not implement `Debug`.
        f.debug_struct("Flash").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct StatusLed {
    pin: Output<'static>,
    active_level: Level
}

impl StatusLed {
    #[must_use]
    pub fn new(pin: AnyPin<'static>, active_level: Level) -> Self {
        Self { 
            pin: Output::new(pin, !active_level, OutputConfig::default()),
            active_level
        }
    }

    pub fn set_on(&mut self) { self.pin.set_level(self.active_level); }
    pub fn set_off(&mut self) { self.pin.set_level(!self.active_level); }
    pub fn toggle(&mut self) { self.pin.toggle(); }
}

#[derive(Debug)]
pub struct CfgButton {
    pin: Input<'static>,
    pressed_level: Level,
}

impl CfgButton {
    #[must_use]
    pub fn new(pin: AnyPin<'static>, pressed_level: Level, pull: Pull) -> Self {
        Self {
            pin: Input::new(pin, InputConfig::default().with_pull(pull)),
            pressed_level
        }
    }

    pub fn is_pressed(&self) -> bool {
        self.pin.level() == self.pressed_level
    }
}