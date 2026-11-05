//! Ranges and conditions of [`Bytes`] bodies: the part of a body a request's
//! `Range` and `If-Range` headers ask for, and whether its `If-Match` header
//! lets it be answered, as RFC 9110 defines them.
//!
//! [`Bytes`]: super::Bytes

use core::fmt;

/// A strong entity tag, as an `ETag` header sends it: an opaque tag of visible
/// ASCII characters other than `"`, in double quotes, such as `"3-17"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntityTag<'a>(&'a str);

impl<'a> EntityTag<'a> {
    /// `tag` as a strong entity tag, quotes included, or `None` if it isn't
    /// one.
    #[must_use]
    pub fn new(tag: &'a str) -> Option<Self> {
        let opaque = tag.strip_prefix('"')?.strip_suffix('"')?;
        // `etagc` in RFC 9110: `!`, `#` to `~`, and obs-text, which is left
        // out so the tag is ASCII.
        let valid = opaque
            .bytes()
            .all(|byte| byte == b'!' || (b'#'..=b'~').contains(&byte));
        valid.then_some(Self(tag))
    }

    /// The tag, quotes included.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

impl fmt::Display for EntityTag<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Whether a request with the `If-Match` header `value` may be answered from
/// a body whose strong entity tag is `tag` (RFC 9110, section 13.1.1).
///
/// - With no header, it may.
/// - With `*`, it may, as the body exists.
/// - With a list of entity tags, it may when one of them is strong and equal
///   to `tag`. The header compares tags strongly, so a weak tag never
///   matches, and a body without a tag matches none.
/// - With a value that isn't valid, or a list of no tags, it may not.
///
/// A request that may not is answered with
/// [`BytesError::PreconditionFailed`].
///
/// [`BytesError::PreconditionFailed`]: super::BytesError::PreconditionFailed
#[must_use]
pub fn if_match(tag: Option<EntityTag<'_>>, value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    let value = value.trim_matches(OWS);
    if value == "*" {
        return true;
    }
    // The whole list is read, so a value that isn't valid fails even after
    // a tag that matches.
    let mut matched = false;
    let mut rest = value;
    loop {
        // A list may have empty elements, which don't count.
        rest = rest.trim_start_matches(|c| c == ',' || OWS.contains(&c));
        if rest.is_empty() {
            return matched;
        }
        let (weak, after) = match rest.strip_prefix("W/") {
            Some(after) => (true, after),
            None => (false, rest),
        };
        let Some(len) = after.strip_prefix('"').and_then(|opaque| opaque.find('"')) else {
            return false;
        };
        // The quotes and the opaque tag between them.
        let (candidate, next) = after.split_at(len + 2);
        if EntityTag::new(candidate).is_none() {
            return false;
        }
        matched |= !weak && tag.is_some_and(|tag| tag.as_str() == candidate);
        rest = next.trim_start_matches(OWS);
        if !rest.is_empty() && !rest.starts_with(',') {
            return false;
        }
    }
}

/// The part of a body a request asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// The whole body.
    Whole,
    /// The bytes from `first` to `last`, both included. `first` is at most
    /// `last`, and `last` is before the body's end.
    Part {
        /// The offset of the first byte.
        first: u64,
        /// The offset of the last byte.
        last: u64,
    },
    /// No part the body has: answered with [`ErrorCode::RangeNotSatisfiable`].
    ///
    /// [`ErrorCode::RangeNotSatisfiable`]: super::ErrorCode::RangeNotSatisfiable
    Unsatisfiable,
}

impl Selection {
    /// The part of a body of `len` bytes, whose entity tag is `tag`, that a
    /// request with the `Range` header `range` and the `If-Range` header
    /// `if_range` asks for.
    ///
    /// - With no `Range`, a `Range` in a unit other than `bytes` or with an
    ///   invalid syntax, or an `If-Range` other than `tag` (a date, a weak tag,
    ///   or a strong tag that differs), the whole body.
    /// - With a single range, `bytes=first-last`, `bytes=first-` or
    ///   `bytes=-suffix_length`, that part, ending at the body's end at the
    ///   latest. A suffix of a body of no bytes is the whole body.
    /// - With a range starting at or past the body's end, a suffix of no bytes,
    ///   or several ranges, [`Unsatisfiable`](Self::Unsatisfiable).
    #[must_use]
    pub fn new(
        len: u64,
        tag: Option<EntityTag<'_>>,
        range: Option<&str>,
        if_range: Option<&str>,
    ) -> Self {
        let Some(range) = range else {
            return Self::Whole;
        };
        // Weak tags and dates never match a strong tag, and a body without a
        // tag has nothing to match.
        if let Some(if_range) = if_range
            && tag.is_none_or(|tag| tag.as_str() != if_range.trim_matches(OWS))
        {
            return Self::Whole;
        }
        match parse(range) {
            Some(Ranges::One(spec)) => spec.select(len),
            Some(Ranges::Several) => Self::Unsatisfiable,
            None => Self::Whole,
        }
    }

