//! Device IDs: globally unique and stable, written for people.
//!
//! An ID is a 50-bit value, written in Crockford's Base32 as 10 symbols,
//! followed by one Luhn mod 32 check symbol, in groups of 4, 4 and 3:
//! `4WSG-0KD7-9C6`. [`DeviceId::from_mac`] makes one from a factory base MAC
//! address, which is unique and never changes.
//!
//! Parsing accepts what a person might type: either case, any hyphens, and the
//! letters `O`, `I` and `L` for the digits they resemble. The check symbol
//! catches any single mistyped symbol and most swaps of adjacent symbols.

use core::fmt;
use core::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Crockford's Base32 alphabet: digits and capital letters except `I`, `L`,
/// `O` and `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Symbols encoding the value, 5 bits each.
const VALUE_SYMBOLS: usize = 10;

/// All symbols: the value and the check symbol.
const SYMBOLS: usize = VALUE_SYMBOLS + 1;

/// Length of the written form: the symbols and two hyphens.
pub const DEVICE_ID_LEN: usize = SYMBOLS + 2;

/// A device's ID.
///
/// Displays, serialises and parses as the written form, e.g. `4WSG-0KD7-9C6`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(
    /// The value, below 2^50.
    u64,
);

impl DeviceId {
    /// The ID made from factory base MAC address `mac`.
    #[must_use]
    pub const fn from_mac(mac: [u8; 6]) -> Self {
        let [m0, m1, m2, m3, m4, m5] = mac;
        Self(u64::from_be_bytes([0, 0, m0, m1, m2, m3, m4, m5]))
    }

    /// The written form, e.g. `4WSG-0KD7-9C6`.
    #[must_use]
    pub fn encode(&self) -> heapless::String<DEVICE_ID_LEN> {
        let mut symbols = [0u8; SYMBOLS];
        let value = self.0;
        for (i, symbol) in symbols[..VALUE_SYMBOLS].iter_mut().enumerate() {
            let shift = 5 * (VALUE_SYMBOLS - 1 - i);
            *symbol = low_5_bits(value >> shift);
        }
        symbols[VALUE_SYMBOLS] = check_symbol(&symbols[..VALUE_SYMBOLS]);

        let mut out = heapless::String::new();
        for (i, &symbol) in symbols.iter().enumerate() {
            if i == 4 || i == 8 {
                let _ = out.push('-');
            }
            let _ = out.push(char::from(ALPHABET[usize::from(symbol)]));
        }
        out
    }
}

/// Why a string is not a device ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseDeviceIdError {
    /// A character outside the alphabet and not a hyphen.
    InvalidCharacter,
    /// Not exactly 11 symbols, ignoring hyphens.
    InvalidLength,
    /// The check symbol does not match: the ID was mistyped.
    CheckFailed,
}

impl fmt::Display for ParseDeviceIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidCharacter => "invalid character in device ID",
            Self::InvalidLength => "device ID must have 11 symbols",
            Self::CheckFailed => "device ID check symbol does not match",
        })
    }
}

impl core::error::Error for ParseDeviceIdError {}

impl FromStr for DeviceId {
    type Err = ParseDeviceIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut symbols = [0u8; SYMBOLS];
        let mut len = 0;
        for c in s.bytes().filter(|&c| c != b'-') {
            let symbol = decode_symbol(c).ok_or(ParseDeviceIdError::InvalidCharacter)?;
            *symbols
                .get_mut(len)
                .ok_or(ParseDeviceIdError::InvalidLength)? = symbol;
            len += 1;
        }
        if len != SYMBOLS {
            return Err(ParseDeviceIdError::InvalidLength);
        }

        let value = symbols[..VALUE_SYMBOLS]
            .iter()
            .fold(0u64, |value, &symbol| (value << 5) | u64::from(symbol));
        if check_symbol(&symbols[..VALUE_SYMBOLS]) != symbols[VALUE_SYMBOLS] {
            return Err(ParseDeviceIdError::CheckFailed);
        }
        Ok(Self(value))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DeviceIdVisitor;

