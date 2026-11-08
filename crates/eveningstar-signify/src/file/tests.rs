use core::fmt::Write as _;
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

use super::{SigError, ed25519, verify};
use crate::key::{Generation, Keynum, Purpose, TrustSet, TrustedKey};
use crate::manifest::ManifestError;

const SEED: [u8; 32] = [7; 32];
const KEYNUM: Keynum = *b"\x01\x02\x03\x04\x05\x06\x07\x08";
const OTHER_KEYNUM: Keynum = [9; 8];

/// The group order L, little-endian.
const L: [u8; 32] = [
    0xED, 0xD3, 0xF5, 0x5C, 0x1A, 0x63, 0x12, 0x58, 0xD6, 0x9C, 0xF7, 0xA2, 0xDE, 0xF9, 0xDE, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&SEED)
}

static TRUST: LazyLock<TrustSet> = LazyLock::new(|| {
    let key = TrustedKey::new(
        Purpose::Firmware,
        Generation::Release(1),
        KEYNUM,
        signing_key().verifying_key().to_bytes(),
    );
    TrustSet::new(Purpose::Firmware, Box::leak(Box::new([key])))
});

const IMAGE: &[u8] = b"an image";

fn manifest_line(name: &str, data: &[u8]) -> String {
    format!("SHA256 ({name}) = {}\n", hex(&Sha256::digest(data)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

/// A signature file as `signify -S -e` writes it, signed by `keynum` with
/// `signature` over `message`.
fn file_with(comment: &str, keynum: Keynum, signature: &[u8; 64], message: &[u8]) -> Vec<u8> {
    let mut decoded = b"Ed".to_vec();
    decoded.extend_from_slice(&keynum);
    decoded.extend_from_slice(signature);
    let mut file = format!(
        "untrusted comment: {comment}\n{}\n",
        STANDARD.encode(&decoded)
    )
    .into_bytes();
    file.extend_from_slice(message);
    file
}

/// A signature file over `message`, signed by the trusted key.
fn signed(message: &[u8]) -> Vec<u8> {
    let signature = signing_key().sign(message).to_bytes();
    file_with("verify with test.pub", KEYNUM, &signature, message)
}

fn good() -> Vec<u8> {
    signed(manifest_line("eveningstar-1.0.0-esp32c6.bin", IMAGE).as_bytes())
}

#[test]
fn a_signed_manifest_verifies_and_gives_its_line() {
    let file = good();
    let verified = verify(&file, &TRUST).unwrap();
    assert_eq!(verified.key().keynum(), &KEYNUM);
    let digest: [u8; 32] = Sha256::digest(IMAGE).into();
    let line = verified.line(&digest).unwrap();
    assert_eq!(line.name, "eveningstar-1.0.0-esp32c6.bin");
    assert_eq!(verified.line(&[0; 32]), Err(ManifestError::NoMatch));
}

#[test]
fn several_lines_and_a_duplicate_digest() {
    let message = format!(
        "{}{}",
        manifest_line("a.bin", IMAGE),
        manifest_line("a.elf", b"an elf")
    );
    let file = signed(message.as_bytes());
    let verified = verify(&file, &TRUST).unwrap();
    assert_eq!(verified.lines().count(), 2);
    let digest: [u8; 32] = Sha256::digest(IMAGE).into();
    assert_eq!(verified.line(&digest).unwrap().name, "a.bin");

    let twice = format!(
        "{}{}",
        manifest_line("a.bin", IMAGE),
        manifest_line("b.bin", IMAGE)
    );
    let file = signed(twice.as_bytes());
    let verified = verify(&file, &TRUST).unwrap();
    assert_eq!(verified.line(&digest), Err(ManifestError::Ambiguous));
}

#[test]
fn the_comment_line_is_checked_but_not_read() {
    let message = manifest_line("a.bin", IMAGE);
    let signature = signing_key().sign(message.as_bytes()).to_bytes();
    for (comment, ok) in [
        ("x", true),
        (&"x".repeat(1023), true),
        ("anything at all: verify with other.pub", true),
        ("", false),
        (&"x".repeat(1024), false),
    ] {
        let file = file_with(comment, KEYNUM, &signature, message.as_bytes());
        assert_eq!(verify(&file, &TRUST).is_ok(), ok, "{comment:?}");
        if !ok {
            assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::Comment);
        }
    }
    let mut file = good();
    file[0] = b'U';
    assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::Comment);
}

#[test]
fn the_signature_line_has_one_canonical_form() {
    let file = good();
    let start = file.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    let end = start + 100;
    let line = &file[start..end];
    assert_eq!(line[99], b'=');
    let variants: [Vec<u8>; 5] = [
        // Whitespace inside, which signify's b64_pton skips.
        [&line[..50], b" ", &line[50..]].concat(),
        // No padding.
        line[..99].to_vec(),
        // A CR before the newline.
        [line, b"\r"].concat(),
        // Non-zero bits in the padding: the last character before `=`
        // carries 2 unused bits.
        {
            let mut changed = line.to_vec();
            changed[98] = flip_low_bits(changed[98]);
            changed
        },
        // Not base64.
        [&line[..10], b"*", &line[11..]].concat(),
    ];
    for variant in variants {
        let changed = [&file[..start], &variant, &file[end..]].concat();
        assert_eq!(
            verify(&changed, &TRUST).unwrap_err(),
            SigError::SignatureLine,
            "{:?}",
            String::from_utf8_lossy(&variant)
        );
    }
}

/// The base64 digit whose value differs from `digit`'s in its lowest bit.
fn flip_low_bits(digit: u8) -> u8 {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value = ALPHABET.iter().position(|c| *c == digit).unwrap();
    ALPHABET[value ^ 1]
}

#[test]
fn the_key_is_chosen_by_keynum_and_must_be_trusted() {
    let message = manifest_line("a.bin", IMAGE);
    let signature = signing_key().sign(message.as_bytes()).to_bytes();
    let file = file_with("x", OTHER_KEYNUM, &signature, message.as_bytes());
    assert_eq!(
        verify(&file, &TRUST).unwrap_err(),
        SigError::UnknownKey {
            keynum: OTHER_KEYNUM
        }
    );
    let other = SigningKey::from_bytes(&[8; 32])
        .sign(message.as_bytes())
        .to_bytes();
    let file = file_with("x", KEYNUM, &other, message.as_bytes());
    assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::Signature);
}

