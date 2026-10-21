//! Wired Ethernet: the Ethernet controller's driver and the embassy-net IP
//! stack on top of it.

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{Config, DhcpConfig, Stack, StackResources};
use embassy_net_wiznet::{Device, InitError, Runner, State};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Ticker};
use embedded_hal_async::spi::{Operation, SpiDevice};
use esp_hal::gpio::{Input, Output};
use eveningstar_board::{ETHERNET_MAC_READ, Ethernet, EthernetChip, EthernetSpi};
use static_cell::{ConstStaticCell, StaticCell};

use crate::identity::NAME_MAX_LEN;
use crate::services::reboot::{GracefulReboot, Reason};
use crate::shared_spi::SharedSpi;
use crate::watchdog::WatchdogHandle;
use crate::wd::{EthernetProbe, LinkMonitor};

/// Packet queue depth in each direction; each slot holds one 1514-byte frame.
const QUEUE_LEN: usize = 4;

/// Sockets in the IP stack. DHCP takes one; the rest are for the application.
const SOCKETS: usize = 8;

/// How often the probe reads the controller's MAC address back.
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// How often the link monitor checks the Ethernet link.
pub(crate) const LINK_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// How long the link may stay down, or without an IPv4 address, before the link
/// monitor requests a reboot.
const LINK_DOWN_REBOOT: Duration = Duration::from_secs(24 * 60 * 60);

type ControllerSpi = SharedSpi<EthernetSpi>;
type SpiError = <EthernetSpi as embedded_hal_async::spi::ErrorType>::Error;
type WiznetRunner = Runner<'static, EthernetChip, ControllerSpi, Input<'static>, Output<'static>>;

#[derive(Debug, defmt::Format)]
pub(crate) enum EthernetError {
    /// The Ethernet controller failed to initialise.
    Init(InitError<SpiError>),
}

impl From<InitError<SpiError>> for EthernetError {
    fn from(err: InitError<SpiError>) -> Self {
        Self::Init(err)
    }
}

#[embassy_executor::task]
async fn wiznet_task(runner: WiznetRunner) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, Device<'static>>) -> ! {
    runner.run().await
}

/// Reads the controller's MAC address back on every tick and feeds the
/// watchdog only when it matches `mac`, the address the driver programmed.
///
/// A transaction that never completes (the SPI bus or device lock never
/// released, or a hung transfer) or a different address (the chip not
/// responding properly, or reset and so no longer configured) stops the feeds.
/// The state of the Ethernet link plays no part.
#[embassy_executor::task]
async fn probe(mut spi: ControllerSpi, mac: [u8; 6], wd: WatchdogHandle<EthernetProbe>) -> ! {
    let mut ticker = Ticker::every(PROBE_INTERVAL);
    loop {
        let mut read = [0u8; 6];
        let result = spi
            .transaction(&mut [
                Operation::Write(&ETHERNET_MAC_READ),
                Operation::Read(&mut read),
            ])
            .await;
        match result {
            Ok(()) if read == mac => wd.feed(),
            Ok(()) => error!(
                "Ethernet controller MAC reads {=[u8]:02x}, expected {=[u8]:02x}",
                read, mac
            ),
            Err(err) => error!("Ethernet controller probe failed: {}", err),
        }
        ticker.next().await;
    }
}

/// Requests a graceful reboot once the Ethernet link has been down, or up without
/// an IPv4 address (static or from DHCP), for [`LINK_DOWN_REBOOT`].
///
/// With no stack, because the controller never initialised, the link counts as down
/// from boot, so the reboot also retries initialisation.
#[embassy_executor::task]
async fn link_monitor(
    stack: Option<Stack<'static>>,
    reboot: GracefulReboot,
    wd: WatchdogHandle<LinkMonitor>,
) -> ! {
    let mut down_since: Option<Instant> = None;
    let mut ticker = Ticker::every(LINK_CHECK_INTERVAL);
    loop {
        wd.feed();
        if stack.is_some_and(|stack| stack.is_link_up() && stack.is_config_up()) {
            down_since = None;
        } else if down_since.get_or_insert_with(Instant::now).elapsed() >= LINK_DOWN_REBOOT
            && !reboot.is_pending()
        {
            warn!(
                "Ethernet link down or without an address for {} s, requesting a reboot",
                LINK_DOWN_REBOOT.as_secs()
            );
            reboot.graceful(Reason::LinkDown);
        }
        ticker.next().await;
    }
}

/// Logs changes in the Ethernet link and the IPv4 address.
#[embassy_executor::task]
async fn net_status(stack: Stack<'static>) -> ! {
    loop {
        stack.wait_link_up().await;
        info!("Ethernet link up, waiting for an IPv4 address");
        if let Either::First(()) = select(stack.wait_config_up(), stack.wait_link_down()).await {
            if let Some(config) = stack.config_v4() {
                info!(
                    "IPv4 address {}, gateway {}",
                    config.address, config.gateway
                );
            }
            if let Either::First(()) =
                select(stack.wait_config_down(), stack.wait_link_down()).await
            {
                warn!("IPv4 address lost");
            }
        }
        if !stack.is_link_up() {
            warn!("Ethernet link down");
        }
    }
}

/// Brings up the Ethernet controller and the IP stack, and spawns the tasks that drive,
/// probe and monitor them.
///
/// If the controller fails to initialise, the probe's watchdog is retired, the link
/// monitor runs with no link, and the error is returned.
#[allow(
    clippy::large_stack_frames,
    reason = "runs once at startup; the frame holds the driver and runners on their way into tasks"
)]
pub(crate) async fn start(
    spawner: Spawner,
    ethernet: Ethernet,
    hostname: heapless::String<NAME_MAX_LEN>,
    seed: u64,
    reboot: GracefulReboot,
    probe_wd: WatchdogHandle<EthernetProbe>,
    link_wd: WatchdogHandle<LinkMonitor>,
) -> Result<Stack<'static>, EthernetError> {
    // Built at compile time, so the packet buffers never pass through the stack.
    static STATE: ConstStaticCell<State<QUEUE_LEN, QUEUE_LEN>> = ConstStaticCell::new(State::new());
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    static SPI: StaticCell<Mutex<CriticalSectionRawMutex, EthernetSpi>> = StaticCell::new();

    let spi = SPI.init(Mutex::new(ethernet.spi));
    let mut dhcp = DhcpConfig::default();
    dhcp.hostname = Some(hostname);

    let result = embassy_net_wiznet::new::<QUEUE_LEN, QUEUE_LEN, EthernetChip, _, _, _>(
        ethernet.mac,
        STATE.take(),
        SharedSpi::new(spi),
        ethernet.int,
        ethernet.reset,
    )
    .await;
    let (device, runner) = match result {
        Ok(parts) => parts,
        Err(err) => {
            probe_wd.retire();
            spawner.spawn(defmt::unwrap!(link_monitor(None, reboot, link_wd)));
            return Err(err.into());
        }
    };
    spawner.spawn(defmt::unwrap!(wiznet_task(runner)));
    spawner.spawn(defmt::unwrap!(probe(
        SharedSpi::new(spi),
        ethernet.mac,
        probe_wd
    )));

    let (stack, runner) = embassy_net::new(
        device,
        Config::dhcpv4(dhcp),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(defmt::unwrap!(net_task(runner)));
    spawner.spawn(defmt::unwrap!(link_monitor(Some(stack), reboot, link_wd)));
    spawner.spawn(defmt::unwrap!(net_status(stack)));

    Ok(stack)
}
