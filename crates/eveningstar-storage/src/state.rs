//! Write states: the word each span of a device keeps, and the epochs and
//! states it decodes to. No other module sees the words' numbers.
//!
//! A word is a sequence lock (seqlock): even while no write session is open
//! on its span, odd while one is. Its upper 31 bits count the sessions that
//! issued a device call, so a session that issued one ends with a new value,
//! and one that issued none ends with the value it started from. A reader
//! loads the word before and after it reads, and discards what it read unless
//! both loads see the same even value.
//!
//! Each word has at most one session open at a time, as each span has at most
//! one writer and a session borrows its writer mutably, so only that session
//! stores to the word while it is open: its loads and stores need no
//! read-modify-write.
//!
//! Every load and store is `SeqCst`. The argument that a read whose loads
//! agree saw bytes of one epoch also rests on two facts, stated where the
//! handles rely on them: every device call is a compiler and hardware
//! barrier, so no device call moves across a load or a store of a word; and
//! one core reaches the flash, so a session's device calls and stores are
//! seen in the order it makes them.
//!
//! A counter that would pass its last value leaves its word odd for good:
//! the span then reads as written to, and every snapshot of it fails, rather
//! than an epoch repeating.

use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

/// The word of a span in which a counter ran out: odd, so the span stays
/// written to, and never produced by a session's start, whose odd words are
/// at most `2 * LAST_COUNT + 1`.
const EXHAUSTED: u32 = u32::MAX;

/// The last count a stable word holds. A session that issues a device call at
/// this count leaves its word [`EXHAUSTED`].
const LAST_COUNT: u32 = (u32::MAX >> 1) - 1;

/// One span's write state, as a seqlock word.
pub(crate) struct Word(AtomicU32);

impl Word {
    /// A span no session has written: stable, at count 0.
    pub(crate) const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// A word holding `raw`, for the tests of the counter's last values.
    #[cfg(test)]
    const fn from_raw(raw: u32) -> Self {
        Self(AtomicU32::new(raw))
    }

    /// The count, and whether a session is open.
    fn load(&self) -> (u32, bool) {
        let word = self.0.load(Ordering::SeqCst);
        (word >> 1, word & 1 == 1)
    }

    /// Marks a session open: a stable word becomes odd at the same count.
    fn begin(&self) {
        let word = self.0.load(Ordering::SeqCst);
        if word & 1 == 0 {
            self.0.store(word | 1, Ordering::SeqCst);
        }
    }

    /// Marks the open session ended: stable at the next count when it issued
    /// a device call (`written`), else at the count it started from. An
    /// exhausted word stays as it is, and so does one whose count can't
    /// advance, which becomes exhausted.
    fn end(&self, written: bool) {
        let word = self.0.load(Ordering::SeqCst);
        if word == EXHAUSTED || word & 1 == 0 {
            return;
        }
        let count = word >> 1;
        let next = if !written {
            count << 1
        } else if count < LAST_COUNT {
            (count + 1) << 1
        } else {
            EXHAUSTED
        };
        self.0.store(next, Ordering::SeqCst);
    }
}

impl fmt::Debug for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (count, writing) = self.load();
        f.debug_struct("Word")
            .field("count", &count)
            .field("writing", &writing)
            .finish()
    }
}

/// A set of span indices of a device's state table, at most 32.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Spans(u32);

impl Spans {
    /// The most spans a device's table has.
    pub(crate) const MAX: usize = 32;

    /// The set holding the spans of `self` and `index`, which is below
    /// [`MAX`](Self::MAX).
    pub(crate) const fn with(self, index: usize) -> Self {
        if index < Self::MAX {
            Self(self.0 | (1 << index))
        } else {
            self
        }
    }

    /// The indices in the set, in increasing order.
    pub(crate) fn indices(self) -> impl Iterator<Item = usize> {
        (0..Self::MAX).filter(move |index| self.0 & (1 << index) != 0)
    }
}

/// A region's epoch: which bytes it holds, among those it held this boot.
///
/// `Epoch` values are opaque: they compare equal, format for an entity tag,
/// and are made only by the storage crate. A region's epoch changes whenever
/// a write session that issued a device call ends on a span it covers, and
/// never returns to an earlier value within a boot. Epochs of regions over
/// different spans never compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Epoch {
    /// The spans of the region.
    spans: Spans,
    /// The sum of the spans' counts.
    count: u64,
}

impl fmt::Display for Epoch {
    /// The epoch as a decimal number, which tells it apart from the region's
    /// other epochs this boot.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.count)
    }
}

/// A region's write state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteState {
    /// No write session is open. The region's bytes are those of this epoch.
    Stable(Epoch),
    /// A write session is open. It started from this epoch.
    Writing(Epoch),
}

impl WriteState {
    /// The epoch the region is at, or the one its open session started from.
    #[must_use]
    pub const fn epoch(self) -> Epoch {
        match self {
            Self::Stable(epoch) | Self::Writing(epoch) => epoch,
        }
    }

    /// Whether no write session is open.
    #[must_use]
    pub const fn is_stable(self) -> bool {
        matches!(self, Self::Stable(_))
    }
}