#[test]
fn a_malleated_signature_is_refused() {
    let message = manifest_line("a.bin", IMAGE);
    let signature = signing_key().sign(message.as_bytes()).to_bytes();
    // S + L verifies under signify, which reduces S modulo L.
    let mut malleated = signature;
    let mut carry = 0u16;
    for (byte, add) in malleated[32..].iter_mut().zip(L) {
        let sum = u16::from(*byte) + u16::from(add) + carry;
        *byte = (sum & 0xFF) as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0);
    let file = file_with("x", KEYNUM, &malleated, message.as_bytes());
    assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::Signature);
}

#[test]
fn every_bit_flip_of_a_good_file_outside_the_comment_is_refused() {
    let file = good();
    let comment = b"untrusted comment: ".len()..file.iter().position(|b| *b == b'\n').unwrap();
    for bit in 0..file.len() * 8 {
        let mut flipped = file.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        let verdict = verify(&flipped, &TRUST);
        if comment.contains(&(bit / 8)) {
            // The comment isn't signed: a flip there matters only when it
            // makes a NUL or a newline.
            let byte = flipped[bit / 8];
            assert_eq!(
                verdict.is_ok(),
                byte != 0 && byte != b'\n',
                "comment bit {bit}"
            );
        } else {
            assert!(verdict.is_err(), "bit {bit} flipped still verifies");
        }
    }
}

#[test]
fn every_truncation_of_a_good_file_is_refused() {
    let file = good();
    for len in 0..file.len() {
        assert!(verify(&file[..len], &TRUST).is_err(), "length {len}");
    }
}

#[test]
fn the_manifest_has_one_line_form() {
    let digest = hex(&Sha256::digest(IMAGE));
    for message in [
        format!("SHA256 (a.bin) = {digest}"),
        format!("SHA256 (a.bin) = {digest}\n\n"),
        format!("SHA256 (a.bin) = {}\n", digest.to_uppercase()),
        format!("SHA256 (a.bin) = {digest} trailing\n"),
        format!("SHA256 (a.bin)  = {digest}\n"),
        format!("SHA512 (a.bin) = {digest}{digest}\n"),
        format!("SHA256 (a bin) = {digest}\n"),
        format!("SHA256 () = {digest}\n"),
        format!("SHA256 ({}) = {}\n", "a".repeat(129), digest),
        format!("SHA256 (a.bin) = {digest}\r\n"),
        format!(
            "SHA256 (a.bin) = {}\n",
            STANDARD.encode(Sha256::digest(IMAGE))
        ),
    ] {
        let file = signed(message.as_bytes());
        assert!(
            matches!(verify(&file, &TRUST), Err(SigError::Manifest { .. })),
            "{message:?}"
        );
    }
    let long_name = format!("SHA256 ({}) = {}\n", "a".repeat(128), digest);
    assert!(verify(&signed(long_name.as_bytes()), &TRUST).is_ok());
}

