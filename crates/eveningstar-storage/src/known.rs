//! The bootloaders known to support app rollback, by the SHA-256 digest of
//! their image (T3). Only a board with one of them reaches the full tier,
//! where the firmware writes the boot selection, as an update with any other
//! bootloader could leave the board unable to boot.
//!
//! The list only grows once boards have shipped: every bootloader a board in
//! the field may have stays in it, so no firmware update takes a board out of
//! the full tier.

use core::fmt;

/// A bootloader known to support app rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    /// The SHA-256 digest its image stores and matches.
    pub digest: [u8; 32],
    /// What it is, for logs and the API.
    pub name: &'static str,
}

/// Every known bootloader.
// Entries are written as hex digests, decoded at compile time, so a malformed
// digest fails the build.
pub const BOOTLOADERS: &[Known] = &[
    // The ESP-IDF v5.5.2 bootloader Nix builds, with
    // BOOTLOADER_APP_ROLLBACK_ENABLE.
    known(
        "6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421",
        "ESP-IDF v5.5.2, app rollback",
    ),
];

/// The entry for `hex`, which has to be 64 hex digits.
const fn known(hex: &str, name: &'static str) -> Known {
    match decode_hex(hex) {
        Some(digest) => Known { digest, name },
        None => panic!("a known bootloader's digest isn't 64 hex digits"),
    }
}

/// The name of the known bootloader whose digest is `digest`, or `None` when
/// it isn't known.
#[must_use]
pub fn name(digest: &[u8; 32]) -> Option<&'static str> {
    BOOTLOADERS
        .iter()
        .find(|known| known.digest == *digest)
        .map(|known| known.name)
}

/// The 32 bytes `hex` spells: 64 hex digits, in either case. `None` for
/// anything else.
#[must_use]
pub const fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    let mut i = 0;
    while i < 32 {
        let (Some(high), Some(low)) = (digit(hex[2 * i]), digit(hex[2 * i + 1])) else {
            return None;
        };
        bytes[i] = high << 4 | low;
        i += 1;
    }
    Some(bytes)
}

const fn digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The digest as 64 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hex<'a>(pub &'a [u8; 32]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// The entry of [`BOOTLOADERS`] for a bootloader with `digest` and `name`, as
/// it is written in this file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The bootloader's digest.
    pub digest: &'a [u8; 32],
    /// Its name.
    pub name: &'a str,
}

impl fmt::Display for Entry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "    known(")?;
        writeln!(f, "        \"{}\",", Hex(self.digest))?;
        writeln!(f, "        \"{}\",", self.name.escape_debug())?;
        write!(f, "    ),")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::verify_bootloader;
    use crate::test_support::{BOOTLOADER, ESPFLASH_BOOTLOADER, run};

    #[test]
    fn the_nix_bootloader_is_known() {
        let verified = run(verify_bootloader(BOOTLOADER)).unwrap();
        assert_eq!(
            name(verified.digest()),
            Some("ESP-IDF v5.5.2, app rollback")
        );
    }

    #[test]
    fn the_espflash_bootloader_isnt_known() {
        let verified = run(verify_bootloader(ESPFLASH_BOOTLOADER)).unwrap();
        assert_eq!(name(verified.digest()), None);
        assert_eq!(name(&[0; 32]), None);
    }

    #[test]
    fn digests_are_unique() {
        for (i, a) in BOOTLOADERS.iter().enumerate() {
            for b in &BOOTLOADERS[i + 1..] {
                assert_ne!(a.digest, b.digest, "{} and {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn hex_decodes_in_either_case() {
        let lower = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1eff";
        let mut expected: [u8; 32] = core::array::from_fn(|i| u8::try_from(i).unwrap());
        expected[31] = 0xff;
        assert_eq!(decode_hex(lower), Some(expected));
        assert_eq!(decode_hex(&lower.to_uppercase()), Some(expected));
        assert_eq!(Hex(&expected).to_string(), lower);
    }

    #[test]
    fn hex_of_the_wrong_length_or_with_other_characters_is_refused() {
        let valid = "6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421";
        assert!(decode_hex(valid).is_some());
        assert_eq!(decode_hex(""), None);
        assert_eq!(decode_hex(&valid[..63]), None);
        assert_eq!(decode_hex(&valid[..62]), None);
        assert_eq!(decode_hex(&format!("{valid}0")), None);
        assert_eq!(decode_hex(&format!("{valid}00")), None);
        for bad in ['g', 'G', ' ', 'x', '-', '\0', '/', ':', '@', '`'] {
            for at in [0, 1, 37, 63] {
                let mut hex: Vec<char> = valid.chars().collect();
                hex[at] = bad;
                let hex: String = hex.into_iter().collect();
                assert_eq!(decode_hex(&hex), None, "{bad:?} at {at}");
            }
        }
        // 64 bytes, but not 64 characters.
        assert_eq!(decode_hex(&format!("{}é", &valid[..62])), None);
    }

    #[test]
    fn entries_are_printed_as_written_here() {
        let source = include_str!("known.rs");
        for known in BOOTLOADERS {
            let entry = Entry {
                digest: &known.digest,
                name: known.name,
            }
            .to_string();
            assert!(source.contains(&entry), "{entry}");
        }
        let entry = Entry {
            digest: &[0xAB; 32],
            name: "a \"quoted\" name",
        };
        assert_eq!(
            entry.to_string(),
            format!(
                "    known(\n        \"{}\",\n        \"a \\\"quoted\\\" name\",\n    ),",
                "ab".repeat(32)
            )
        );
    }
}
