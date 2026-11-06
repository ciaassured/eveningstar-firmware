#![no_std]
#![no_main]
// The HTTP workers' futures nest picoserve's router types deeper than the
// default limit of 128 allows when computing their layout.
#![recursion_limit = "256"]

mod ethernet;
mod http;
mod reset;
mod services;
mod sha;
mod shared_spi;
mod storage;
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
use eveningstar_system::watchdog::WatchdogHandle;
use static_cell::StaticCell;

use crate::services::reboot::{self, Holder};
use crate::services::system::System;

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

/// The partition layout the firmware is built for, as a layout record.
/// esp-hal's linker script places the input sections `.flash.appdesc.*`
/// right after the app descriptor's `.flash.appdesc`, at the start of the
/// first segment, and the record's alignment of 1 leaves no gap after the
/// descriptor's 256 bytes: the record lies where
/// `image::app_layout` reads it.
#[unsafe(link_section = ".flash.appdesc.layout")]
#[used]
static LAYOUT_RECORD: eveningstar_storage::CompiledLayoutRecord =
    eveningstar_storage::LAYOUT_RECORD;

/// The supervised tasks, each with its marker type and timeout.
mod wd {
    use embassy_time::Duration;
    use eveningstar_system::reboot::RebootWatchdog;
    use eveningstar_system::watchdog::TaskWatchdog;

    /// Marker of the blink task's watchdog.
    pub(crate) enum Blinky {}
    /// Marker of the Ethernet probe's watchdog.
    pub(crate) enum EthernetProbe {}
    /// Marker of the link monitor's watchdog.
    pub(crate) enum LinkMonitor {}
    /// Marker of the app check task's watchdog.
    pub(crate) enum AppCheck {}
    /// Marker of the confirmation window's watchdog.
    pub(crate) enum Confirmation {}
    /// Marker of the watchdog of the storage API's binary answers.
    pub(crate) enum Download {}

    pub(crate) const BLINKY: TaskWatchdog<Blinky> =
        TaskWatchdog::new("blinky", Duration::from_millis(5000));
    /// Tolerates two missed probes.
    pub(crate) const ETHERNET: TaskWatchdog<EthernetProbe> = TaskWatchdog::new(
        "ethernet",
        Duration::from_ticks(crate::ethernet::PROBE_INTERVAL.as_ticks() * 3),
    );
    /// Tolerates two missed checks.
    pub(crate) const LINK: TaskWatchdog<LinkMonitor> = TaskWatchdog::new(
        "link",
        Duration::from_ticks(crate::ethernet::LINK_CHECK_INTERVAL.as_ticks() * 3),
    );
    pub(crate) const REBOOT: TaskWatchdog<RebootWatchdog> = eveningstar_system::reboot::WATCHDOG;
    pub(crate) const APP_CHECK: TaskWatchdog<AppCheck> =
        TaskWatchdog::new("app_check", crate::storage::WATCHDOG_TIMEOUT);
    pub(crate) const CONFIRMATION: TaskWatchdog<Confirmation> = TaskWatchdog::new(
        "confirmation",
        crate::storage::CONFIRMATION_WATCHDOG_TIMEOUT,
    );
    /// An answer feeds it after each chunk written and each chunk hashed,
    /// and a chunk's write is cut off after 5 s (`http/api.rs`), well
    /// within this.
    pub(crate) const DOWNLOAD: TaskWatchdog<Download> =
        TaskWatchdog::new("download", Duration::from_secs(10));

    /// The supervisor's slots, one per task above.
    pub(crate) const SLOTS: usize = [
        BLINKY.name(),
        ETHERNET.name(),
        LINK.name(),
        REBOOT.name(),
        APP_CHECK.name(),
        CONFIRMATION.name(),
        DOWNLOAD.name(),
    ]
    .len();
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

    static SYSTEM: StaticCell<System> = StaticCell::new();

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let mut board = Board::new(esp_hal::init(config));
    info!(
        "reset reason: {}",
        defmt::Debug2Format(&esp_hal::system::reset_reason())
    );
    let system = SYSTEM.init(System::new(DeviceId::from_mac(board.base_mac)));
    info!(
        "device {} (ID {})",
        system.name().as_str(),
        system.id().encode().as_str()
    );

    let timg0 = TimerGroup::new(board.timg0);
    let rtc = esp_hal::rtc_cntl::Rtc::new(board.rtc_timer);
    let reset = reset::take();
    let mut wd_supervisor = watchdog::supervisor(rtc.rwdt, timg0.wdt, reset);
    let blinky_wd = wd_supervisor.register(wd::BLINKY);
    let ethernet_wd = wd_supervisor.register(wd::ETHERNET);
    let link_wd = wd_supervisor.register(wd::LINK);
    let reboot_wd = wd_supervisor.register(wd::REBOOT);
    let app_check_wd = wd_supervisor.register(wd::APP_CHECK);
    let confirmation_wd = wd_supervisor.register(wd::CONFIRMATION);
    // Started now, which pauses its watchdog until a download, so it is
    // supervised whether or not the network comes up.
    let answers = http::Answers::start(wd_supervisor.register(wd::DOWNLOAD));

    esp_rtos::start(timg0.timer0, board.from_cpu_intr0);
    info!("Embassy initialized");
    spawner.spawn(defmt::unwrap!(watchdog::supervise(wd_supervisor)));
    let reboot = reboot::start(spawner, reset, reboot_wd);

    spawner.spawn(defmt::unwrap!(blink(
        board.status_led,
        board.cfg_button,
        blinky_wd
    )));

    let sha = sha::start(board.sha);
    // Drawn apart from the network stack's seed: it only keeps entity tags
    // of different boots apart, and is public.
    let boot = random_seed(board.rng.reborrow(), board.adc1.reborrow());
    let (storage, ota) = storage::start(
        spawner,
        board.internal_flash,
        board.flash_cache,
        sha,
        app_check_wd,
        reboot.graceful_only(),
        confirmation_wd,
        boot,
    )
    .await;

    let seed = random_seed(board.rng.reborrow(), board.adc1.reborrow());
    match ethernet::start(
        spawner,
        board.ethernet,
        system.name(),
        seed,
        reboot.graceful_only(),
        ethernet_wd,
        link_wd,
    )
    .await
    {
        Ok(stack) => http::start(
            spawner,
            stack,
            http::AppState {
                system,
                reboot,
                reboot_holder: reboot.register_holder(Holder::Http),
                storage,
                answers,
            },
        ),
        Err(err) => error!("Ethernet unavailable: {}", err),
    }

    // Built last, as it waits for the running slot's check. Nothing takes it
    // yet: it stays in its `static`, and can't be built again, as the boot
    // writers exist once per boot.
    if let Some(ota) = ota {
        ota.start().await;
    }
}

/// Draws a true random seed, briefly enabling ADC1 as the RNG's entropy source.
fn random_seed(rng: RNG<'_>, adc1: ADC1<'_>) -> u64 {
    let _source = TrngSource::new(rng, adc1);
    let trng = defmt::unwrap!(Trng::try_new());
    (u64::from(trng.random()) << 32) | u64::from(trng.random())
}

#[embassy_executor::task]
async fn blink(mut led: StatusLed, button: CfgButton, wd: WatchdogHandle<wd::Blinky>) -> ! {
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
