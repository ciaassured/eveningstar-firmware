//! Signature files: any bytes verify or are refused without a panic, and
//! whatever verifies holds, on its second line, a signature by the trusted
//! key over its message that `verify_strict` accepts, and only SHA256 lines.

#![no_main]

use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use eveningstar_signify::{Generation, Purpose, TrustSet, TrustedKey, verify};
use libfuzzer_sys::fuzz_target;

/// The key the corpus is signed with: seed `[7; 32]`, keynum 1 to 8.
const SEED: [u8; 32] = [7; 32];
const KEYNUM: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

static TRUST: LazyLock<TrustSet> = LazyLock::new(|| {
    let key = SigningKey::from_bytes(&SEED).verifying_key().to_bytes();
    let keys = Box::leak(Box::new([TrustedKey::for_tests(
        Purpose::Firmware,
        Generation::Release(1),
        KEYNUM,
        key,
    )]));
    TrustSet::for_tests(Purpose::Firmware, keys)
});

fuzz_target!(|data: &[u8]| {
    let Ok(verified) = verify(data, &TRUST) else {
        return;
    };
    let message = verified.message();
    assert!(data.len() <= 4096 && !message.is_empty() && message.len() <= 2048);
    // The message is everything after the second line.
    let header = data.len() - message.len();
    assert_eq!(data[header - 1], b'\n');
    assert_eq!(
        data[..header].iter().filter(|byte| **byte == b'\n').count(),
        2
    );
    let line = data.split(|byte| *byte == b'\n').nth(1).unwrap();
    let decoded = STANDARD.decode(line).unwrap();
    assert_eq!(&decoded[..2], b"Ed");
    assert_eq!(&decoded[2..10], &KEYNUM);
    let signature = Signature::from_bytes(&decoded[10..74].try_into().unwrap());
    let key = VerifyingKey::from_bytes(verified.key().public_key()).unwrap();
    assert!(key.verify_strict(message, &signature).is_ok());
    assert_eq!(
        verified.lines().count(),
        message.iter().filter(|byte| **byte == b'\n').count()
    );
});
