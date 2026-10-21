#![no_std]
#![no_main]

mod ethernet;
mod identity;
mod services;
mod shared_spi;
mod watchdog;

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{ADC1, RNG};
use esp_hal::rng::{Trng, TrngSource};
use esp_hal::timer::timg::TimerGroup;
use eveningstar_api::DeviceId;
use eveningstar_board::{Board, CfgButton, StatusLed};

use crate::services::reboot;
use crate::watchdog::WatchdogHandle;
use crate::wd::Blinky;

#[panic_handler]
#[expect(
    clippy::disallowed_methods,
    reason = "a panic resets at once, as the system's state can't be trusted"
)]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    // Gives the log time to reach the host. A spin rather than a timer, because
    // a panic can come before the timers run.
    for _ in 0..PANIC_LOG_SPINS {
        core::hint::spin_loop();
    }
    esp_hal::system::software_reset()
}

/// Busy-wait iterations between logging a panic and resetting: a fraction of a
/// second at full CPU clock, longer if the panic comes before it is raised.
const PANIC_LOG_SPINS: u32 = 10_000_000;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

mod wd {
    use crate::watchdog;
    use embassy_time::Duration;

    watchdog::watchdog! {
        blinky: Blinky = Duration::from_millis(5000),
        // Tolerates two missed probes.
        ethernet: EthernetProbe = Duration::from_ticks(crate::ethernet::PROBE_INTERVAL.as_ticks() * 3),
        // Tolerates two missed checks.
        link: LinkMonitor = Duration::from_ticks(crate::ethernet::LINK_CHECK_INTERVAL.as_ticks() * 3),
        reboot: Reboot = crate::services::reboot::WATCHDOG_TIMEOUT,
    }
}

const BLINK_PERIOD: Duration = Duration::from_millis(1000);

/// Fast flashes shown once when the blink task starts.
const BOOT_FLASHES: u32 = 10;
const BOOT_FLASH_PERIOD: Duration = Duration::from_millis(300);

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
    let mut board = Board::new(esp_hal::init(config));
    info!(
        "reset reason: {}",
        defmt::Debug2Format(&esp_hal::system::reset_reason())
    );
    let device_id = DeviceId::from_mac(board.base_mac);
    let device_name = identity::name(device_id);
    info!(
        "device {} (ID {})",
        device_name.as_str(),
        device_id.encode().as_str()
    );

    let timg0 = TimerGroup::new(board.timg0);
    let rtc = esp_hal::rtc_cntl::Rtc::new(board.rtc_timer);
    let (wd_supervisor, wd_handles) = wd::supervisor(rtc.rwdt, timg0.wdt);

    esp_rtos::start(timg0.timer0, board.from_cpu_intr0);
    info!("Embassy initialized");
    spawner.spawn(defmt::unwrap!(wd::supervise(wd_supervisor)));
    spawner.spawn(defmt::unwrap!(reboot::reboot_task(wd_handles.reboot)));

    spawner.spawn(defmt::unwrap!(blink(
        board.status_led,
        board.cfg_button,
        wd_handles.blinky
    )));

    let seed = random_seed(board.rng.reborrow(), board.adc1.reborrow());
    if let Err(err) = ethernet::start(
        spawner,
        board.ethernet,
        device_name,
        seed,
        wd_handles.ethernet,
        wd_handles.link,
    )
    .await
    {
        error!("Ethernet unavailable: {}", err);
    }
}

/// Draws a true random seed, briefly enabling ADC1 as the RNG's entropy source.
fn random_seed(rng: RNG<'_>, adc1: ADC1<'_>) -> u64 {
    let _source = TrngSource::new(rng, adc1);
    let trng = defmt::unwrap!(Trng::try_new());
    (u64::from(trng.random()) << 32) | u64::from(trng.random())
}

#[embassy_executor::task]
async fn blink(mut led: StatusLed, button: CfgButton, wd: WatchdogHandle<Blinky>) -> ! {
    let mut ticker = Ticker::every(BOOT_FLASH_PERIOD / 2);
    for _ in 0..BOOT_FLASHES {
        led.set_on();
        ticker.next().await;
        led.set_off();
        ticker.next().await;
        wd.feed();
    }

    let mut ticker = Ticker::every(BLINK_PERIOD);

    loop {
        ticker.next().await;

        led.toggle();
        wd.feed();

        if button.is_pressed() {
            Timer::after_secs(5).await;
        }
    }
}
