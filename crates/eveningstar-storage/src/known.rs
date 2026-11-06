//! The known bootloaders, by the SHA-256 digest of their image, with the
//! offset each reads the partition table from and whether it supports app
//! rollback.
//!
//! A known bootloader's entry is how the verification chain knows where the
//! table the bootloader reads lies; without one, the board is searched for
//! it. Only a known bootloader that supports rollback lets a board reach the
//! boot tier, in which the boot selection is written, as an update with any
//! other bootloader could leave a board unable to boot. The table offset is
//! for the report and for refusing a table elsewhere: it never moves a region
//! of the compiled layout.
//!
//! The list only grows once boards have shipped: every bootloader a board in
//! the field may have stays in it, so no firmware update takes a board out of
//! the boot tier. The `build` check group fails while the bootloader built
//! from `nix/board.nix` isn't listed, printing the entry for it.

use core::fmt;

use hex_literal::hex;

/// A known bootloader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    /// The SHA-256 digest its image stores and matches.
    pub digest: [u8; 32],
    /// What it is, for logs and the API.
    pub name: &'static str,
    /// The offset it reads the partition table from
    /// (`CONFIG_PARTITION_TABLE_OFFSET`).
    pub table_offset: u32,
    /// Whether it supports app rollback
    /// (`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`).
    pub rollback: bool,
}

/// Every known bootloader.
// Digests are written in hex and decoded at compile time, so a malformed
// digest fails the build.
pub const BOOTLOADERS: &[Known] = &[
    // The ESP-IDF v5.5.2 bootloader Nix builds, with
    // PARTITION_TABLE_OFFSET 0x8000 and BOOTLOADER_APP_ROLLBACK_ENABLE.
    Known {
        digest: hex!("6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421"),
        name: "ESP-IDF v5.5.2, app rollback",
        table_offset: 0x8000,
        rollback: true,
    },
];

/// The known bootloader whose digest is `digest`, or `None` when it isn't
/// known.
#[must_use]
pub fn find(digest: &[u8; 32]) -> Option<&'static Known> {
    BOOTLOADERS.iter().find(|known| known.digest == *digest)
}

/// The digest as 64 lowercase hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hex<'a>(pub &'a [u8; 32]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// The entry of [`BOOTLOADERS`] for a bootloader, as it is written in this
/// file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The bootloader's digest.
    pub digest: &'a [u8; 32],
    /// Its name.
    pub name: &'a str,
    /// The offset it reads the partition table from.
    pub table_offset: u32,
    /// Whether it supports app rollback.
    pub rollback: bool,
}

impl fmt::Display for Entry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "    Known {{")?;
        writeln!(f, "        digest: hex!(\"{}\"),", Hex(self.digest))?;
        writeln!(f, "        name: \"{}\",", self.name.escape_debug())?;
        writeln!(f, "        table_offset: {:#x},", self.table_offset)?;
        writeln!(f, "        rollback: {},", self.rollback)?;
        write!(f, "    }},")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::verify_bootloader;
    use crate::layout;
    use crate::test_support::{BOOTLOADER, ESPFLASH_BOOTLOADER, run};

    #[test]
    fn the_nix_bootloader_is_known_with_its_table_offset_and_rollback() {
        let verified = run(verify_bootloader(BOOTLOADER)).unwrap();
        let known = find(verified.digest()).unwrap();
        assert_eq!(known.name, "ESP-IDF v5.5.2, app rollback");
        assert_eq!(known.table_offset, layout::TABLE_OFFSET);
        assert!(known.rollback);
        // It ends before the table it reads.
        assert!(verified.size() <= known.table_offset);
    }

    #[test]
    fn the_espflash_bootloader_isnt_known() {
        let verified = run(verify_bootloader(ESPFLASH_BOOTLOADER)).unwrap();
        assert_eq!(find(verified.digest()), None);
        assert_eq!(find(&[0; 32]), None);
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
    fn hex_is_written_in_lowercase() {
        let mut digest: [u8; 32] = core::array::from_fn(|i| u8::try_from(i).unwrap());
        digest[31] = 0xff;
        assert_eq!(
            Hex(&digest).to_string(),
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1eff"
        );
    }

    #[test]
    fn entries_are_printed_as_written_here() {
        let source = include_str!("known.rs");
        for known in BOOTLOADERS {
            let entry = Entry {
                digest: &known.digest,
                name: known.name,
                table_offset: known.table_offset,
                rollback: known.rollback,
            }
            .to_string();
            assert!(source.contains(&entry), "{entry}");
        }
        let entry = Entry {
            digest: &[0xAB; 32],
            name: "a \"quoted\" name",
            table_offset: 0xC000,
            rollback: false,
        };
        assert_eq!(
            entry.to_string(),
            format!(
                "    Known {{\n        digest: hex!(\"{}\"),\n        name: \"a \\\"quoted\\\" name\",\n        table_offset: 0xc000,\n        rollback: false,\n    }},",
                "ab".repeat(32)
            )
        );
    }
}
