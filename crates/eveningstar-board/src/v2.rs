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
//!
//! The W5500 Ethernet controller and the external flash share SPI2: MOSI
//! (GPIO7) and MISO (GPIO2) are common, and each has its own SCK and chip
//! select (W5500: GPIO6, GPIO18; flash: GPIO3, GPIO19). See [`FlashCs`] for how
//! the flash's separate SCK is driven.
//!
//! The external flash is a `GD5F2GM7UE` SPI NAND.

use core::convert::Infallible;

use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_hal::Async;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, OutputSignal, Pull};
use esp_hal::peripherals::Peripherals;
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use static_cell::StaticCell;

use crate::{Board, CfgButton, Ethernet, Flash, StatusLed};

/// The SPI bus shared by the Ethernet controller and the external flash.
type SpiBus = Mutex<CriticalSectionRawMutex, Spi<'static, Async>>;

/// SPI device for the Ethernet controller, on the bus it shares with the flash.
pub type EthernetSpi = SpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, Output<'static>>;

/// SPI device for the external flash, on the bus it shares with the Ethernet
/// controller.
pub type FlashSpi = SpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, FlashCs>;

const SPI_FREQUENCY: Rate = Rate::from_mhz(20);

/// Chip select for the external flash that also gates the flash's SCK line.
///
/// The flash shares SPI2 with the Ethernet controller but has its own SCK pin,
/// and its datasheet forbids toggling SCLK while CS# is high ("While CS# is
/// HIGH, keep SCLK at VCC or GND"). So the SPI clock is routed to the flash's SCK
/// pin only while the flash is selected; the rest of the time the pin is a GPIO
/// held low, the mode 0 idle level.
///
/// The shared-bus [`SpiDevice`] asserts CS after locking the bus and deasserts it
/// before unlocking, including when a transaction is cancelled, so the clock is
/// only ever switched while the bus is idle.
#[derive(Debug)]
pub struct FlashCs {
    cs: Output<'static>,
    sck: Output<'static>,
}

impl embedded_hal::digital::ErrorType for FlashCs {
    type Error = Infallible;
}

impl embedded_hal::digital::OutputPin for FlashCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        OutputSignal::FSPICLK.connect_to(&self.sck);
        self.cs.set_low();
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.cs.set_high();
        // Hands the pin back to its GPIO output, which is held low.
        OutputSignal::FSPICLK.disconnect_from(&self.sck);
        Ok(())
    }
}

impl Board {
    /// Takes ownership of the chip's peripherals and splits them by role.
    ///
    /// # Panics
    ///
    /// Panics if called more than once, or if the HAL rejects the board's
    /// fixed SPI configuration, which would be a bug in this crate.
    #[must_use]
    pub fn new(peripherals: Peripherals) -> Self {
        static SPI_BUS: StaticCell<SpiBus> = StaticCell::new();

        // Ethernet uses the base MAC address + 3, following ESP-IDF's MAC address
        // allocation. See:
        // https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-reference/system/misc_system_api.html#mac-address-allocation
        let mut mac = [0u8; 6];
        mac.copy_from_slice(esp_hal::efuse::base_mac_address().as_bytes());
        mac[5] = mac[5].wrapping_add(3);

        let spi = Spi::new(
            peripherals.SPI2,
            SpiConfig::default()
                .with_mode(Mode::_0)
                .with_frequency(SPI_FREQUENCY),
        )
        .expect("SPI configuration is valid")
        .with_sck(peripherals.GPIO6)
        .with_mosi(peripherals.GPIO7)
        .with_miso(peripherals.GPIO2)
        .into_async();

        let spi_bus = SPI_BUS.init(SpiBus::new(spi));

        let ethernet = Ethernet {
            spi: EthernetSpi::new(
                spi_bus,
                Output::new(peripherals.GPIO18, Level::High, OutputConfig::default()),
            ),
            // The board has no external pull-up on INTn.
            int: Input::new(peripherals.GPIO10, InputConfig::default().with_pull(Pull::Up)),
            // Starts low, holding the W5500 in reset.
            reset: Output::new(peripherals.GPIO11, Level::Low, OutputConfig::default()),
            mac,
        };

        let flash = Flash {
            spi: FlashSpi::new(
                spi_bus,
                FlashCs {
                    cs: Output::new(peripherals.GPIO19, Level::High, OutputConfig::default()),
                    sck: Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default()),
                },
            ),
        };

        Self {
            status_led: StatusLed::new(peripherals.GPIO20.into(), Level::High),
            cfg_button: CfgButton::new(peripherals.GPIO21.into(), Level::Low, Pull::Up),
            ethernet,
            flash,
            timg0: peripherals.TIMG0,
            rtc_timer: peripherals.RTC_TIMER,
            from_cpu_intr0: peripherals.FROM_CPU_INTR0,
        }
    }
}
