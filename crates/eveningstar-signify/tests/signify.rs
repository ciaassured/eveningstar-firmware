//! Differential tests against OpenBSD's signify: every case is judged by the
//! stock `signify` binary and by this crate, and the device may be stricter
//! than signify only in the cases the crate documents. The device finds the
//! image's line by its digest, so signify checks the image under the name of
//! the line the device took.
//!
//! The binary is `signify` on the `PATH` (nixpkgs' portable signify in the
//! checks). Without it the tests are skipped, unless
//! `EVENINGSTAR_REQUIRE_SIGNIFY` is set, as the checks set it.

#![allow(clippy::unwrap_used, clippy::too_many_lines, reason = "tests")]

use core::fmt::Write as _;
use core::ops::Deref;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use eveningstar_signify::{
    Generation, ManifestError, Purpose, SigError, TrustSet, TrustedKey, check_key, public_key,
    verify,
};
use sha2::{Digest, Sha256};

/// The order L of the curve's prime-order subgroup, little-endian.
const L: [u8; 32] = [
    0xED, 0xD3, 0xF5, 0x5C, 0x1A, 0x63, 0x12, 0x58, 0xD6, 0x9C, 0xF7, 0xA2, 0xDE, 0xF9, 0xDE, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// Whether `signify` runs. Its absence fails when it's required, and is
/// noted otherwise.
fn signify_available() -> bool {
    let found = Command::new("signify")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();
    assert!(
        found || std::env::var_os("EVENINGSTAR_REQUIRE_SIGNIFY").is_none(),
        "signify isn't on the PATH, and EVENINGSTAR_REQUIRE_SIGNIFY is set"
    );
    if !found {
        eprintln!("signify isn't on the PATH: skipped");
    }
    found
}

fn signify(dir: &Path, args: &[&str]) -> bool {
    Command::new("signify")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A scratch directory for one test, removed when dropped.
struct Scratch(PathBuf);

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir =
        std::env::temp_dir().join(format!("eveningstar-signify-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// A key made by `signify -G` in `dir`, as `key.pub` and `key.sec`, and the
/// trust set holding its public key.
fn generated_key(dir: &Path) -> TrustSet {
    assert!(signify(
        dir,
        &["-G", "-n", "-c", "test", "-p", "key.pub", "-s", "key.sec"]
    ));
    let (keynum, key) = public_key(&fs::read(dir.join("key.pub")).unwrap()).unwrap();
    check_key(&key).unwrap();
    let keys = Box::leak(Box::new([TrustedKey::for_tests(
        Purpose::Firmware,
        Generation::Release(1),
        keynum,
        key,
    )]));
    TrustSet::for_tests(Purpose::Firmware, keys)
}

const IMAGE: &str = "eveningstar-1.0.0-esp32c6.bin";

fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

/// Why the device refuses a file for an image.
#[derive(Debug)]
enum Refusal {
    /// The file doesn't verify.
    File(SigError),
    /// No one line has the image's digest.
    Line(ManifestError),
}

/// The device's verdict on `file` for the image: the name of the line with
/// its digest.
fn device(file: &[u8], trust: &TrustSet, image: &[u8]) -> Result<String, Refusal> {
    let verified = verify(file, trust).map_err(Refusal::File)?;
    let line = verified.line(&digest(image)).map_err(Refusal::Line)?;
    Ok(line.name.to_owned())
}

/// Signify's verdict on `file` for the image as `name`: `signify -C` naming
/// it, with the image's bytes in a file of that name.
fn stock(dir: &Path, file: &[u8], image: &[u8], name: &str) -> bool {
    assert!(
        !matches!(
            name,
            "." | ".." | "key.pub" | "key.sec" | "case.sig" | "SHA256" | "SHA256.sig"
        ),
        "the image can't be named {name}"
    );
    fs::write(dir.join(name), image).unwrap();
    fs::write(dir.join("case.sig"), file).unwrap();
    signify(dir, &["-C", "-p", "key.pub", "-x", "case.sig", name])
}

/// Where the device may refuse what signify accepts: forms outside the
/// device's subset, the size bound, and a digest on more than one line.
fn stricter(refusal: &Refusal) -> bool {
    matches!(
        refusal,
        Refusal::File(
            SigError::SignatureLine
                | SigError::Manifest { .. }
                | SigError::Message
                | SigError::TooLong
                | SigError::Nul
        ) | Refusal::Line(ManifestError::Ambiguous)
    )
}

/// Compares the verdicts on `file`, and returns them: the device's and
/// signify's.
fn compare(dir: &Path, trust: &TrustSet, image: &[u8], file: &[u8], case: &str) -> (bool, bool) {
    let device = device(file, trust, image);
    let name = device.as_deref().unwrap_or(IMAGE);
    let signify_ok = stock(dir, file, image, name);
    match (&device, signify_ok) {
        (Ok(_), false) => panic!("{case}: the device accepts what signify refuses"),
        (Err(refusal), true) => assert!(
            stricter(refusal),
            "{case}: signify accepts, the device refuses with {refusal:?}"
        ),
        _ => {}
    }
    (device.is_ok(), signify_ok)
}

/// `listing` signed by `signify -S -e` in `dir`.
fn signed(dir: &Path, listing: &[u8]) -> Vec<u8> {
    fs::write(dir.join("SHA256"), listing).unwrap();
    assert!(signify(
        dir,
        &[
            "-S",
            "-e",
            "-s",
            "key.sec",
            "-m",
            "SHA256",
            "-x",
            "SHA256.sig"
        ]
    ));
    fs::read(dir.join("SHA256.sig")).unwrap()
}

#[test]
fn every_mutation_of_a_signature_file_is_judged_alike() {
    if !signify_available() {
        return;
    }
    let dir = scratch("mutations");
    let trust = generated_key(&dir);
    let image = b"the image's bytes".to_vec();
    fs::write(dir.join(IMAGE), &image).unwrap();
    let line = format!("SHA256 ({IMAGE}) = {}\n", hex(&digest(&image)));
    let file = signed(&dir, line.as_bytes());

    assert_eq!(
        compare(&dir, &trust, &image, &file, "the good file"),
        (true, true)
    );

    let mut accepted = 0;
    for at in 0..file.len() {
        for change in [0x01, 0x80, 0xFF] {
            let mut mutated = file.clone();
            mutated[at] ^= change;
            let (device_ok, _) =
                compare(&dir, &trust, &image, &mutated, &format!("{at}^{change:#x}"));
            accepted += usize::from(device_ok);
        }
        for byte in *b" \n=A\r" {
            let mut mutated = file.clone();
            mutated[at] = byte;
            compare(&dir, &trust, &image, &mutated, &format!("{at}={byte}"));
        }
    }
    // Only changes inside the unsigned comment can still verify.
    assert!(accepted > 0);

    // Truncations and additions.
    for len in 0..file.len() {
        compare(
            &dir,
            &trust,
            &image,
            &file[..len],
            &format!("truncated to {len}"),
        );
    }
    let mut longer = file.clone();
    longer.extend_from_slice(b"\n");
    compare(&dir, &trust, &image, &longer, "an extra newline");
}

#[test]
fn signature_line_variants_are_judged_alike() {
    if !signify_available() {
        return;
    }
    let dir = scratch("variants");
    let trust = generated_key(&dir);
    let image = b"image".to_vec();
    fs::write(dir.join(IMAGE), &image).unwrap();
    let line = format!("SHA256 ({IMAGE}) = {}\n", hex(&digest(&image)));
    let file = signed(&dir, line.as_bytes());
    let first = file.iter().position(|b| *b == b'\n').unwrap() + 1;
    let second = first + 100;
    let (head, sig, tail) = (&file[..first], &file[first..second], &file[second..]);

    // Each case with the verdicts it gets: the device's and signify's.
    let cases: Vec<(&str, Vec<u8>, (bool, bool))> = vec![
        (
            "whitespace inside the base64",
            [head, &sig[..40], b" ", &sig[40..], tail].concat(),
            (false, true),
        ),
        (
            "a tab inside the base64",
            [head, &sig[..40], b"\t", &sig[40..], tail].concat(),
            (false, true),
        ),
        (
            "no padding",
            [head, &sig[..99], tail].concat(),
            (false, false),
        ),
        ("CRLF", [head, sig, b"\r", tail].concat(), (false, true)),
        (
            "an empty comment",
            [b"untrusted comment: \n".as_slice(), sig, tail].concat(),
            (false, false),
        ),
        (
            "a 1023-byte comment",
            [
                format!("untrusted comment: {}\n", "c".repeat(1023)).as_bytes(),
                sig,
                tail,
            ]
            .concat(),
            (true, true),
        ),
        (
            "a 1024-byte comment",
            [
                format!("untrusted comment: {}\n", "c".repeat(1024)).as_bytes(),
                sig,
                tail,
            ]
            .concat(),
            (false, false),
        ),
    ];
    for (case, file, verdicts) in cases {
        assert_eq!(
            compare(&dir, &trust, &image, &file, case),
            verdicts,
            "{case}"
        );
    }

    // S + L: signify reduces S modulo L, the device refuses it.
    let mut decoded = STANDARD.decode(sig).unwrap();
    let mut carry = 0u16;
    for (byte, add) in decoded[42..].iter_mut().zip(L) {
        let sum = u16::from(*byte) + u16::from(add) + carry;
        *byte = (sum & 0xFF) as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0);
    let malleated = [head, STANDARD.encode(&decoded).as_bytes(), tail].concat();
    assert!(
        stock(&dir, &malleated, &image, IMAGE),
        "signify accepts S + L"
    );
    assert!(
        matches!(
            device(&malleated, &trust, &image),
            Err(Refusal::File(SigError::Signature))
        ),
        "the device refuses S + L"
    );
}

#[test]
fn manifest_variants_are_judged_alike() {
    if !signify_available() {
        return;
    }
    let dir = scratch("manifests");
    let trust = generated_key(&dir);
    let image = b"image".to_vec();
    fs::write(dir.join(IMAGE), &image).unwrap();
    fs::write(dir.join("other.bin"), b"other").unwrap();
    let hash = hex(&digest(&image));
    let line = format!("SHA256 ({IMAGE}) = {hash}\n");
    let other = format!("SHA256 (other.bin) = {}\n", hex(&digest(b"other")));
    let zeros = "0".repeat(64);
    // Each case with the verdicts it gets: the device's and signify's.
    let cases = [
        ("the line", line.clone(), (true, true)),
        ("two files", format!("{line}{other}"), (true, true)),
        (
            "uppercase hex",
            format!("SHA256 ({IMAGE}) = {}\n", hash.to_uppercase()),
            (false, false),
        ),
        (
            "base64 hash",
            format!("SHA256 ({IMAGE}) = {}\n", STANDARD.encode(digest(&image))),
            (false, true),
        ),
        (
            "trailing text",
            format!("SHA256 ({IMAGE}) = {hash} trailing\n"),
            (false, true),
        ),
        (
            "double space",
            format!("SHA256 ({IMAGE})  = {hash}\n"),
            (false, true),
        ),
        (
            "a SHA512 line for another file",
            format!("{line}SHA512 (other.bin) = {}\n", "0".repeat(128)),
            (false, true),
        ),
        (
            "no final newline",
            format!("SHA256 ({IMAGE}) = {hash}"),
            (false, true),
        ),
        ("a blank line", format!("{line}\n"), (false, false)),
        ("a line twice", format!("{line}{line}"), (false, true)),
        ("another file only", other.clone(), (false, false)),
        (
            "the digest under another name",
            format!("SHA256 (renamed.bin) = {hash}\nSHA256 ({IMAGE}) = {zeros}\n"),
            (true, true),
        ),
        (
            "a wrong digest",
            format!("SHA256 ({IMAGE}) = {zeros}\n"),
            (false, false),
        ),
        (
            "garbage",
            "not a checksum line\n".to_owned(),
            (false, false),
        ),
    ];
    for (case, listing, verdicts) in cases {
        let file = signed(&dir, listing.as_bytes());
        assert_eq!(
            compare(&dir, &trust, &image, &file, case),
            verdicts,
            "{case}"
        );
    }
}

#[test]
fn wycheproof_signatures_are_judged_alike() {
    if !signify_available() {
        return;
    }
    let dir = scratch("wycheproof");
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../testdata/wycheproof-ed25519.json")).unwrap();
    let keynum = [7u8; 8];
    for group in json["testGroups"].as_array().unwrap() {
        let key = unhex(group["publicKey"]["pk"].as_str().unwrap());
        let mut decoded = b"Ed".to_vec();
        decoded.extend_from_slice(&keynum);
        decoded.extend_from_slice(&key);
        fs::write(
            dir.join("key.pub"),
            format!("untrusted comment: w\n{}\n", STANDARD.encode(&decoded)),
        )
        .unwrap();
        let key: [u8; 32] = key.try_into().unwrap();
        let keys = Box::leak(Box::new([TrustedKey::for_tests(
            Purpose::Firmware,
            Generation::Release(1),
            keynum,
            key,
        )]));
        let trust = TrustSet::for_tests(Purpose::Firmware, keys);
        for test in group["tests"].as_array().unwrap() {
            let id = test["tcId"].as_u64().unwrap();
            let message = unhex(test["msg"].as_str().unwrap());
            let signature = unhex(test["sig"].as_str().unwrap());
            if signature.len() != 64 || message.is_empty() {
                continue;
            }
            let mut decoded = b"Ed".to_vec();
            decoded.extend_from_slice(&keynum);
            decoded.extend_from_slice(&signature);
            let mut file =
                format!("untrusted comment: w\n{}\n", STANDARD.encode(&decoded)).into_bytes();
            file.extend_from_slice(&message);
            fs::write(dir.join("case.sig"), &file).unwrap();
            // The signature layer alone: signify -V -e, which doesn't parse
            // the message as a listing.
            let signify_ok = signify(
                &dir,
                &["-V", "-e", "-p", "key.pub", "-x", "case.sig", "-m", "out"],
            );
            let device_ok = match verify(&file, &trust) {
                // The manifest is read only once the signature verified.
                Ok(_) | Err(SigError::Manifest { .. }) => true,
                Err(SigError::Signature) => false,
                // Refused before the signature is checked; the unit tests
                // check these messages' signatures directly.
                Err(SigError::Nul) => continue,
                Err(error) => panic!("case {id}: refused before the signature: {error:?}"),
            };
            let flags: Vec<&str> = test["flags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|flag| flag.as_str().unwrap())
                .collect();
            if device_ok {
                assert!(
                    signify_ok,
                    "case {id}: the device accepts what signify refuses"
                );
            } else if signify_ok {
                // The documented classes: S of L or more, which signify
                // reduces modulo L, and small-order or mixed-order keys,
                // which no trust set holds.
                let documented = scalar_at_least_l(&signature[32..])
                    || flags
                        .iter()
                        .any(|flag| matches!(*flag, "SmallOrderPublicKey" | "MixedOrderPublicKey"));
                assert!(
                    documented,
                    "case {id} ({flags:?}): signify accepts, the device refuses"
                );
            }
        }
    }
}

/// Whether the little-endian scalar `s` is at least the group order L.
fn scalar_at_least_l(s: &[u8]) -> bool {
    const L: [u8; 32] = [
        0xED, 0xD3, 0xF5, 0x5C, 0x1A, 0x63, 0x12, 0x58, 0xD6, 0x9C, 0xF7, 0xA2, 0xDE, 0xF9, 0xDE,
        0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ];
    for (a, b) in s.iter().rev().zip(L.iter().rev()) {
        if a != b {
            return a > b;
        }
    }
    true
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}
