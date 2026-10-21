//! The device's identity and uptime.

use embassy_time::{Duration, Instant};
use eveningstar_api::{DEVICE_ID_LEN, DeviceId, v1};

/// Longest name a DHCP hostname or a Wi-Fi SSID can hold, and so the API's
/// limit on a device name.
pub(crate) const NAME_MAX_LEN: usize = v1::DEVICE_NAME_MAX_LEN;

/// Prefix of the device's [name](System::name).
const NAME_PREFIX: &str = "eveningstar-";

/// Length of the end of the [`DeviceId`] a [name](System::name) carries: its
/// last two groups. The first group encodes only the manufacturer's prefix of
/// the MAC address, which boards share, while the last two hold the bits that
/// tell boards apart, and the check symbol.
const NAME_ID_SUFFIX_LEN: usize = 8;

/// The device's identity and how long it has been running.
#[derive(Debug)]
pub(crate) struct System {
    id: DeviceId,
}

impl System {
    pub(crate) const fn new(id: DeviceId) -> Self {
        Self { id }
    }

    /// The device's globally unique ID.
    pub(crate) fn id(&self) -> DeviceId {
        self.id
    }

    /// The device's name on networks: [`NAME_PREFIX`] and the end of its ID in
    /// lowercase, e.g. `eveningstar-0kd7-9c6` for ID `4WSG-0KD7-9C6`.
    pub(crate) fn name(&self) -> heapless::String<NAME_MAX_LEN> {
        let id = self.id.encode();
        let mut name = heapless::String::new();
        // The prefix and the suffix always fit in NAME_MAX_LEN.
        let _ = name.push_str(NAME_PREFIX);
        let _ = name.push_str(&id[DEVICE_ID_LEN - NAME_ID_SUFFIX_LEN..]);
        name.make_ascii_lowercase();
        name
    }

    /// Time since the device started.
    pub(crate) fn uptime() -> Duration {
        Duration::from_ticks(Instant::now().as_ticks())
    }
}