        impl Visitor<'_> for DeviceIdVisitor {
            type Value = DeviceId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a device ID such as 4WSG-0KD7-9C6")
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<DeviceId, E> {
                s.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DeviceIdVisitor)
    }
}

/// The written form: symbols of the alphabet, in groups of 4, 4 and 3.
#[cfg(feature = "schema")]
const PATTERN: &str = "^[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{3}$";

#[cfg(feature = "schema")]
impl schemars::JsonSchema for DeviceId {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "DeviceId".into()
    }

    fn schema_id() -> alloc::borrow::Cow<'static, str> {
        concat!(module_path!(), "::DeviceId").into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A device's ID: globally unique, and the same for the life \
                of the device. 11 symbols of Crockford's Base32, in groups of 4, 4 and 3; \
                the last is a check symbol over the others, by the Luhn mod N algorithm \
                with N = 32.",
            "pattern": PATTERN,
            "examples": ["4WSG-0KD7-9C6"],
        })
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is masked to 5 bits first"
)]
fn low_5_bits(value: u64) -> u8 {
    (value & 0x1f) as u8
}

/// The symbol's value, reading `O` as `0` and `I` and `L` as `1`.
fn decode_symbol(c: u8) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        b'O' => b'0',
        b'I' | b'L' => b'1',
        c => c,
    };
    ALPHABET
        .iter()
        .position(|&symbol| symbol == c)
        .and_then(|position| u8::try_from(position).ok())
}