    /// The value of the `Content-Range` header of the answer with this part
    /// of a body of `len` bytes, or `None` for the whole body.
    #[must_use]
    pub const fn content_range(self, len: u64) -> Option<ContentRange> {
        match self {
            Self::Whole => None,
            Self::Part { first, last } => Some(ContentRange {
                part: Some((first, last)),
                len,
            }),
            Self::Unsatisfiable => Some(ContentRange { part: None, len }),
        }
    }
}

/// The value of a `Content-Range` header: `bytes <first>-<last>/<len>` for a
/// part, or `bytes */<len>` when no part is satisfiable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentRange {
    part: Option<(u64, u64)>,
    len: u64,
}

impl fmt::Display for ContentRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.part {
            Some((first, last)) => write!(f, "bytes {first}-{last}/{}", self.len),
            None => write!(f, "bytes */{}", self.len),
        }
    }
}

/// Optional whitespace in RFC 9110.
const OWS: &[char] = &[' ', '\t'];

/// The ranges of a valid `Range` header in bytes.
enum Ranges {
    One(Spec),
    Several,
}

/// One range of a `Range` header.
#[derive(Clone, Copy)]
enum Spec {
    /// `first-` or `first-last`.
    From { first: u64, last: Option<u64> },
    /// `-length`: the last `length` bytes.
    Suffix(u64),
}

impl Spec {
    /// The range in `range-spec` syntax, or `None` if it is invalid.
    fn parse(spec: &str) -> Option<Self> {
        let (first, last) = spec.split_once('-')?;
        if first.is_empty() {
            return Some(Self::Suffix(number(last)?));
        }
        let first = number(first)?;
        let last = if last.is_empty() {
            None
        } else {
            Some(number(last)?)
        };
        // A last position before the first makes the range invalid.
        if last.is_some_and(|last| last < first) {
            return None;
        }
        Some(Self::From { first, last })
    }

    /// The part of a body of `len` bytes the range covers.
    fn select(self, len: u64) -> Selection {
        match self {
            Self::From { first, .. } if first >= len => Selection::Unsatisfiable,
            Self::From { first, last } => Selection::Part {
                first,
                last: last.map_or(len - 1, |last| last.min(len - 1)),
            },
            Self::Suffix(0) => Selection::Unsatisfiable,
            Self::Suffix(_) if len == 0 => Selection::Whole,
            Self::Suffix(length) => Selection::Part {
                first: len - length.min(len),
                last: len - 1,
            },
        }
    }
}

/// The ranges of the `Range` header `value`, or `None` if it isn't in bytes or
/// its syntax is invalid.
fn parse(value: &str) -> Option<Ranges> {
    let (unit, set) = value.trim_matches(OWS).split_once('=')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let mut first = None;
    let mut several = false;
    // A list may have empty elements, which don't count.
    for element in set.split(',').map(|element| element.trim_matches(OWS)) {
        if element.is_empty() {
            continue;
        }
        let spec = Spec::parse(element)?;
        if first.is_some() {
            several = true;
        } else {
            first = Some(spec);
        }
    }
    let first = first?;
    Some(if several {
        Ranges::Several
    } else {
        Ranges::One(first)
    })
}

