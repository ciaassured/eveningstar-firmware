//! Digests of [`Bytes`] bodies, in the fields RFC 9530 defines: a
//! `Repr-Digest` of the whole body, a `Content-Digest` of the bytes an answer
//! carries, and the `Want-Content-Digest` a request asks for one with.
//!
//! [`Bytes`]: super::Bytes

use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The value of a `Repr-Digest` or `Content-Digest` header holding one
/// SHA-256 digest: `sha-256=:<digest in base64>:`, a Structured Fields
/// dictionary with one byte sequence (RFC 9530, section 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sha256Field<'a>(pub &'a [u8; 32]);

impl fmt::Display for Sha256Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 32 bytes are 44 characters of padded base64 (RFC 4648, section 4).
        let mut encoded = [0; 44];
        let len = STANDARD
            .encode_slice(self.0, &mut encoded)
            .map_err(|_| fmt::Error)?;
        let encoded = core::str::from_utf8(&encoded[..len]).map_err(|_| fmt::Error)?;
        write!(f, "sha-256=:{encoded}:")
    }
}

/// Whether a request with the `Want-Content-Digest` header `value` asks for a
/// `Content-Digest` with SHA-256: the header is a valid Structured Fields
/// dictionary (RFC 9651, section 3.2) whose `sha-256` member is an integer
/// from 1 to 10, its weight (RFC 9530, section 4).
///
/// A weight of 0 means SHA-256 isn't acceptable. Any other member is ignored,
/// and so is a header that isn't valid, as Structured Fields require, since
/// the header is only a preference.
#[must_use]
pub fn wants_sha256(value: &str) -> bool {
    let mut parser = Parser(value.as_bytes());
    let mut weight = None;
    let valid = parser.dictionary(|key, value| {
        if key == b"sha-256" {
            weight = value;
        }
    });
    valid.is_some() && weight.is_some_and(|weight| (1..=10).contains(&weight))
}

/// A Structured Fields parser over the bytes left of a field's value. Each
/// method parses one production of RFC 9651, section 4.2, or returns `None`
/// if the bytes don't hold one, in which case the field is not valid.
struct Parser<'a>(&'a [u8]);

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    fn next(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.0 = &self.0[1..];
        }
        found
    }

    fn skip(&mut self, mut skip: impl FnMut(u8) -> bool) {
        while self.peek().is_some_and(&mut skip) {
            self.0 = &self.0[1..];
        }
    }

    /// A dictionary, calling `member` with each member's key and, when its
    /// value is an integer, the integer. A later member of a key replaces an
    /// earlier one, so the last call for a key is the member that counts.
    fn dictionary(&mut self, mut member: impl FnMut(&[u8], Option<i64>)) -> Option<()> {
        self.skip(|byte| byte == b' ');
        if self.0.is_empty() {
            return Some(());
        }
        loop {
            let key = self.key()?;
            let value = if self.eat(b'=') {
                self.item_or_inner_list()?
            } else {
                // A key alone is the Boolean true.
                self.parameters()?;
                Value::Other
            };
            let integer = match value {
                Value::Integer(integer) => Some(integer),
                Value::Other => None,
            };
            member(key, integer);
            self.skip(|byte| byte == b' ' || byte == b'\t');
            if self.0.is_empty() {
                return Some(());
            }
            if !self.eat(b',') {
                return None;
            }
            self.skip(|byte| byte == b' ' || byte == b'\t');
            // A trailing comma isn't valid.
            if self.0.is_empty() {
                return None;
            }
        }
    }

    /// An item or an inner list.
    fn item_or_inner_list(&mut self) -> Option<Value> {
        if self.eat(b'(') {
            loop {
                self.skip(|byte| byte == b' ');
                if self.eat(b')') {
                    self.parameters()?;
                    return Some(Value::Other);
                }
                self.item()?;
                if !matches!(self.peek(), Some(b' ' | b')')) {
                    return None;
                }
            }
        }
        self.item()
    }

    /// An item.
    fn item(&mut self) -> Option<Value> {
        let integer = self.bare_item()?;
        self.parameters()?;
        Some(integer)
    }

    fn parameters(&mut self) -> Option<()> {
        while self.eat(b';') {
            self.skip(|byte| byte == b' ');
            self.key()?;
            if self.eat(b'=') {
                self.bare_item()?;
            }
        }
        Some(())
    }

    fn key(&mut self) -> Option<&'a [u8]> {
        let start = self.0;
        if !self
            .peek()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'*')
        {
            return None;
        }
        self.skip(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.*".contains(&byte)
        });
        Some(&start[..start.len() - self.0.len()])
    }

    /// A bare item.
    fn bare_item(&mut self) -> Option<Value> {
        match self.peek()? {
            b'-' | b'0'..=b'9' => self.number(),
            b'"' => self.string().map(|()| Value::Other),
            b':' => self.byte_sequence().map(|()| Value::Other),
            b'?' => {
                self.next();
                matches!(self.next(), Some(b'0' | b'1')).then_some(Value::Other)
            }
            b'@' => {
                self.next();
                // A date is an integer.
                self.number().map(|_| Value::Other)
            }
            byte if byte.is_ascii_alphabetic() || byte == b'*' => {
                self.skip(|byte| is_tchar(byte) || byte == b':' || byte == b'/');
                Some(Value::Other)
            }
            _ => None,
        }
    }

    /// An integer or a decimal.
    fn number(&mut self) -> Option<Value> {
        let negative = self.eat(b'-');
        let start = self.0;
        self.skip(|byte| byte.is_ascii_digit());
        let digits = &start[..start.len() - self.0.len()];
        if digits.is_empty() {
            return None;
        }
        if self.eat(b'.') {
            let after = self.0;
            self.skip(|byte| byte.is_ascii_digit());
            let fraction = after.len() - self.0.len();
            return (digits.len() <= 12 && (1..=3).contains(&fraction)).then_some(Value::Other);
        }
        if digits.len() > 15 {
            return None;
        }
        let value = digits
            .iter()
            .fold(0_i64, |value, digit| value * 10 + i64::from(digit - b'0'));
        Some(Value::Integer(if negative { -value } else { value }))
    }

    fn string(&mut self) -> Option<()> {
        self.next();
        loop {
            match self.next()? {
                b'"' => return Some(()),
                b'\\' => {
                    if !matches!(self.next()?, b'"' | b'\\') {
                        return None;
                    }
                }
                0x20..=0x7e => {}
                _ => return None,
            }
        }
    }

    fn byte_sequence(&mut self) -> Option<()> {
        self.next();
        self.skip(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte));
        self.eat(b':').then_some(())
    }
}

