//! The internal flash's state table: one write-state word per physical span
//! of the compiled layout.

use core::fmt;

use crate::span::Span;
use crate::state::{self, Spans, Word, WriteState};
use crate::{SECTOR_SIZE, layout};

/// The number of physical spans: the bootloader's, the table's sector, and
/// each partition.
const SPANS: usize = 2 + layout::PARTITIONS.len();

const _: () = assert!(SPANS <= Spans::MAX);

/// The physical spans of the compiled layout, by index: the bootloader's
/// span up to [`layout::TABLE_OFFSET`], the table's sector, then each entry of
/// [`layout::PARTITIONS`] in order.
const PHYSICAL: [Span; SPANS] = physical();

const fn physical() -> [Span; SPANS] {
    let mut spans = [Span::new(0, 0); SPANS];
    spans[0] = Span::new(0, layout::TABLE_OFFSET);
    spans[1] = Span::new(layout::TABLE_OFFSET, SECTOR_SIZE);
    let mut i = 0;
    while i < layout::PARTITIONS.len() {
        spans[i + 2] = Span::of(&layout::PARTITIONS[i]);
        i += 1;
    }
    spans
}

/// Whether no two physical spans share a byte, and none is empty.
const fn disjoint(spans: &[Span]) -> bool {
    let mut i = 0;
    while i < spans.len() {
        let a = spans[i];
        if a.len == 0 {
            return false;
        }
        let mut j = i + 1;
        while j < spans.len() {
            let b = spans[j];
            if a.offset < b.offset + b.len && b.offset < a.offset + a.len {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const _: () = assert!(disjoint(&PHYSICAL));

/// The write state of each physical span of the internal flash's compiled
/// layout: the bootloader's span (0x0 up to [`layout::TABLE_OFFSET`]), the
/// table's sector, and each entry of [`layout::PARTITIONS`].
///
/// One table serves one device. It is placed by its owner, for example in a
/// `static`, and borrowed mutably for as long as the handles made over it
/// live, so its handles are made once. Bytes outside every physical span have
/// no write state: no writer covers them.
pub struct State {
    words: [Word; SPANS],
}

impl State {
    /// A table in which no span has been written.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: [const { Word::new() }; SPANS],
        }
    }

    /// The indices of the physical spans that `span`, absolute, shares a
    /// byte with.
    pub(crate) fn spans_of(span: Span) -> Spans {
        PHYSICAL
            .iter()
            .enumerate()
            .filter(|(_, physical)| physical.overlaps(span))
            .fold(Spans::default(), |spans, (index, _)| spans.with(index))
    }

    /// Whether `span`, absolute, lies inside one physical span.
    pub(crate) fn inside_one(span: Span) -> bool {
        PHYSICAL.iter().any(|physical| physical.contains(span))
    }

    /// The write state of the region covering `spans`.
    pub(crate) fn load(&self, spans: Spans) -> WriteState {
        state::load(&self.words, spans)
    }

    /// Marks a write session open on `spans`.
    pub(crate) fn begin(&self, spans: Spans) {
        state::begin(&self.words, spans);
    }

    /// Marks the write session open on `spans` ended, at a new epoch when it
    /// issued a device call (`written`).
    pub(crate) fn end(&self, spans: Spans, written: bool) {
        state::end(&self.words, spans, written);
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("words", &self.words)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices(span: Span) -> Vec<usize> {
        State::spans_of(span).indices().collect()
    }

    #[test]
    fn the_physical_spans_follow_the_layout() {
        assert_eq!(PHYSICAL[0], Span::new(0, layout::TABLE_OFFSET));
        assert_eq!(PHYSICAL[1], Span::new(layout::TABLE_OFFSET, SECTOR_SIZE));
        for (physical, entry) in PHYSICAL[2..].iter().zip(layout::PARTITIONS) {
            assert_eq!(*physical, Span::from(entry));
        }
    }

    #[test]
    fn a_span_maps_to_the_physical_spans_it_overlaps() {
        assert_eq!(indices(Span::new(0, 1)), [0]);
        assert_eq!(indices(Span::new(0, 0x9000)), [0, 1]);
        assert_eq!(indices(Span::new(0x7FFF, 2)), [0, 1]);
        assert_eq!(indices(Span::from(layout::OTA_1)), [5]);
        assert_eq!(
            indices(Span::new(0, layout::FLASH_SIZE)),
            (0..SPANS).collect::<Vec<_>>()
        );
        // The gap between the otadata and the first slot has no span.
        assert_eq!(indices(Span::new(0xC000, 0x4000)), [] as [usize; 0]);
        assert_eq!(indices(Span::new(0xB000, 0x6000)), [3, 4]);
        // Nor has anything past the layout.
        assert_eq!(
            indices(Span::new(layout::FLASH_SIZE, 0x1000)),
            [] as [usize; 0]
        );
    }

    #[test]
    fn a_writer_span_lies_inside_one_physical_span() {
        assert!(State::inside_one(Span::from(layout::DIAG)));
        assert!(State::inside_one(Span::new(layout::DIAG.offset, 0x1000)));
        assert!(!State::inside_one(Span::new(0x8000, 0x2000)));
        assert!(!State::inside_one(Span::new(0xC000, 0x1000)));
    }

    #[test]
    fn a_table_starts_stable() {
        let table = State::new();
        assert!(
            table
                .load(State::spans_of(Span::new(0, layout::FLASH_SIZE)))
                .is_stable()
        );
    }
}