/// The decimal number `digits`, or `None` if it isn't one. Numbers too large
/// for a `u64` are `u64::MAX`, past the end of any body.
fn number(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(digits.bytes().fold(0_u64, |number, digit| {
        number
            .saturating_mul(10)
            .saturating_add(u64::from(digit - b'0'))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_match_without_a_header_or_with_a_star_passes() {
        assert!(if_match(tag(), None));
        assert!(if_match(tag(), Some("*")));
        assert!(if_match(tag(), Some(" * ")));
        assert!(if_match(None, Some("*")));
    }

    #[test]
    fn if_match_passes_when_one_strong_tag_of_the_list_is_the_body_s() {
        assert!(if_match(tag(), Some(TAG)));
        assert!(if_match(tag(), Some(r#""other", "7f3a-2""#)));
        assert!(if_match(tag(), Some(r#",, "other" ,"7f3a-2" ,"#)));
        // `,` is a valid character of an opaque tag.
        let comma = EntityTag::new(r#""a,b""#);
        assert!(if_match(comma, Some(r#""a,b""#)));
        assert!(!if_match(comma, Some(r#""a""#)));
    }

    #[test]
    fn if_match_fails_for_other_weak_or_no_tags() {
        assert!(!if_match(tag(), Some(r#""7f3a-3""#)));
        assert!(!if_match(tag(), Some(r#"W/"7f3a-2""#)));
        assert!(!if_match(None, Some(TAG)));
        assert!(!if_match(tag(), Some("")));
        assert!(!if_match(tag(), Some(" , ")));
    }

    #[test]
    fn if_match_fails_for_values_that_are_not_valid() {
        for value in [
            "7f3a-2",
            r#""7f3a-2"#,
            r#""7f3a-2" "other""#,
            r#""7f3a-2"x"#,
            "**",
            r#"*, "7f3a-2""#,
            "\"7f3a 2\"",
        ] {
            assert!(!if_match(tag(), Some(value)), "{value}");
        }
    }

    const TAG: &str = r#""7f3a-2""#;

    fn tag() -> Option<EntityTag<'static>> {
        EntityTag::new(TAG)
    }

    /// The part of a body of `len` bytes, with [`TAG`], that `range` asks for.
    fn select(len: u64, range: &str) -> Selection {
        Selection::new(len, tag(), Some(range), None)
    }

    fn part(first: u64, last: u64) -> Selection {
        Selection::Part { first, last }
    }

    #[test]
    fn without_a_range_the_whole_body() {
        assert_eq!(Selection::new(10, tag(), None, None), Selection::Whole);
        assert_eq!(Selection::new(10, None, None, Some(TAG)), Selection::Whole);
        assert_eq!(Selection::new(0, None, None, None), Selection::Whole);
    }

    #[test]
    fn selects_closed_ranges() {
        assert_eq!(select(1000, "bytes=0-499"), part(0, 499));
        assert_eq!(select(1000, "bytes=500-999"), part(500, 999));
        assert_eq!(select(1000, "bytes=7-7"), part(7, 7));
        assert_eq!(select(1000, "bytes=0-0"), part(0, 0));
    }

    #[test]
    fn clamps_the_last_position_to_the_end() {
        assert_eq!(select(1000, "bytes=500-1000"), part(500, 999));
        assert_eq!(select(1000, "bytes=999-5000"), part(999, 999));
        assert_eq!(
            select(1000, "bytes=0-99999999999999999999999999"),
            part(0, 999)
        );
    }

    #[test]
    fn selects_open_ended_ranges() {
        assert_eq!(select(1000, "bytes=0-"), part(0, 999));
        assert_eq!(select(1000, "bytes=600-"), part(600, 999));
        assert_eq!(select(1000, "bytes=999-"), part(999, 999));
    }

    #[test]
    fn selects_suffixes() {
        assert_eq!(select(1000, "bytes=-1"), part(999, 999));
        assert_eq!(select(1000, "bytes=-300"), part(700, 999));
        assert_eq!(select(1000, "bytes=-1000"), part(0, 999));
    }

    #[test]
    fn a_suffix_longer_than_the_body_is_the_body() {
        assert_eq!(select(1000, "bytes=-1001"), part(0, 999));
        assert_eq!(
            select(1000, "bytes=-99999999999999999999999999"),
            part(0, 999)
        );
    }

    #[test]
    fn ranges_past_the_end_are_unsatisfiable() {
        assert_eq!(select(1000, "bytes=1000-"), Selection::Unsatisfiable);
        assert_eq!(select(1000, "bytes=1000-1000"), Selection::Unsatisfiable);
        assert_eq!(select(1000, "bytes=5000-9000"), Selection::Unsatisfiable);
        assert_eq!(
            select(1000, "bytes=99999999999999999999999999-"),
            Selection::Unsatisfiable
        );
    }

    #[test]
    fn an_empty_suffix_is_unsatisfiable() {
        assert_eq!(select(1000, "bytes=-0"), Selection::Unsatisfiable);
        assert_eq!(select(0, "bytes=-0"), Selection::Unsatisfiable);
    }

    #[test]
    fn several_ranges_are_unsatisfiable() {
        for range in [
            "bytes=0-1,5-6",
            "bytes=0-499, 500-999",
            "bytes=-1,-1",
            "bytes=0-0,5000-",
            "bytes=0-,0-",
        ] {
            assert_eq!(select(1000, range), Selection::Unsatisfiable, "{range}");
        }
    }

    #[test]
    fn empty_list_elements_are_skipped() {
        assert_eq!(select(1000, "bytes=0-9,"), part(0, 9));
        assert_eq!(select(1000, "bytes=, ,0-9"), part(0, 9));
        assert_eq!(select(1000, "bytes= 0-9 "), part(0, 9));
    }

    #[test]
    fn a_body_of_no_bytes_has_no_part() {
        assert_eq!(select(0, "bytes=0-"), Selection::Unsatisfiable);
        assert_eq!(select(0, "bytes=0-0"), Selection::Unsatisfiable);
        assert_eq!(select(0, "bytes=-5"), Selection::Whole);
    }

    #[test]
    fn the_unit_is_case_insensitive() {
        assert_eq!(select(1000, "Bytes=0-9"), part(0, 9));
        assert_eq!(select(1000, "BYTES=0-9"), part(0, 9));
        assert_eq!(select(1000, "  bytes=0-9\t"), part(0, 9));
    }

    #[test]
    fn invalid_ranges_are_ignored() {
        for range in [
            "",
            "bytes",
            "bytes=",
            "bytes=,",
            "bytes=-",
            "bytes=5-3",
            "bytes=a-b",
            "bytes=0-9x",
            "bytes=+5-9",
            "bytes=5-+9",
            "bytes=--5",
            "bytes=0--5",
            "bytes=0-5-",
            "bytes=0 - 5",
            "bytes = 0-5",
            "bytes=0x10-",
            "bytes=0-1,5-3",
            "bytes=0-1,x",
            "items=0-5",
            "0-5",
            "bytes=٣-٥",
        ] {
            assert_eq!(select(1000, range), Selection::Whole, "{range:?}");
        }
    }

    #[test]
    fn a_matching_if_range_applies_the_range() {
        let range = Some("bytes=10-19");
        assert_eq!(Selection::new(100, tag(), range, Some(TAG)), part(10, 19));
        let padded = format!(" {TAG}\t");
        assert_eq!(
            Selection::new(100, tag(), range, Some(&padded)),
            part(10, 19)
        );
        // Past the end, the range still applies.
        assert_eq!(
            Selection::new(100, tag(), Some("bytes=100-"), Some(TAG)),
            Selection::Unsatisfiable
        );
    }

    #[test]
    fn a_stale_if_range_answers_the_whole_body() {
        let range = Some("bytes=10-19");
        for if_range in [
            r#""7f3a-1""#,
            r#""7F3A-2""#,
            r#""7f3a-2"#,
            "7f3a-2",
            "",
            r#""""#,
        ] {
            assert_eq!(
                Selection::new(100, tag(), range, Some(if_range)),
                Selection::Whole,
                "{if_range}"
            );
        }
        // Before an unsatisfiable range is looked at.
        assert_eq!(
            Selection::new(100, tag(), Some("bytes=500-"), Some(r#""old""#)),
            Selection::Whole
        );
        assert_eq!(
            Selection::new(100, tag(), Some("bytes=0-1,4-5"), Some(r#""old""#)),
            Selection::Whole
        );
    }

    #[test]
    fn a_weak_if_range_never_matches() {
        let weak = format!("W/{TAG}");
        assert_eq!(
            Selection::new(100, tag(), Some("bytes=10-19"), Some(&weak)),
            Selection::Whole
        );
    }

    #[test]
    fn a_dated_if_range_answers_the_whole_body() {
        assert_eq!(
            Selection::new(
                100,
                tag(),
                Some("bytes=10-19"),
                Some("Sat, 31 Oct 2026 10:12:44 GMT")
            ),
            Selection::Whole
        );
    }

    #[test]
    fn if_range_without_a_tag_answers_the_whole_body() {
        assert_eq!(
            Selection::new(100, None, Some("bytes=10-19"), Some(TAG)),
            Selection::Whole
        );
        assert_eq!(
            Selection::new(100, None, Some("bytes=10-19"), None),
            part(10, 19)
        );
    }

    #[test]
    fn writes_content_ranges() {
        let range =
            |selection: Selection, len| selection.content_range(len).map(|range| range.to_string());
        assert_eq!(range(part(0, 499), 1000).unwrap(), "bytes 0-499/1000");
        assert_eq!(range(part(7, 7), 8).unwrap(), "bytes 7-7/8");
        assert_eq!(
            range(Selection::Unsatisfiable, 1000).unwrap(),
            "bytes */1000"
        );
        assert_eq!(range(Selection::Unsatisfiable, 0).unwrap(), "bytes */0");
        assert_eq!(range(Selection::Whole, 1000), None);
    }

    #[test]
    fn entity_tags_are_strong_and_quoted() {
        for tag in [r#""""#, r#""a""#, TAG, r#""!#~""#, r#""W/x""#] {
            assert_eq!(EntityTag::new(tag).map(EntityTag::as_str), Some(tag));
        }
        for tag in [
            "",
            "\"",
            "abc",
            r#"W/"abc""#,
            r#""a"b""#,
            r#""a b""#,
            "\"a\r\nb\"",
            "\"a\tb\"",
            "\"é\"",
            r#" "abc""#,
        ] {
            assert_eq!(EntityTag::new(tag), None, "{tag:?}");
        }
    }
}
