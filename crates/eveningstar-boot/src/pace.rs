//! What the crate's long jobs do between their steps: the caller's pace, and
//! the caller's progress hook.

use eveningstar_esp_formats::pace::Pace;

/// A hook a long job calls once per step, such as once per KiB hashed, and
/// before it waits for a change that only something else brings about.
///
/// Every `FnMut()` closure is one, whose [`idle`](Self::idle) does nothing.
/// A job calls [`advance`](Self::advance) only while it works, and
/// [`idle`](Self::idle) before each such wait: until the next `advance`, the
/// job makes no progress of its own.
pub trait Progress {
    /// Runs after a step, before the job pauses.
    fn advance(&mut self);

    /// Runs before the job waits for a change that only something else
    /// brings about, such as a write session on a region ending. Does
    /// nothing by default.
    fn idle(&mut self) {}
}

impl<F: FnMut()> Progress for F {
    fn advance(&mut self) {
        self();
    }
}

/// A pace that calls a progress hook before each pause.
pub(crate) struct Paced<'a, P, G> {
    pace: &'a mut P,
    progress: &'a mut G,
}

impl<'a, P: Pace, G: Progress> Paced<'a, P, G> {
    /// `pace`, with `progress` called before each of its pauses.
    pub(crate) const fn new(pace: &'a mut P, progress: &'a mut G) -> Self {
        Self { pace, progress }
    }
}

impl<P: Pace, G: Progress> Pace for Paced<'_, P, G> {
    fn pause(&mut self) -> impl Future<Output = ()> {
        self.progress.advance();
        self.pace.pause()
    }
}

#[cfg(test)]
mod tests {
    use eveningstar_storage::test_support::block_on;

    use super::*;
    use crate::test_support::Counting;

    #[test]
    fn the_hook_runs_before_each_pause() {
        let mut pace = Counting::default();
        let mut advanced = 0;
        let mut progress = || advanced += 1;
        let mut paced = Paced::new(&mut pace, &mut progress);
        let run = block_on(async {
            paced.pause().await;
            paced.pause().await;
        });
        assert_eq!((run.polls, run.wakes), (1, 0));
        assert_eq!(pace.pauses, 2);
        assert_eq!(advanced, 2);
    }

    #[test]
    fn a_closure_does_nothing_when_idle() {
        let mut advanced = 0;
        let mut progress = || advanced += 1;
        progress.idle();
        progress.advance();
        assert_eq!(advanced, 1);
    }
}