/// What the parser keeps of a value: an integer, or that it is something
/// else.
#[derive(Clone, Copy)]
enum Value {
    Integer(i64),
    Other,
}

/// Whether `byte` is a `tchar` (RFC 9110, section 5.6.2).
fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_sha256_digest_in_base64() {
        // The SHA-256 of no bytes, as RFC 9530's examples write it.
        let empty = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(
            Sha256Field(&empty).to_string(),
            "sha-256=:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=:"
        );
        assert_eq!(
            Sha256Field(&[0xff; 32]).to_string(),
            "sha-256=://////////////////////////////////////////8=:"
        );
    }

    #[test]
    fn wants_sha256_with_a_weight_from_1_to_10() {
        for value in [
            "sha-256=1",
            "sha-256=10",
            "sha-512=3, sha-256=10, unixsum=1",
            "sha-256=5;q=a, other",
            " sha-256=1 ",
            "sha-256=0, sha-256=4",
            "id-sha-256=1, sha-256=2",
        ] {
            assert!(wants_sha256(value), "{value}");
        }
    }

    #[test]
    fn does_not_want_sha256_otherwise() {
        for value in [
            "",
            "sha-512=10",
            "sha-256=0",
            "sha-256=11",
            "sha-256=-1",
            "sha-256",
            "sha-256=?1",
            "sha-256=1.0",
            "sha-256=\"1\"",
            "sha-256=(1 2)",
            "sha-256=4, sha-256=0",
            "SHA-256=1",
        ] {
            assert!(!wants_sha256(value), "{value}");
        }
    }

    #[test]
    fn ignores_a_header_that_is_not_a_valid_dictionary() {
        for value in [
            "sha-256=1,",
            "sha-256=1 sha-512=1",
            "sha-256=1, 512=1",
            "sha-256=1, a=\"unterminated",
            "sha-256=1, a=:abc",
            "sha-256=1, a=(1 2",
            "sha-256=1, a=1234567890123456",
            "sha-256=1;",
        ] {
            assert!(!wants_sha256(value), "{value}");
        }
    }

    #[test]
    fn reads_every_kind_of_member_around_sha256() {
        assert!(wants_sha256(
            r#"a="x\"y", b=:AAE=:, c=?0, d=tok/en:1, e=(1 "s";p=1);q, f=-2.5, g=@1, sha-256=3"#
        ));
    }
}
