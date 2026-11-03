use alloc::boxed::Box;
use alloc::rc::Rc;
use core::pin::pin;

use super::*;
use crate::test_support::{RESET, TestWatchdogs, WatchdogLog, advance, poll, poll_reset, time};

enum Short {}
enum Long {}

const SHORT: TaskWatchdog<Short> = TaskWatchdog::new("short", Duration::from_secs(2));
const LONG: TaskWatchdog<Long> = TaskWatchdog::new("long", Duration::from_secs(10));

/// A supervisor with `N` slots, and the log of its hardware watchdogs.
fn supervisor<const N: usize>() -> (Supervisor<N, TestWatchdogs>, Rc<WatchdogLog>) {
    let log = Rc::new(WatchdogLog::default());
    let mailboxes = Box::leak(Box::new(Mailboxes::new()));
    let supervisor = Supervisor::new(mailboxes, TestWatchdogs(log.clone()), &RESET);
    (supervisor, log)
}

/// The reset for `task`'s missed deadline, as [`poll_reset`] returns it.
#[expect(
    clippy::unnecessary_wraps,
    reason = "compared with what `poll_reset` returns"
)]
fn missed(task: &'static str) -> Option<ResetCause> {
    Some(ResetCause::MissedDeadline { task })
}

#[test]
fn enables_the_hardware_watchdogs_when_made_and_feeds_them_per_poll() {
    let _time = time();
    let (supervisor, log) = supervisor::<1>();
    assert_eq!(
        log.enabled.get(),
        Some((Duration::from_secs(5), Duration::from_secs(15)))
    );
    assert_eq!(log.feeds.get(), 0);

    let mut run = pin!(supervisor.run());
    assert!(poll(run.as_mut()).is_pending());
    assert_eq!(log.feeds.get(), 1);
    for polls in 1..=3 {
        advance(POLL_INTERVAL);
        assert!(poll(run.as_mut()).is_pending());
        assert_eq!(log.feeds.get(), 1 + polls);
    }
}

#[test]
fn a_missed_deadline_resets_for_that_task() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<2>();
    let long = supervisor.register(LONG);
    let _short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    // The deadline is missed once more than the timeout has passed, at the
    // first poll after 2 s.
    for _ in 0..2 {
        advance(POLL_INTERVAL);
        long.feed();
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    long.feed();
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn feeding_keeps_a_task_supervised_without_a_reset() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    for _ in 0..60 {
        advance(POLL_INTERVAL);
        short.feed();
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    // Then it stops.
    for _ in 0..2 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn the_timeout_runs_from_the_start_of_run() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let _short = supervisor.register(SHORT);
    advance(Duration::from_secs(60));

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    for _ in 0..2 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn a_pause_stops_the_deadline_until_the_next_feed() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    short.pause();
    for _ in 0..600 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    short.feed();
    for _ in 0..3 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn a_pause_before_run_is_kept() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);
    short.pause();

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    for _ in 0..60 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
}

#[test]
fn a_sleep_replaces_the_timeout_until_the_next_feed() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    short.sleep(Duration::from_secs(30));
    // The sleep starts at the first poll that sees it, and ends once more
    // than its time has passed.
    for _ in 0..31 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn a_feed_ends_a_sleep() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    short.sleep(Duration::from_secs(30));
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), None);
    short.feed();
    for _ in 0..3 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
fn a_retired_task_is_no_longer_supervised() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<2>();
    let short = supervisor.register(SHORT);
    let _long = supervisor.register(LONG);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    short.retire();
    for _ in 0..10 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    // The other task is still supervised.
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("long"));
}

#[test]
fn the_latest_message_since_a_poll_wins() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let short = supervisor.register(SHORT);

    let mut run = pin!(supervisor.run());
    assert_eq!(poll_reset(run.as_mut()), None);
    // A feed after a pause supervises the task again from that poll.
    short.pause();
    short.feed();
    for _ in 0..3 {
        advance(POLL_INTERVAL);
        assert_eq!(poll_reset(run.as_mut()), None);
    }
    advance(POLL_INTERVAL);
    assert_eq!(poll_reset(run.as_mut()), missed("short"));
}

#[test]
#[should_panic(expected = "every task watchdog slot is taken")]
fn registering_more_tasks_than_slots_panics() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<1>();
    let _short = supervisor.register(SHORT);
    let _long = supervisor.register(LONG);
}

#[test]
#[should_panic(expected = "the task watchdog `again`'s marker type is registered already")]
fn registering_a_marker_twice_panics() {
    let _time = time();
    let (mut supervisor, _log) = supervisor::<2>();
    let _short = supervisor.register(SHORT);
    let _again = supervisor.register(TaskWatchdog::<Short>::new("again", Duration::from_secs(2)));
}

#[test]
#[should_panic(expected = "at least 2x POLL_INTERVAL")]
fn a_timeout_under_two_polls_panics() {
    let _ = TaskWatchdog::<Short>::new("quick", Duration::from_millis(1999));
}

#[test]
fn the_timeouts_keep_their_order() {
    assert_eq!(MAIN_TIMEOUT, POLL_INTERVAL * 5);
    assert_eq!(BACKSTOP_TIMEOUT, MAIN_TIMEOUT * 3);
    assert_eq!(SHORT.name(), "short");
    assert_eq!(SHORT.timeout(), Duration::from_secs(2));
}
