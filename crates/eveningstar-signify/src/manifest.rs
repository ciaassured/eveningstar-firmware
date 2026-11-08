//! The signed message: a SHA256 listing.

use core::fmt;

use crate::DIGEST_LEN;
use crate::file::SigError;
use crate::key::{TrustedKey, split_line};

/// The longest file name a line may give, in bytes.
const NAME_MAX: usize = 128;

/// A signature file whose signature verified under a key of a trust set, from
/// [`verify`]. Only `verify` makes one.
///
/// [`verify`]: crate::verify
#[derive(Debug)]
pub struct Verified<'s> {
    key: &'static TrustedKey,
    message: &'s [u8],
}

/// A line of the manifest: a file's name and SHA-256 digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line<'s> {
    /// The file's name.
    pub name: &'s str,
    /// The file's SHA-256 digest.
    pub digest: [u8; DIGEST_LEN],
}

/// Why no line of a verified manifest is the one asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// No line has the digest.
    NoMatch,
    /// More than one line has the digest.
    Ambiguous,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoMatch => "no line of the manifest has the digest",
            Self::Ambiguous => "more than one line of the manifest has the digest",
        })
    }
}

impl<'s> Verified<'s> {
    /// The key the signature verified under.
    #[must_use]
    pub const fn key(&self) -> &'static TrustedKey {
        self.key
    }

    /// The signed message.
    #[must_use]
    pub const fn message(&self) -> &'s [u8] {
        self.message
    }

    /// The lines of the manifest, in order.
    pub fn lines(&self) -> impl Iterator<Item = Line<'s>> + 's {
        let mut rest = self.message;
        core::iter::from_fn(move || {
            let (line, after) = split_line(rest)?;
            rest = after;
            parse_line(line)
        })
    }

    /// The one line whose digest is `digest`.
    ///
    /// # Errors
    ///
    /// [`ManifestError::NoMatch`] and [`ManifestError::Ambiguous`].
    pub fn line(&self, digest: &[u8; DIGEST_LEN]) -> Result<Line<'s>, ManifestError> {
        let mut matching = self.lines().filter(|line| line.digest == *digest);
        let first = matching.next().ok_or(ManifestError::NoMatch)?;
        if matching.next().is_some() {
            return Err(ManifestError::Ambiguous);
        }
        Ok(first)
    }
}

/// The verified `message` signed by `key`, once every one of its lines reads
/// as a SHA256 line.
pub(crate) fn read<'s>(
    message: &'s [u8],
    key: &'static TrustedKey,
) -> Result<Verified<'s>, SigError> {
    let mut rest = message;
    let mut number = 0;
    while !rest.is_empty() {
        number += 1;
        let (line, after) = split_line(rest).ok_or(SigError::Manifest { line: number })?;
        parse_line(line).ok_or(SigError::Manifest { line: number })?;
        rest = after;
    }
    Ok(Verified { key, message })
}

/// The line `SHA256 (<name>) = <64 lowercase hex>`, with single spaces and
/// a name of 1 to 128 bytes of `[A-Za-z0-9._+-]`.
fn parse_line(line: &[u8]) -> Option<Line<'_>> {
    let rest = line.strip_prefix(b"SHA256 (")?;
    let close = rest.iter().position(|byte| *byte == b')')?;
    let (name, rest) = rest.split_at(close);
    if name.is_empty() || name.len() > NAME_MAX || !name.iter().all(|byte| is_name_byte(*byte)) {
        return None;
    }
    let hex = rest.strip_prefix(b") = ")?;
    if hex.len() != 2 * DIGEST_LEN {
        return None;
    }
    let mut digest = [0u8; DIGEST_LEN];
    for (byte, pair) in digest.iter_mut().zip(hex.as_chunks::<2>().0) {
        *byte = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(Line {
        name: core::str::from_utf8(name).ok()?,
        digest,
    })
}

/// Whether `byte` may appear in a file name.
const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
}

/// The value of a lowercase hex digit.
const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