#[test]
fn sizes_and_nul_are_bounded() {
    assert_eq!(verify(&signed(b""), &TRUST).unwrap_err(), SigError::Message);
    let line = manifest_line("a.bin", IMAGE);
    let long = line.repeat(2048 / line.len() + 1);
    assert_eq!(
        verify(&signed(long.as_bytes()), &TRUST).unwrap_err(),
        SigError::Message
    );
    let mut file = good();
    file.extend(core::iter::repeat_n(b'x', 4097 - file.len()));
    assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::TooLong);
    let mut file = good();
    let last = file.len() - 2;
    file[last] = 0;
    assert_eq!(verify(&file, &TRUST).unwrap_err(), SigError::Nul);
}

/// An RFC 8032 vector: its name, public key, message and signature.
type Vector = (String, [u8; 32], Vec<u8>, [u8; 64]);

/// The vectors of RFC 8032 section 7.1, from `testdata`.
fn rfc8032() -> Vec<Vector> {
    include_str!("../../testdata/rfc8032-ed25519.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split(' ');
            let name = fields.next().unwrap().to_owned();
            let key = unhex(fields.next().unwrap()).try_into().unwrap();
            let message = match fields.next().unwrap() {
                "-" => Vec::new(),
                message => unhex(message),
            };
            let signature = unhex(fields.next().unwrap()).try_into().unwrap();
            (name, key, message, signature)
        })
        .collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn rfc_8032_vectors_verify_and_any_flipped_bit_fails() {
    let vectors = rfc8032();
    assert_eq!(vectors.len(), 5);
    for (name, key, message, signature) in vectors {
        assert!(ed25519(&key, &message, &signature), "{name}");
        for bit in [0, 255, 256, 511] {
            let mut flipped = signature;
            flipped[bit / 8] ^= 1 << (bit % 8);
            assert!(
                !ed25519(&key, &message, &flipped),
                "{name} signature bit {bit}"
            );
        }
        let mut flipped = key;
        flipped[0] ^= 1;
        assert!(!ed25519(&flipped, &message, &signature), "{name} key");
        if !message.is_empty() {
            let mut flipped = message.clone();
            flipped[0] ^= 1;
            assert!(!ed25519(&key, &flipped, &signature), "{name} message");
        }
    }
}

/// The `acceptable` Wycheproof cases `verify_strict` accepts: 160 to 180, the
/// 21 with a mixed-order public key (`MixedOrderPublicKey`), which it doesn't
/// refuse (it refuses small-order ones only). Reviewed: no such key can be
/// trusted, as `check_key` refuses a key that isn't torsion-free.
const WYCHEPROOF_ACCEPTABLE_ACCEPTED: core::ops::RangeInclusive<u64> = 160..=180;

#[test]
fn wycheproof_vectors_give_their_verdicts() {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../../testdata/wycheproof-ed25519.json")).unwrap();
    let mut seen = 0;
    let mut accepted_acceptable = Vec::new();
    for group in json["testGroups"].as_array().unwrap() {
        let key: [u8; 32] = unhex(group["publicKey"]["pk"].as_str().unwrap())
            .try_into()
            .unwrap();
        for test in group["tests"].as_array().unwrap() {
            seen += 1;
            let id = test["tcId"].as_u64().unwrap();
            let message = unhex(test["msg"].as_str().unwrap());
            let signature = unhex(test["sig"].as_str().unwrap());
            let verdict = signature
                .try_into()
                .is_ok_and(|signature| ed25519(&key, &message, &signature));
            match test["result"].as_str().unwrap() {
                "valid" => assert!(verdict, "valid case {id} refused"),
                "invalid" => assert!(!verdict, "invalid case {id} accepted"),
                "acceptable" => {
                    if verdict {
                        accepted_acceptable.push(id);
                    }
                }
                other => panic!("unknown result {other}"),
            }
        }
    }
    assert_eq!(seen, 210);
    assert_eq!(
        accepted_acceptable,
        WYCHEPROOF_ACCEPTABLE_ACCEPTED.collect::<Vec<_>>()
    );
}
