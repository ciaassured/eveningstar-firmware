//! What a long job does between its steps.
//!
//! The crate's long jobs, such as verifying an image, run in short steps on
//! a cooperative executor. Between two steps they hand control back through a
//! [`Pace`]. [`Yield`] yields once, so the job runs as fast as the executor
//! lets it; a caller can instead pause for a while after each step, so the
//! job runs in the background of everything else.

use core::future::Future;

use crate::future::yield_now;

/// What happens between two steps of a job.
pub trait Pace {
    /// Runs after a step, before the next one.
    fn pause(&mut self) -> impl Future<Output = ()>;
}

/// Yields to the executor once between steps.
#[derive(Clone, Copy, Debug, Default)]
pub struct Yield;

impl Pace for Yield {
    fn pause(&mut self) -> impl Future<Output = ()> {
        yield_now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::block_on;

    #[test]
    fn yielding_takes_one_wake() {
        let run = block_on(async { Yield.pause().await });
        assert_eq!((run.polls, run.wakes), (2, 1));
    }
}
