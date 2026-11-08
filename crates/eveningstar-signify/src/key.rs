//! Trusted keys, trust sets, and signify's public key files.

use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use curve25519_dalek::edwards::CompressedEdwardsY;

/// A signify key number: 8 random bytes a key and its signatures share.
pub type Keynum = [u8; 8];

/// What a key may sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Firmware images.
    Firmware,
}

/// Which key of a purpose a key is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generation {
    /// The development key, whose secret key is public.
    Development,
    /// A release key of this generation.
    Release(u16),
}

/// A key a trust set holds: its purpose, generation, keynum and Ed25519
/// public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    purpose: Purpose,
    generation: Generation,
    keynum: Keynum,
    key: [u8; 32],
}

impl TrustedKey {
    /// A trusted key. Only this crate makes one, for the trust sets it
    /// defines and, with the `test-support` feature, for tests.
    #[must_use]
    pub(crate) const fn new(
        purpose: Purpose,
        generation: Generation,
        keynum: Keynum,
        key: [u8; 32],
    ) -> Self {
        Self {
            purpose,
            generation,
            keynum,
            key,
        }
    }

    /// The key's purpose.
    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// The key's generation.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// The key's keynum.
    #[must_use]
    pub const fn keynum(&self) -> &Keynum {
        &self.keynum
    }

    /// The key's Ed25519 public key.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; 32] {
        &self.key
    }

    /// Whether this is the development key.
    #[must_use]
    pub const fn is_development(&self) -> bool {
        matches!(self.generation, Generation::Development)
    }

    /// A key a test chooses.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub const fn for_tests(
        purpose: Purpose,
        generation: Generation,
        keynum: Keynum,
        key: [u8; 32],
    ) -> Self {
        Self::new(purpose, generation, keynum, key)
    }
}

/// The keys trusted for one purpose. Only the trust sets this crate defines
/// exist, and, with the `test-support` feature, ones tests build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustSet {
    purpose: Purpose,
    keys: &'static [TrustedKey],
}

impl TrustSet {
    /// A trust set for `purpose` holding `keys`, which all have that purpose
    /// and distinct keynums.
    #[must_use]
    pub(crate) const fn new(purpose: Purpose, keys: &'static [TrustedKey]) -> Self {
        Self { purpose, keys }
    }

    /// The purpose the set's keys may sign for.
    #[must_use]
    pub const fn purpose(&self) -> Purpose {
        self.purpose
    }

    /// The keys, in the order the set lists them.
    #[must_use]
    pub const fn keys(&self) -> &'static [TrustedKey] {
        self.keys
    }

    /// The key with `keynum`, if the set holds it.
    #[must_use]
    pub fn find(&self, keynum: &Keynum) -> Option<&'static TrustedKey> {
        self.keys.iter().find(|key| key.keynum == *keynum)
    }

    /// Whether the set holds the development key.
    #[must_use]
    pub fn trusts_development_key(&self) -> bool {
        self.keys.iter().any(TrustedKey::is_development)
    }

    /// A set a test chooses.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub const fn for_tests(purpose: Purpose, keys: &'static [TrustedKey]) -> Self {
        Self::new(purpose, keys)
    }
}

/// Why a public key file doesn't read, or a key isn't one a trust set may
/// hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The file isn't a comment line and a 56-character base64 line, each
    /// ending in a newline, and nothing else.
    Format,
    /// The key isn't an Ed25519 key (`Ed`).
    Algorithm,
    /// The key isn't a valid point, or isn't encoded canonically.
    Point,
    /// The key has a small-order component.
    Weak,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Format => "not a signify public key file",
            Self::Algorithm => "not an Ed25519 key",
            Self::Point => "not a canonically encoded curve point",
            Self::Weak => "a key with a small-order component",
        })
    }
}

/// The comment line's header.
pub(crate) const COMMENT_HEADER: &[u8] = b"untrusted comment: ";

/// The longest comment signify accepts, in bytes.
pub(crate) const COMMENT_MAX: usize = 1023;

/// The keynum and Ed25519 key of a signify public key file: a comment line,
/// then the base64 of `Ed`, the keynum and the key, each line ending in a
/// newline, and nothing after.
///
/// # Errors
///
/// [`KeyError::Format`] and [`KeyError::Algorithm`].
pub fn public_key(file: &[u8]) -> Result<(Keynum, [u8; 32]), KeyError> {
    let rest = comment_line(file).ok_or(KeyError::Format)?;
    let (line, after) = split_line(rest).ok_or(KeyError::Format)?;
    if line.len() != 56 || !after.is_empty() {
        return Err(KeyError::Format);
    }
    let decoded: [u8; 42] = decode_exact(line).ok_or(KeyError::Format)?;
    let (algorithm, rest) = decoded.split_at(2);
    if algorithm != b"Ed" {
        return Err(KeyError::Algorithm);
    }
    let (keynum, key) = rest.split_at(8);
    Ok((
        keynum.try_into().map_err(|_| KeyError::Format)?,
        key.try_into().map_err(|_| KeyError::Format)?,
    ))
}

/// Checks that `key` may be trusted: a point on the curve, encoded
/// canonically, with no small-order component. Keys `signify -G` makes
/// always pass.
///
/// # Errors
///
/// [`KeyError::Point`] and [`KeyError::Weak`].
pub fn check_key(key: &[u8; 32]) -> Result<(), KeyError> {
    if !canonical_point(key) {
        return Err(KeyError::Point);
    }
    if !torsion_free(key) {
        return Err(KeyError::Weak);
    }
    Ok(())
}

/// Whether `key` decodes to a curve point that encodes back to `key`. Never
/// inlined, so the point stays out of [`check_key`]'s frame.
#[inline(never)]
fn canonical_point(key: &[u8; 32]) -> bool {
    CompressedEdwardsY(*key)
        .decompress()
        .is_some_and(|point| point.compress().0 == *key)
}

/// Whether the point `key` encodes has no small-order component. Never
/// inlined, so the point stays out of [`check_key`]'s frame.
#[inline(never)]
fn torsion_free(key: &[u8; 32]) -> bool {
    CompressedEdwardsY(*key)
        .decompress()
        .is_some_and(|point| !point.is_small_order() && point.is_torsion_free())
}

/// The `N` bytes the canonical base64 `line` decodes to, if it decodes to
/// exactly `N`. Never inlined, so the decoder stays out of its callers'
/// frames.
#[inline(never)]
pub(crate) fn decode_exact<const N: usize>(line: &[u8]) -> Option<[u8; N]> {
    // A line decoding to more than `N` bytes doesn't fit, and fails.
    let mut decoded = [0u8; N];
    match STANDARD.decode_slice(line, &mut decoded) {
        Ok(len) if len == N => Some(decoded),
        _ => None,
    }
}

/// What follows a comment line of 1 to 1023 bytes, holding no NUL, at the
/// start of `file`.
pub(crate) fn comment_line(file: &[u8]) -> Option<&[u8]> {
    let rest = file.strip_prefix(COMMENT_HEADER)?;
    let (comment, rest) = split_line(rest)?;
    if comment.is_empty() || comment.len() > COMMENT_MAX || comment.contains(&0) {
        return None;
    }
    Some(rest)
}

/// The line at the start of `bytes`, without its newline, and what follows
/// it.
pub(crate) fn split_line(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes.iter().position(|byte| *byte == b'\n')?;
    Some((bytes.get(..end)?, bytes.get(end + 1..)?))
}

#[cfg(test)]
mod tests;
