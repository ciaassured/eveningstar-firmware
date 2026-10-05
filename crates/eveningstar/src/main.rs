#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{AnyPin, Level, Output, OutputConfig};
use esp_hal::timer::timg::TimerGroup;
use eveningstar_board::Board;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    loop {}
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const BLINK_PERIOD: Duration = Duration::from_millis(500);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // generator version: 1.4.0
    // generator parameters: -o esp32c6 -o esp32c6-wroom-1 -o stack-smashing-protection -o probe-rs -o defmt -o ci -o claude -o vscode

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let board = Board::new(esp_hal::init(config));

    let timg0 = TimerGroup::new(board.timg0);
    esp_rtos::start(timg0.timer0, board.from_cpu_intr0);
    info!("Embassy initialized");

    spawner.spawn(defmt::unwrap!(blink(board.status_led)));
}

#[embassy_executor::task]
async fn blink(pin: AnyPin<'static>) -> ! {
    let mut led = Output::new(pin, Level::Low, OutputConfig::default());
    let mut ticker = Ticker::every(BLINK_PERIOD);

    loop {
        led.toggle();
        ticker.next().await;
    }
}
