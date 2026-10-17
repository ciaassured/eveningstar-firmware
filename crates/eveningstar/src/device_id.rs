//! The device's identity, derived from the chip's factory base MAC address.

/// Prefix of the device's [name](DeviceId::name).
const NAME_PREFIX: &str = "eveningstar-";

/// Longest name a DHCP hostname or a Wi-Fi SSID can hold.
pub(crate) const NAME_MAX_LEN: usize = 32;

/// Uniquely identifies this device: the chip's factory base MAC address, which
/// Espressif assigns to every chip. Every interface's MAC address is derived
/// from it, so the ID is the same whichever interface reports it.
#[derive(Clone, Copy, Debug, defmt::Format)]
pub(crate) struct DeviceId([u8; 6]);

impl DeviceId {
    /// Reads the ID from eFuse.
    pub(crate) fn read() -> Self {
        let mut id = [0u8; 6];
        id.copy_from_slice(esp_hal::efuse::base_mac_address().as_bytes());
        Self(id)
    }

    /// The full ID as 12 lowercase hex digits, e.g. `a1b2c3d4e5f6`.
    pub(crate) fn serial(&self) -> heapless::String<12> {
        let mut serial = heapless::String::new();
        push_hex(&mut serial, &self.0);
        serial
    }

    /// The device's name on networks: [`NAME_PREFIX`] and the last three bytes
    /// of the ID as hex, e.g. `eveningstar-d4e5f6`.
    pub(crate) fn name(&self) -> heapless::String<NAME_MAX_LEN> {
        let mut name = heapless::String::new();
        // The prefix and 6 hex digits always fit in NAME_MAX_LEN.
        let _ = name.push_str(NAME_PREFIX);
        push_hex(&mut name, &self.0[3..]);
        name
    }
}

/// Appends `bytes` to `out` as lowercase hex, two digits per byte, stopping if
/// `out` fills up.
fn push_hex<const N: usize>(out: &mut heapless::String<N>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in bytes {
        let _ = out.push(char::from(HEX[usize::from(byte >> 4)]));
        let _ = out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}