/// The Luhn mod 32 check symbol of `symbols`, each a value below 32.
fn check_symbol(symbols: &[u8]) -> u8 {
    const N: u32 = 32;
    let mut sum = 0;
    // The symbol nearest the check symbol is doubled, then every other one.
    for (i, &symbol) in symbols.iter().rev().enumerate() {
        let factor = if i % 2 == 0 { 2 } else { 1 };
        let addend = factor * u32::from(symbol);
        sum += addend / N + addend % N;
    }
    low_5_bits(u64::from((N - sum % N) % N))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ID of the first board this was tested on.
    const MAC: [u8; 6] = [0x9c, 0xcc, 0x01, 0x36, 0x9d, 0x2c];
    const ID: &str = "4WSG-0KD7-9C6";

    /// Values and their written forms, from a separate implementation of the
    /// Luhn mod N algorithm.
    const VECTORS: [(u64, &str); 6] = [
        (0x9ccc_0136_9d2c, "4WSG-0KD7-9C6"),
        (1, "0000-0000-01Y"),
        ((1 << 50) - 1, "ZZZZ-ZZZZ-ZZA"),
        (0x2_a5a5_a5a5_a5a5, "N5MP-JTB9-D55"),
        (0x3_ca9d_52e6_b438, "YAKN-9EDD-1RJ"),
        (0x1_944c_269e_0d37, "CM9G-K9W3-9Q2"),
    ];

    #[test]
    fn encodes_known_id() {
        assert_eq!(DeviceId::from_mac(MAC).encode(), ID);
    }

    #[test]
    fn matches_reference_vectors() {
        for (value, written) in VECTORS {
            assert_eq!(DeviceId(value).encode(), written);
            assert_eq!(written.parse(), Ok(DeviceId(value)));
        }
    }

    #[test]
    fn reads_mac_addresses_big_endian() {
        assert_eq!(DeviceId::from_mac([0, 0, 0, 0, 0, 1]), DeviceId(1));
        assert_eq!(DeviceId::from_mac(MAC), DeviceId(0x9ccc_0136_9d2c));
    }

    #[test]
    fn written_form_has_fixed_layout() {
        for id in extremes() {
            let written = id.encode();
            assert_eq!(written.len(), DEVICE_ID_LEN);
            for (i, c) in written.chars().enumerate() {
                assert_eq!(c == '-', i == 4 || i == 9, "{written}");
            }
            assert_eq!(id.to_string(), written.as_str());
        }
    }

    /// IDs at the ends of the range, and with every first symbol.
    fn extremes() -> impl Iterator<Item = DeviceId> {
        let first_symbols = (0..32).map(|symbol| DeviceId(symbol << 45));
        [0, 1, (1 << 50) - 1]
            .into_iter()
            .map(DeviceId)
            .chain(first_symbols)
    }

    #[test]
    fn round_trips_extremes() {
        for id in extremes() {
            assert_eq!(id.encode().parse(), Ok(id));
        }
        let id = DeviceId((1 << 50) - 1);
        assert!(id.encode().starts_with("ZZZZ-ZZZZ-ZZ"));
    }

    #[test]
    fn parses_what_people_type() {
        let id = Ok(DeviceId::from_mac(MAC));
        assert_eq!("4wsg-0kd7-9c6".parse(), id);
        assert_eq!("4WSG0KD79C6".parse(), id);
        assert_eq!("4WSG-OKD7-9C6".parse(), id);
        assert_eq!("4-W-S-G-0-K-D-7-9-C-6".parse(), id);
    }

    #[test]
    fn reads_i_and_l_as_one() {
        // An ID containing a 1, to swap for I and L.
        let id = DeviceId::from_mac([0, 0, 0, 0, 0, 1]);
        let written = id.encode();
        assert!(written.contains('1'));
        assert_eq!(written.replace('1', "I").parse(), Ok(id));
        assert_eq!(written.replace('1', "l").parse(), Ok(id));
    }

    #[test]
    fn serialises_as_written_form() {
        let id = DeviceId::from_mac(MAC);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{ID}\""));
        assert_eq!(serde_json::from_str::<DeviceId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<DeviceId>("\"4WSG-0KD7-9C7\"").is_err());
        assert!(serde_json::from_str::<DeviceId>("42").is_err());
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(
            "".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidLength)
        );
        assert_eq!(
            "----".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidLength)
        );
        assert_eq!(
            "4WSG-0KD7-9Cu".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidCharacter)
        );
        assert_eq!(
            "4WSG-0KD7-9C\u{e9}".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidCharacter)
        );
        assert_eq!(
            "4WSG-0KD7-9C".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidLength)
        );
        assert_eq!(
            "4WSG-0KD7-9C66".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidLength)
        );
        assert_eq!(
            "4WSG-0KD7-9CU".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidCharacter)
        );
        assert_eq!(
            "4WSG 0KD7 9C6".parse::<DeviceId>(),
            Err(ParseDeviceIdError::InvalidCharacter)
        );
    }

    #[test]
    fn detects_every_single_substitution() {
        let symbols: Vec<u8> = ID.bytes().filter(|&c| c != b'-').collect();
        for position in 0..symbols.len() {
            for &replacement in ALPHABET {
                if replacement == symbols[position] {
                    continue;
                }
                let mut typo = symbols.clone();
                typo[position] = replacement;
                let typo = core::str::from_utf8(&typo).unwrap();
                assert!(typo.parse::<DeviceId>().is_err(), "{typo} accepted");
            }
        }
    }

    #[test]
    fn detects_adjacent_swaps_except_0_and_z() {
        for position in 0..VALUE_SYMBOLS - 1 {
            for a in 0..32 {
                for b in 0..32 {
                    let mut symbols = [0u8; VALUE_SYMBOLS];
                    symbols[position] = a;
                    symbols[position + 1] = b;
                    let check = check_symbol(&symbols);
                    symbols.swap(position, position + 1);
                    let undetected = check_symbol(&symbols) == check;
                    let blind_spot = (a, b) == (0, 31) || (a, b) == (31, 0);
                    assert_eq!(undetected, a == b || blind_spot, "{a} {b} at {position}");
                }
            }
        }
    }

    #[cfg(feature = "schema")]
    #[test]
    fn schema_matches_written_form() {
        use crate::schema_accepts;

        let accepts = |id: &str| schema_accepts::<DeviceId>(&serde_json::json!(id));
        for id in extremes().chain([DeviceId::from_mac(MAC)]) {
            assert!(accepts(&id.encode()));
        }
        assert!(!accepts("4WSG-0KD7-9CU"));
        assert!(!accepts("4WSG0KD79C6"));
        assert!(!accepts("4wsg-0kd7-9c6"));
    }
}
