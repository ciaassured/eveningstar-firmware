//! A future that hands control back to the executor once.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Completes on its second poll. The first poll wakes the task before
/// returning `Pending`, so the executor polls it again after running others.
pub(crate) fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

/// The future [`yield_now`] returns.
#[derive(Debug)]
pub(crate) struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::block_on;

    #[test]
    fn yielding_takes_two_polls_and_one_wake() {
        let run = block_on(yield_now());
        assert_eq!(run.polls, 2);
        assert_eq!(run.wakes, 1);
    }
}
