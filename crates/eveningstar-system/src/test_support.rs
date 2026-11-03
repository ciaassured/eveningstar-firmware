//! Time, polling and a reset for the tests.

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::Cell;
use core::future::Future;
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::panic;
use std::sync::{Mutex, MutexGuard, PoisonError};

use embassy_time::{Duration, MockDriver};

use crate::watchdog::HardwareWatchdogs;
use crate::{Reset, ResetCause};

/// A reset that unwinds with its [`ResetCause`] as the payload, which
/// [`reset_by`] catches.
#[derive(Debug)]
pub(crate) struct TestReset;

impl Reset for TestReset {
    fn reset(&self, cause: ResetCause) -> ! {
        panic::resume_unwind(Box::new(cause))
    }
}

pub(crate) static RESET: TestReset = TestReset;

/// Runs `f`, and returns the cause it reset for, if it reset.
pub(crate) fn reset_by(f: impl FnOnce()) -> Option<ResetCause> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => None,
        Err(payload) => match payload.downcast::<ResetCause>() {
            Ok(cause) => Some(*cause),
            Err(payload) => panic::resume_unwind(payload),
        },
    }
}

/// Polls `future` once.
pub(crate) fn poll<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Polls `future` once, and returns the cause it reset for, if it reset.
pub(crate) fn poll_reset<F: Future + ?Sized>(future: Pin<&mut F>) -> Option<ResetCause> {
    reset_by(|| {
        let _ = poll(future);
    })
}

static TIME: Mutex<()> = Mutex::new(());

/// Takes the mock time driver, which every test shares, and sets it back to
/// zero. The tests that take it run one at a time.
pub(crate) fn time() -> MutexGuard<'static, ()> {
    let guard = TIME.lock().unwrap_or_else(PoisonError::into_inner);
    MockDriver::get().reset();
    guard
}

/// Advances the mock time by `duration`.
pub(crate) fn advance(duration: Duration) {
    MockDriver::get().advance(duration);
}

/// What the supervisor did with the [`TestWatchdogs`].
#[derive(Debug, Default)]
pub(crate) struct WatchdogLog {
    /// The timeouts the watchdogs were enabled with.
    pub(crate) enabled: Cell<Option<(Duration, Duration)>>,
    /// How many times they were fed.
    pub(crate) feeds: Cell<u32>,
}

/// Hardware watchdogs that record what the supervisor does with them.
#[derive(Debug, Default)]
pub(crate) struct TestWatchdogs(pub(crate) Rc<WatchdogLog>);

impl HardwareWatchdogs for TestWatchdogs {
    fn enable(&mut self, main: Duration, backstop: Duration) {
        self.0.enabled.set(Some((main, backstop)));
    }

    fn feed(&mut self) {
        self.0.feeds.set(self.0.feeds.get() + 1);
    }
}

/// A `defmt` logger that drops everything, for the tests with the `defmt`
/// feature.
#[cfg(feature = "defmt")]
mod logger {
    #[defmt::global_logger]
    struct Logger;

    // SAFETY: the logger keeps no state, so any order of calls is sound.
    unsafe impl defmt::Logger for Logger {
        fn acquire() {}
        unsafe fn flush() {}
        unsafe fn release() {}
        unsafe fn write(_bytes: &[u8]) {}
    }

    defmt::timestamp!("");

    /// A `defmt` panic, such as embassy-time's on an `Instant` overflow, as
    /// a test failure.
    #[defmt::panic_handler]
    fn panic() -> ! {
        core::panic!("defmt panic")
    }
}
