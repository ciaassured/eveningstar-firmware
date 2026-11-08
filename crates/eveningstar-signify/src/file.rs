//! Reading and verifying a signature file.

use core::fmt;

use ed25519_dalek::{Signature, VerifyingKey};

use crate::key::{Keynum, TrustSet, comment_line, decode_exact, split_line};
use crate::manifest::{self, Verified};

/// The largest signature file accepted, in bytes.
pub const FILE_MAX: usize = 4096;

/// The largest signed message accepted, in bytes.
pub const MESSAGE_MAX: usize = 2048;

/// Length of the signature line's base64, in characters.
const SIGNATURE_BASE64_LEN: usize = 100;

/// Length of the decoded signature: `Ed`, the keynum, R and S.
const SIGNATURE_LEN: usize = 74;

/// Why a signature file is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigError {
    /// Longer than 4096 bytes.
    TooLong,
    /// Holds a NUL byte.
    Nul,
    /// The first line isn't `untrusted comment: ` and 1 to 1023 bytes.
    Comment,
    /// The second line isn't 100 characters of canonical base64 decoding to
    /// 74 bytes.
    SignatureLine,
    /// The signature isn't an Ed25519 signature (`Ed`).
    Algorithm,
    /// No trusted key has the signature's keynum.
    UnknownKey {
        /// The signature's keynum.
        keynum: Keynum,
    },
    /// The signature doesn't verify under the key with its keynum.
    Signature,
    /// The signed message is empty or longer than 2048 bytes.
    Message,
    /// A line of the signed message isn't a SHA256 line.
    Manifest {
        /// The line's number in the message, from 1.
        line: usize,
    },
}

impl fmt::Display for SigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => f.write_str("longer than 4096 bytes"),
            Self::Nul => f.write_str("holds a NUL byte"),
            Self::Comment => f.write_str("the first line isn't an untrusted comment"),
            Self::SignatureLine => f.write_str("the second line isn't a signature"),
            Self::Algorithm => f.write_str("not an Ed25519 signature"),
            Self::UnknownKey { .. } => f.write_str("signed by a key that isn't trusted"),
            Self::Signature => f.write_str("the signature doesn't verify"),
            Self::Message => f.write_str("the signed message is empty or too long"),
            Self::Manifest { line } => write!(f, "line {line} of the manifest doesn't read"),
        }
    }
}

/// Verifies the signature file `file` under `trust`: its form as the crate
/// describes, then the signature over the message under the trusted key with
/// its keynum, then every line of the message as a SHA256 line.
///
/// # Errors
///
/// The first [`SigError`] found, in that order.
pub fn verify<'s>(file: &'s [u8], trust: &TrustSet) -> Result<Verified<'s>, SigError> {
    let mut decoded = [0u8; SIGNATURE_LEN];
    let message = parse(file, &mut decoded)?;
    let (keynum, signature) = split_signature(&decoded);
    let key = trust.find(&keynum).ok_or(SigError::UnknownKey { keynum })?;
    if !ed25519(key.public_key(), message, signature) {
        return Err(SigError::Signature);
    }
    manifest::read(message, key)
}

/// The message of the signature file `file`, once its form is checked, with
/// the signature line's 74 bytes decoded into `decoded`. Never inlined, so
/// the decoding stays out of [`verify`]'s frame.
#[inline(never)]
fn parse<'s>(file: &'s [u8], decoded: &mut [u8; SIGNATURE_LEN]) -> Result<&'s [u8], SigError> {
    if file.len() > FILE_MAX {
        return Err(SigError::TooLong);
    }
    if file.contains(&0) {
        return Err(SigError::Nul);
    }
    let rest = comment_line(file).ok_or(SigError::Comment)?;
    let (line, message) = split_line(rest).ok_or(SigError::SignatureLine)?;
    if line.len() != SIGNATURE_BASE64_LEN {
        return Err(SigError::SignatureLine);
    }
    *decoded = decode_exact(line).ok_or(SigError::SignatureLine)?;
    if !decoded.starts_with(b"Ed") {
        return Err(SigError::Algorithm);
    }
    if message.is_empty() || message.len() > MESSAGE_MAX {
        return Err(SigError::Message);
    }
    Ok(message)
}

/// The keynum and the signature of the decoded signature line `decoded`.
fn split_signature(decoded: &[u8; SIGNATURE_LEN]) -> (Keynum, &[u8; 64]) {
    let [_, _, k0, k1, k2, k3, k4, k5, k6, k7, signature @ ..] = decoded;
    ([*k0, *k1, *k2, *k3, *k4, *k5, *k6, *k7], signature)
}

/// Whether `signature` is a valid Ed25519 signature of `message` under the
/// public key `key`, by ed25519-dalek's `verify_strict`. Never inlined, so
/// the curve arithmetic stays out of its callers' frames.
#[inline(never)]
pub(crate) fn ed25519(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(key) else {
        return false;
    };
    key.verify_strict(message, &Signature::from_bytes(signature))
        .is_ok()
}

#[cfg(test)]
mod tests;
