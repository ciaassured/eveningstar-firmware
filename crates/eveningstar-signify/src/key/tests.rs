use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use curve25519_dalek::constants::EIGHT_TORSION;
use ed25519_dalek::SigningKey;

use super::{KeyError, check_key, public_key};

fn pub_file(comment: &str, decoded: &[u8]) -> Vec<u8> {
    format!(
        "untrusted comment: {comment}\n{}\n",
        STANDARD.encode(decoded)
    )
    .into_bytes()
}

fn key() -> [u8; 32] {
    SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes()
}

#[test]
fn a_public_key_file_reads() {
    let mut decoded = b"Ed".to_vec();
    decoded.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    decoded.extend_from_slice(&key());
    let file = pub_file("eveningstar 1 firmware public key", &decoded);
    assert_eq!(public_key(&file), Ok(([1, 2, 3, 4, 5, 6, 7, 8], key())));
    // Anything after the key's line, or a missing newline, is refused.
    let mut longer = file.clone();
    longer.extend_from_slice(b"more\n");
    assert_eq!(public_key(&longer), Err(KeyError::Format));
    assert_eq!(public_key(&file[..file.len() - 1]), Err(KeyError::Format));
    decoded[0] = b'X';
    assert_eq!(
        public_key(&pub_file("x", &decoded)),
        Err(KeyError::Algorithm)
    );
}

#[test]
fn a_generated_key_may_be_trusted() {
    for seed in 0..32u8 {
        let key = SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes();
        assert_eq!(check_key(&key), Ok(()));
    }
}

#[test]
fn small_and_mixed_order_keys_are_refused() {
    for point in EIGHT_TORSION {
        assert_eq!(check_key(&point.compress().0), Err(KeyError::Weak));
    }
    let good = curve25519_dalek::edwards::CompressedEdwardsY(key())
        .decompress()
        .unwrap();
    let mixed = good + EIGHT_TORSION[1];
    assert_eq!(check_key(&mixed.compress().0), Err(KeyError::Weak));
}

#[test]
fn non_canonical_and_off_curve_encodings_are_refused() {
    // y = p + 1, a non-canonical encoding of y = 1 (the identity).
    let mut non_canonical = [0xFF; 32];
    non_canonical[0] = 0xEE;
    non_canonical[31] = 0x7F;
    assert!(matches!(
        check_key(&non_canonical),
        Err(KeyError::Point | KeyError::Weak)
    ));
    // y = 2 isn't on the curve.
    let mut off = [0; 32];
    off[0] = 2;
    assert_eq!(check_key(&off), Err(KeyError::Point));
}
