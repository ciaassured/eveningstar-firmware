//! Partition layout of the internal flash, generated from `nix/board.nix` by
//! the `storage-layout` command and not edited by hand.

use crate::Entry;

/// Size of the flash chip in bytes.
pub const FLASH_SIZE: u32 = 0x0080_0000;

/// Offset of the partition table.
pub const TABLE_OFFSET: u32 = 0x0000_8000;

/// The `factory` partition, of type `data` and subtype `undefined`.
pub const FACTORY: Entry = Entry {
    label: "factory",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0000_9000,
    size: 0x0000_1000,
    flags: 0x0000_0002,
};

/// The `otadata` partition, of type `data` and subtype `ota`.
pub const OTADATA: Entry = Entry {
    label: "otadata",
    kind: 0x01,
    subtype: 0x00,
    offset: 0x0000_a000,
    size: 0x0000_2000,
    flags: 0x0000_0000,
};

/// The `sig_0` partition, of type `data` and subtype `undefined`.
pub const SIG_0: Entry = Entry {
    label: "sig_0",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0000_c000,
    size: 0x0000_1000,
    flags: 0x0000_0000,
};

/// The `sig_1` partition, of type `data` and subtype `undefined`.
pub const SIG_1: Entry = Entry {
    label: "sig_1",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0000_d000,
    size: 0x0000_1000,
    flags: 0x0000_0000,
};

/// The `ota_0` partition, of type `app` and subtype `ota_0`.
pub const OTA_0: Entry = Entry {
    label: "ota_0",
    kind: 0x00,
    subtype: 0x10,
    offset: 0x0001_0000,
    size: 0x0030_0000,
    flags: 0x0000_0000,
};

/// The `ota_1` partition, of type `app` and subtype `ota_1`.
pub const OTA_1: Entry = Entry {
    label: "ota_1",
    kind: 0x00,
    subtype: 0x11,
    offset: 0x0031_0000,
    size: 0x0030_0000,
    flags: 0x0000_0000,
};

/// The `config_0` partition, of type `data` and subtype `undefined`.
pub const CONFIG_0: Entry = Entry {
    label: "config_0",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0061_0000,
    size: 0x0002_0000,
    flags: 0x0000_0000,
};

/// The `config_1` partition, of type `data` and subtype `undefined`.
pub const CONFIG_1: Entry = Entry {
    label: "config_1",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0063_0000,
    size: 0x0002_0000,
    flags: 0x0000_0000,
};

/// The `diag` partition, of type `data` and subtype `undefined`.
pub const DIAG: Entry = Entry {
    label: "diag",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0065_0000,
    size: 0x0004_0000,
    flags: 0x0000_0000,
};

/// The `reserved` partition, of type `data` and subtype `undefined`.
pub const RESERVED: Entry = Entry {
    label: "reserved",
    kind: 0x01,
    subtype: 0x06,
    offset: 0x0069_0000,
    size: 0x0017_0000,
    flags: 0x0000_0000,
};

/// Every partition, in table order.
// One partition per line, whatever the lengths of the names.
#[rustfmt::skip]
pub const PARTITIONS: [Entry; 10] = [
    FACTORY,
    OTADATA,
    SIG_0,
    SIG_1,
    OTA_0,
    OTA_1,
    CONFIG_0,
    CONFIG_1,
    DIAG,
    RESERVED,
];
