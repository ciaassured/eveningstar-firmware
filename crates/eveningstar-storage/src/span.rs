//! Spans: extents of a flash, a region or a view, as an offset and a length.

use crate::Entry;

/// A span inside a flash, a region or a view: `Copy`, unlike `Range<u32>`.
///
/// Every operation uses checked arithmetic: a span whose end doesn't fit in
/// a `u32` lies inside nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    /// Offset of the first byte.
    pub offset: u32,
    /// Length in bytes.
    pub len: u32,
}

impl Span {
    /// The `len` bytes at `offset`.
    #[must_use]
    pub const fn new(offset: u32, len: u32) -> Self {
        Self { offset, len }
    }

    /// The span of a partition: its offset and size.
    #[must_use]
    pub const fn of(entry: &Entry) -> Self {
        Self::new(entry.offset, entry.size)
    }

    /// Offset of the first byte after the span, or `None` when it doesn't fit
    /// in a `u32`.
    #[must_use]
    pub const fn end(self) -> Option<u32> {
        self.offset.checked_add(self.len)
    }

    /// Whether the span has no bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Whether `inner` lies inside the span. An empty `inner` lies inside it
    /// when its offset is inside the span or at its end.
    #[must_use]
    pub fn contains(self, inner: Self) -> bool {
        match (self.end(), inner.end()) {
            (Some(end), Some(inner_end)) => self.offset <= inner.offset && inner_end <= end,
            _ => false,
        }
    }

    /// Whether the span and `other` share a byte. An empty span shares none,
    /// except with a span that has bytes on both sides of its offset. A span
    /// whose end doesn't fit in a `u32` counts as running up to `u32::MAX`.
    #[must_use]
    pub fn overlaps(self, other: Self) -> bool {
        let end = self.end().unwrap_or(u32::MAX);
        let other_end = other.end().unwrap_or(u32::MAX);
        self.offset < other_end && other.offset < end
    }

    /// The span at `relative`, an offset and a length counted from the start
    /// of this one, in the coordinates this one is in; `None` when it doesn't
    /// lie inside this one.
    #[must_use]
    pub fn inner(self, relative: Self) -> Option<Self> {
        let end = relative.end()?;
        if end > self.len {
            return None;
        }
        Some(Self::new(
            self.offset.checked_add(relative.offset)?,
            relative.len,
        ))
    }
}

impl From<Entry> for Span {
    fn from(entry: Entry) -> Self {
        Self::of(&entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_is_checked() {
        assert_eq!(Span::new(0x1000, 0x100).end(), Some(0x1100));
        assert_eq!(Span::new(u32::MAX, 0).end(), Some(u32::MAX));
        assert_eq!(Span::new(u32::MAX, 1).end(), None);
        assert_eq!(Span::new(1, u32::MAX).end(), None);
    }

    #[test]
    fn a_span_contains_the_spans_inside_it() {
        let outer = Span::new(0x1000, 0x1000);
        assert!(outer.contains(outer));
        assert!(outer.contains(Span::new(0x1000, 0)));
        assert!(outer.contains(Span::new(0x2000, 0)));
        assert!(outer.contains(Span::new(0x1800, 0x800)));
        assert!(!outer.contains(Span::new(0x0FFF, 1)));
        assert!(!outer.contains(Span::new(0x1800, 0x801)));
        assert!(!outer.contains(Span::new(0x2001, 0)));
        assert!(!outer.contains(Span::new(0x1800, u32::MAX)));
        assert!(!Span::new(u32::MAX, 1).contains(Span::new(u32::MAX, 0)));
    }

    #[test]
    fn spans_overlap_when_they_share_a_byte() {
        let a = Span::new(0x1000, 0x1000);
        assert!(a.overlaps(Span::new(0x1FFF, 1)));
        assert!(a.overlaps(Span::new(0, 0x1001)));
        assert!(a.overlaps(Span::new(0x1800, u32::MAX)));
        assert!(!a.overlaps(Span::new(0x2000, 0x1000)));
        assert!(!a.overlaps(Span::new(0, 0x1000)));
        assert!(!a.overlaps(Span::new(0x1000, 0)));
        assert!(!Span::new(0x1800, 0).overlaps(Span::new(0x1800, 0x10)));
        // An empty span strictly inside another counts as overlapping it.
        assert!(Span::new(0x1800, 0).overlaps(a));
        assert!(a.overlaps(Span::new(0x1800, 0)));
    }

    #[test]
    fn an_inner_span_is_counted_from_the_start() {
        let outer = Span::new(0x1000, 0x100);
        assert_eq!(
            outer.inner(Span::new(0x10, 0x20)),
            Some(Span::new(0x1010, 0x20))
        );
        assert_eq!(outer.inner(Span::new(0x100, 0)), Some(Span::new(0x1100, 0)));
        assert_eq!(outer.inner(Span::new(0xF0, 0x11)), None);
        assert_eq!(outer.inner(Span::new(0x101, 0)), None);
        assert_eq!(outer.inner(Span::new(1, u32::MAX)), None);
        assert_eq!(
            Span::new(u32::MAX, 0x10).inner(Span::new(1, 0)),
            None,
            "the absolute offset overflows"
        );
    }

    #[test]
    fn an_entry_gives_its_span() {
        assert_eq!(
            Span::from(crate::layout::OTA_0),
            Span::new(0x1_0000, 0x30_0000)
        );
    }
}
