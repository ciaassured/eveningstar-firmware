use core::pin::pin;

use super::*;
use crate::test_support::{RESET, TestWatchdogs, advance, poll_reset, reset_by, time};
use crate::watchdog::{Mailboxes, Supervisor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Api,
    LinkDown,
    Other(OtherReason),
}

/// The reasons of another crate, which convert to [`Reason`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OtherReason {
    Update,
}

impl From<OtherReason> for Reason {
    fn from(reason: OtherReason) -> Self {
        Self::Other(reason)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holder {
    Http,
    Log,
    More,
}

type Service = RebootService<Reason, Holder, 2>;

/// A watchdog handle whose supervisor never runs.
fn unsupervised() -> WatchdogHandle<RebootWatchdog> {
    let mailboxes = Box::leak(Box::new(Mailboxes::<1>::new()));
    let mut supervisor = Supervisor::new(mailboxes, TestWatchdogs::default(), &RESET);
    supervisor.register(WATCHDOG)
}

/// A started service of its own.
fn start() -> (Reboot<Reason, Holder, 2>, RebootTask<Reason, Holder, 2>) {
    let service: &'static Service = Box::leak(Box::new(Service::new()));
    service.start(&RESET, unsupervised())
}

fn holds(reboot: Reboot<Reason, Holder, 2>) -> Vec<(Holder, u16)> {
    reboot.status().holds().collect()
}

#[test]
fn a_graceful_reboot_without_holds_resets_at_once() {
    let _time = time();
    let (reboot, task) = start();
    let mut task = pin!(task.run());
    assert_eq!(poll_reset(task.as_mut()), None);

    reboot.graceful(Reason::Api);
    assert_eq!(poll_reset(task.as_mut()), Some(ResetCause::GracefulReboot));
}

#[test]
fn a_graceful_reboot_waits_for_every_hold() {
    let _time = time();
    let (reboot, task) = start();
    let http = reboot.register_holder(Holder::Http);
    let log = reboot.register_holder(Holder::Log);
    let mut task = pin!(task.run());
    assert_eq!(poll_reset(task.as_mut()), None);

    let first = http.hold();
    let second = http.hold();
    let flush = log.hold();
    reboot.graceful(Reason::Api);
    assert_eq!(poll_reset(task.as_mut()), None);

    drop(first);
    advance(Duration::from_secs(1));
    assert_eq!(poll_reset(task.as_mut()), None);
    drop(flush);
    assert_eq!(poll_reset(task.as_mut()), None);
    drop(second);
    assert_eq!(poll_reset(task.as_mut()), Some(ResetCause::GracefulReboot));
}

#[test]
fn a_graceful_reboot_resets_at_its_deadline_while_held() {
    let _time = time();
    let (reboot, task) = start();
    let http = reboot.register_holder(Holder::Http);
    let mut task = pin!(task.run());
    assert_eq!(poll_reset(task.as_mut()), None);

    let _hold = http.hold();
    reboot.graceful(Reason::LinkDown);
    assert_eq!(poll_reset(task.as_mut()), None);
    advance(DEADLINE - Duration::from_ticks(1));
    assert_eq!(poll_reset(task.as_mut()), None);
    advance(Duration::from_ticks(1));
    assert_eq!(poll_reset(task.as_mut()), Some(ResetCause::GracefulReboot));
}

#[test]
fn the_deadline_runs_from_the_request() {
    let _time = time();
    let (reboot, task) = start();
    let http = reboot.register_holder(Holder::Http);
    let _hold = http.hold();
    advance(Duration::from_secs(100));
    reboot.graceful(Reason::Api);

    // The task starts later, and keeps the request's deadline.
    advance(Duration::from_secs(100));
    let mut task = pin!(task.run());
    assert_eq!(poll_reset(task.as_mut()), None);
    advance(DEADLINE - Duration::from_secs(100) - Duration::from_ticks(1));
    assert_eq!(poll_reset(task.as_mut()), None);
    advance(Duration::from_ticks(1));
    assert_eq!(poll_reset(task.as_mut()), Some(ResetCause::GracefulReboot));
}

#[test]
fn an_immediate_reboot_ignores_holds() {
    let _time = time();
    let (reboot, task) = start();
    let http = reboot.register_holder(Holder::Http);
    let mut task = pin!(task.run());
    assert_eq!(poll_reset(task.as_mut()), None);

    let _hold = http.hold();
    reboot.graceful(Reason::LinkDown);
    assert_eq!(poll_reset(task.as_mut()), None);
    assert_eq!(
        reset_by(|| reboot.immediate(Reason::Api)),
        Some(ResetCause::ImmediateReboot)
    );
}

#[test]
fn the_status_names_the_pending_reboot_and_the_live_holds() {
    let _time = time();
    let (reboot, _task) = start();
    let http = reboot.register_holder(Holder::Http);
    let log = reboot.register_holder(Holder::Log);
    assert_eq!(http.holder(), Holder::Http);
    assert_eq!(log.holder(), Holder::Log);

    assert!(!reboot.is_pending());
    assert_eq!(reboot.status().pending, None);
    assert_eq!(holds(reboot), []);

    let first = http.hold();
    let second = http.hold();
    let flush = log.hold();
    assert_eq!(holds(reboot), [(Holder::Http, 2), (Holder::Log, 1)]);
    drop(flush);
    drop(first);
    assert_eq!(holds(reboot), [(Holder::Http, 1)]);

    advance(Duration::from_secs(7));
    reboot.graceful(Reason::LinkDown);
    assert!(reboot.is_pending());
    assert!(reboot.graceful_only().is_pending());
    assert_eq!(
        reboot.status().pending,
        Some(Pending {
            reason: Reason::LinkDown,
            deadline: Instant::from_secs(7) + DEADLINE,
        })
    );
    drop(second);
    assert_eq!(holds(reboot), []);
}

#[test]
fn only_the_first_graceful_request_takes_effect() {
    let _time = time();
    let (reboot, _task) = start();
    reboot.graceful_only().graceful(Reason::LinkDown);
    advance(Duration::from_secs(10));
    reboot.graceful(Reason::Api);
    assert_eq!(
        reboot.status().pending,
        Some(Pending {
            reason: Reason::LinkDown,
            deadline: Instant::from_secs(0) + DEADLINE,
        })
    );
}

#[test]
fn a_request_with_another_reason_type_converts_it() {
    let _time = time();
    let (reboot, _task) = start();
    let request: &dyn RebootRequest<OtherReason> = &reboot.graceful_only();
    assert!(!request.is_pending());
    request.graceful(OtherReason::Update);
    assert!(request.is_pending());
    assert_eq!(
        reboot.status().pending.map(|pending| pending.reason),
        Some(Reason::Other(OtherReason::Update))
    );

    let request: &dyn RebootRequest<Reason> = &reboot;
    assert!(request.is_pending());
}

#[test]
#[should_panic(expected = "every reboot holder slot is taken")]
fn registering_more_holders_than_slots_panics() {
    let _time = time();
    let (reboot, _task) = start();
    let _http = reboot.register_holder(Holder::Http);
    let _log = reboot.register_holder(Holder::Log);
    let _more = reboot.register_holder(Holder::More);
}

#[test]
#[should_panic(expected = "the reboot holder is registered already: Http")]
fn registering_a_holder_twice_panics() {
    let _time = time();
    let (reboot, _task) = start();
    let _http = reboot.register_holder(Holder::Http);
    let _again = reboot.register_holder(Holder::Http);
}

#[test]
#[should_panic(expected = "the reboot service has started already")]
fn the_service_starts_once() {
    let _time = time();
    let service: &'static Service = Box::leak(Box::new(Service::new()));
    let _first = service.start(&RESET, unsupervised());
    let _second = service.start(&RESET, unsupervised());
}

#[test]
fn the_reboot_task_feeds_its_watchdog_until_a_graceful_reboot() {
    let _time = time();
    let mailboxes = Box::leak(Box::new(Mailboxes::<1>::new()));
    let mut supervisor = Supervisor::new(mailboxes, TestWatchdogs::default(), &RESET);
    let wd = supervisor.register(WATCHDOG);
    let service: &'static Service = Box::leak(Box::new(Service::new()));
    let (reboot, task) = service.start(&RESET, wd);
    let http = reboot.register_holder(Holder::Http);

    let mut supervisor = pin!(supervisor.run());
    let mut task = pin!(task.run());
    let mut step = |duration: Duration| {
        advance(duration);
        let task = poll_reset(task.as_mut());
        let supervisor = poll_reset(supervisor.as_mut());
        task.or(supervisor)
    };
    assert_eq!(step(Duration::from_ticks(0)), None);
    // Twice the watchdog's timeout without a reset.
    for _ in 0..2 * WATCHDOG_TIMEOUT.as_secs() {
        assert_eq!(step(Duration::from_secs(1)), None);
    }

    // A held graceful reboot stops the feeds, and resets at its deadline,
    // before the watchdog's timeout.
    let _hold = http.hold();
    reboot.graceful(Reason::Api);
    for _ in 0..DEADLINE.as_secs() - 1 {
        assert_eq!(step(Duration::from_secs(1)), None);
    }
    assert_eq!(
        step(Duration::from_secs(1)),
        Some(ResetCause::GracefulReboot)
    );
}