/// The write state of the region covering the spans `spans` of `words`:
/// `Writing` while any of them has a session open, else `Stable`. Its epoch
/// is the sum of their counts.
///
/// The words are loaded one after the other, not at one instant. Each count
/// only grows, so two loads of the same spans that both return `Stable` with
/// the same epoch saw every span stable at the same count both times: had any
/// count grown in between, the sum would have grown with it.
pub(crate) fn load(words: &[Word], spans: Spans) -> WriteState {
    let mut count = 0_u64;
    let mut writing = false;
    for word in spans.indices().filter_map(|index| words.get(index)) {
        let (span_count, span_writing) = word.load();
        count = count.saturating_add(u64::from(span_count));
        writing |= span_writing;
    }
    let epoch = Epoch { spans, count };
    if writing {
        WriteState::Writing(epoch)
    } else {
        WriteState::Stable(epoch)
    }
}

/// Marks a session open on each span of `spans`.
pub(crate) fn begin(words: &[Word], spans: Spans) {
    for word in spans.indices().filter_map(|index| words.get(index)) {
        word.begin();
    }
}

/// Marks the session open on each span of `spans` ended, at a new epoch when
/// it issued a device call (`written`).
pub(crate) fn end(words: &[Word], spans: Spans, written: bool) {
    for word in spans.indices().filter_map(|index| words.get(index)) {
        word.end(written);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> Spans {
        Spans::default().with(0)
    }

    fn session(words: &[Word], spans: Spans, written: bool) {
        begin(words, spans);
        end(words, spans, written);
    }

    #[test]
    fn a_new_word_is_stable_at_its_first_epoch() {
        let words = [Word::new()];
        let state = load(&words, one());
        assert!(state.is_stable());
        assert_eq!(state.epoch().to_string(), "0");
    }

    #[test]
    fn a_session_is_writing_from_the_epoch_it_started_at() {
        let words = [Word::new()];
        let before = load(&words, one());
        begin(&words, one());
        assert_eq!(load(&words, one()), WriteState::Writing(before.epoch()));
    }

    #[test]
    fn a_session_that_wrote_ends_at_a_new_epoch() {
        let words = [Word::new()];
        let mut seen = vec![load(&words, one()).epoch()];
        for _ in 0..100 {
            session(&words, one(), true);
            let state = load(&words, one());
            assert!(state.is_stable());
            assert!(!seen.contains(&state.epoch()), "{seen:?}");
            seen.push(state.epoch());
        }
    }

    #[test]
    fn a_session_that_didnt_write_ends_at_its_epoch() {
        let words = [Word::new()];
        session(&words, one(), true);
        let before = load(&words, one());
        session(&words, one(), false);
        assert_eq!(load(&words, one()), before);
    }

    #[test]
    fn the_last_count_ends_written_to_for_good() {
        let words = [Word::from_raw(LAST_COUNT << 1)];
        let last = load(&words, one());
        assert!(last.is_stable());

        // A session that writes nothing at the last count still ends stable.
        session(&words, one(), false);
        assert_eq!(load(&words, one()), last);

        session(&words, one(), true);
        let exhausted = load(&words, one());
        assert!(!exhausted.is_stable());
        assert_ne!(exhausted.epoch(), last.epoch());

        // Nothing brings it back, whatever later sessions do.
        session(&words, one(), false);
        assert_eq!(load(&words, one()), exhausted);
        session(&words, one(), true);
        assert_eq!(load(&words, one()), exhausted);
    }

    #[test]
    fn the_counter_doesnt_wrap_on_the_last_counts() {
        let words = [Word::from_raw((LAST_COUNT - 1) << 1)];
        let mut seen = vec![load(&words, one()).epoch()];
        session(&words, one(), true);
        let last = load(&words, one());
        assert!(last.is_stable());
        assert!(!seen.contains(&last.epoch()));
        seen.push(last.epoch());
        session(&words, one(), true);
        let state = load(&words, one());
        assert!(!state.is_stable());
    }

    #[test]
    fn a_region_over_several_spans_is_writing_while_any_is() {
        let words = [Word::new(), Word::new(), Word::new()];
        let both = Spans::default().with(0).with(2);
        let before = load(&words, both);

        // A session on a span outside the region changes nothing.
        session(&words, Spans::default().with(1), true);
        assert_eq!(load(&words, both), before);

        begin(&words, Spans::default().with(2));
        assert_eq!(load(&words, both), WriteState::Writing(before.epoch()));
        end(&words, Spans::default().with(2), true);
        let after = load(&words, both);
        assert!(after.is_stable());
        assert_ne!(after.epoch(), before.epoch());
    }

    #[test]
    fn epochs_of_different_spans_differ() {
        let words = [Word::new(), Word::new()];
        assert_ne!(
            load(&words, Spans::default().with(0)),
            load(&words, Spans::default().with(1))
        );
    }

    #[test]
    fn a_set_lists_its_indices_in_order() {
        let spans = Spans::default().with(31).with(3).with(0).with(32);
        assert_eq!(spans.indices().collect::<Vec<_>>(), [0, 3, 31]);
        assert_eq!(Spans::default().indices().count(), 0);
    }
}
