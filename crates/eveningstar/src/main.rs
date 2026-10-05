#![no_std]
#![no_main]

use defmt::{error, info};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::main;
use esp_hal::time::{Duration, Instant};

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    // generator version: 1.4.0
    // generator parameters: -o esp32c6 -o esp32c6-wroom-1 -o stack-smashing-protection -o probe-rs -o defmt -o ci -o claude -o vscode

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO4
    // - GPIO5
    // - GPIO8
    // - GPIO9
    // - GPIO15
    // These GPIO pins are in use by some feature of the module and should not be used.
    // Binding them takes them out of `peripherals` so they cannot be used by accident.
    #[allow(
        clippy::no_effect_underscore_binding,
        reason = "the bindings reserve pins used by the module"
    )]
    let (_gpio24, _gpio25, _gpio26, _gpio27, _gpio28, _gpio29, _gpio30) = (
        peripherals.GPIO24,
        peripherals.GPIO25,
        peripherals.GPIO26,
        peripherals.GPIO27,
        peripherals.GPIO28,
        peripherals.GPIO29,
        peripherals.GPIO30,
    );

    let mut led = Output::new(peripherals.GPIO20, Level::Low, OutputConfig::default());

    loop {
        info!("Hello world!");
        let delay_start = Instant::now();
        while delay_start.elapsed() < Duration::from_millis(500) {}
        led.toggle();
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.2.2/examples
}
