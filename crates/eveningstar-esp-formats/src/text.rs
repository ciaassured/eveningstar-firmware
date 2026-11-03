//! Fixed-size, NUL-padded text fields, as the table and image formats store
//! labels, names and versions.

use core::fmt;

/// A text field of `N` bytes, kept as the raw bytes read from the flash.
///
/// The text ends at the first NUL byte, or fills all `N` bytes when there is
/// none. The bytes need not be UTF-8.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Text<const N: usize>([u8; N]);

impl<const N: usize> Text<N> {
    /// The field holding exactly `raw`.
    #[must_use]
    pub const fn from_raw(raw: [u8; N]) -> Self {
        Self(raw)
    }

    /// All `N` bytes, including the NUL padding and anything after it.
    #[must_use]
    pub const fn raw(&self) -> &[u8; N] {
        &self.0
    }

    /// The text: the bytes before the first NUL.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        let len = self.0.iter().position(|&b| b == 0).unwrap_or(N);
        self.0.get(..len).unwrap_or(&[])
    }

    /// The text as a string, or `None` when it isn't UTF-8.
    #[must_use]
    pub fn to_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// Whether the field holds `text` followed by NUL bytes only, which is how
    /// the formats write a string. A `text` longer than `N` never matches.
    #[must_use]
    pub fn is_padded(&self, text: &str) -> bool {
        let text = text.as_bytes();
        match (self.0.get(..text.len()), self.0.get(text.len()..)) {
            (Some(head), Some(tail)) => head == text && tail.iter().all(|&b| b == 0),
            _ => false,
        }
    }
}

impl<const N: usize> fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_str() {
            Some(text) => fmt::Debug::fmt(text, f),
            None => write!(f, "b\"{}\"", self.as_bytes().escape_ascii()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text<const N: usize>(bytes: &[u8]) -> Text<N> {
        let mut raw = [0; N];
        raw[..bytes.len()].copy_from_slice(bytes);
        Text::from_raw(raw)
    }

    #[test]
    fn text_ends_at_the_first_nul() {
        let mut raw = [0; 8];
        raw[..3].copy_from_slice(b"abc");
        raw[4] = b'x';
        let t = Text::from_raw(raw);
        assert_eq!(t.as_bytes(), b"abc");
        assert_eq!(t.to_str(), Some("abc"));
        assert_eq!(t.raw(), &raw);
        assert!(!t.is_padded("abc"), "a byte after the NUL");
    }

    #[test]
    fn text_without_a_nul_fills_the_field() {
        let t: Text<4> = text(b"abcd");
        assert_eq!(t.as_bytes(), b"abcd");
        assert_eq!(t.to_str(), Some("abcd"));
        assert!(t.is_padded("abcd"));
        assert!(!t.is_padded("abc"));
        assert!(!t.is_padded("abcde"));
    }

    #[test]
    fn padded_text_matches_only_itself() {
        let t: Text<8> = text(b"ota_0");
        assert!(t.is_padded("ota_0"));
        assert!(!t.is_padded("ota_"));
        assert!(!t.is_padded("ota_00"));
        assert!(!t.is_padded(""));
        assert!(Text::<8>::from_raw([0; 8]).is_padded(""));
    }

    #[test]
    fn text_that_isnt_utf8_is_kept_as_bytes() {
        let t: Text<4> = text(&[0xff, b'a', 0xc3]);
        assert_eq!(t.as_bytes(), &[0xff, b'a', 0xc3]);
        assert_eq!(t.to_str(), None);
        assert_eq!(std::format!("{t:?}"), r#"b"\xffa\xc3""#);
        assert_eq!(std::format!("{:?}", text::<4>(b"ab")), r#""ab""#);
    }
}
