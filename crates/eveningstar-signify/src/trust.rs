//! Firmware trust sets, generated from `nix/keys.nix` by the `trust-sets`
//! command and not edited by hand.

use crate::key::{Generation, Purpose, TrustSet, TrustedKey};

/// The development key, `keys/eveningstar-dev-fw.pub`, whose secret key is
/// public.
pub const DEVELOPMENT_KEY: TrustedKey = TrustedKey::new(
    Purpose::Firmware,
    Generation::Development,
    [0x2c, 0x34, 0xef, 0x67, 0xb0, 0x8d, 0x9c, 0xdb],
    [
        0x30, 0xf0, 0xbc, 0x59, 0x0f, 0x0c, 0xed, 0xfb, 0xd3, 0x01, 0x86, 0xe5, 0xbe, 0x2e, 0x20,
        0xe6, 0x1a, 0x6b, 0x18, 0x40, 0xe0, 0xce, 0x0c, 0x76, 0xd4, 0x04, 0x99, 0x05, 0x81, 0x53,
        0x7d, 0xd2,
    ],
);

/// What a development build trusts: the development key alone, as no release
/// generation exists yet.
pub const DEVELOPMENT: TrustSet = TrustSet::new(Purpose::Firmware, &[DEVELOPMENT_KEY]);
