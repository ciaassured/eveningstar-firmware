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

use esp_hal::gpio::{AnyPin, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::peripherals::{FROM_CPU_INTR0, TIMG0};

#[derive(Debug)]
pub struct Board {
    pub status_led: StatusLed,
    pub cfg_button: CfgButton,
    pub timg0: TIMG0<'static>,
    pub from_cpu_intr0: FROM_CPU_INTR0<'static>,
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