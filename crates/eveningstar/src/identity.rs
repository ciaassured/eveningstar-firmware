//! The device's identity: its [`DeviceId`] and the names derived from it.

use eveningstar_api::{DEVICE_ID_LEN, DeviceId};

/// Prefix of the device's [name].
const NAME_PREFIX: &str = "eveningstar-";

/// Length of the end of the [`DeviceId`] a [name] carries: its last two
/// groups. The first group encodes only the manufacturer's prefix of the MAC
/// address, which boards share, while the last two hold the bits that tell
/// boards apart, and the check symbol.
const NAME_ID_SUFFIX_LEN: usize = 8;

/// Longest name a DHCP hostname or a Wi-Fi SSID can hold.
pub(crate) const NAME_MAX_LEN: usize = 32;

/// The device's name on networks: [`NAME_PREFIX`] and the end of its ID in
/// lowercase, e.g. `eveningstar-0kd7-9c6` for ID `4WSG-0KD7-9C6`.
pub(crate) fn name(id: DeviceId) -> heapless::String<NAME_MAX_LEN> {
    let id = id.encode();
    let mut name = heapless::String::new();
    // The prefix and the suffix always fit in NAME_MAX_LEN.
    let _ = name.push_str(NAME_PREFIX);
    let _ = name.push_str(&id[DEVICE_ID_LEN - NAME_ID_SUFFIX_LEN..]);
    name.make_ascii_lowercase();
    name
}
