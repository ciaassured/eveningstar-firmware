//! Self-checks: the conditions besides OTA's own readiness that a running
//! image has to meet before [`Ota::confirm`] makes it `Valid`.
//!
//! A [`SelfChecks`] holds a fixed set of checks, named by values of a type
//! `C`, and is built in a `static` with every check pending. Its
//! [`handles`](SelfChecks::handles) are handed out once, all together, one
//! [`SelfCheck`] per check, and each passes or fails its check. A check's
//! state changes once, from pending to passed or to failed, and never again,
//! so a verdict read once holds for the rest of the boot. A handle dropped
//! without a verdict fails its check.
//!
//! [`Ota`] reads the set through [`ConfirmGate`], which only [`SelfChecks`]
//! implements, and confirms only once every check has passed.
//!
//! [`Ota::confirm`]: crate::Ota::confirm
//! [`Ota`]: crate::Ota

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// The state of one check, or of a set of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    /// Not decided yet: for a set, no check failed and at least one is
    /// pending.
    Pending,
    /// Passed: for a set, every check passed, which an empty set does.
    Passed,
    /// Failed: for a set, at least one check failed.
    Failed,
}

/// The encoding of [`CheckState::Pending`] in a check's atomic.
const PENDING: u8 = 0;
/// The encoding of [`CheckState::Passed`].
const PASSED: u8 = 1;
/// The encoding of [`CheckState::Failed`].
const FAILED: u8 = 2;

impl CheckState {
    /// The state `value` encodes.
    const fn decode(value: u8) -> Self {
        match value {
            PENDING => Self::Pending,
            PASSED => Self::Passed,
            _ => Self::Failed,
        }
    }
}

/// A fixed set of `N` self-checks, named by values of `C`, each pending
/// until its [`SelfCheck`] decides it.
pub struct SelfChecks<C, const N: usize> {
    names: [C; N],
    states: [AtomicU8; N],
    /// Whether [`handles`](Self::handles) has handed out the handles.
    taken: AtomicBool,
}

impl<C, const N: usize> SelfChecks<C, N> {
    /// The set of the checks `names`, each pending.
    #[must_use]
    pub const fn new(names: [C; N]) -> Self {
        Self {
            names,
            states: [const { AtomicU8::new(PENDING) }; N],
            taken: AtomicBool::new(false),
        }
    }

    /// The state of the whole set: [`CheckState::Failed`] when any check
    /// failed, otherwise [`CheckState::Pending`] when any is pending,
    /// otherwise [`CheckState::Passed`].
    #[must_use]
    pub fn verdict(&self) -> CheckState {
        let mut verdict = CheckState::Passed;
        for state in &self.states {
            match CheckState::decode(state.load(Ordering::Acquire)) {
                CheckState::Failed => return CheckState::Failed,
                CheckState::Pending => verdict = CheckState::Pending,
                CheckState::Passed => {}
            }
        }
        verdict
    }
}

impl<C: Copy, const N: usize> SelfChecks<C, N> {
    /// Each check's name and state, in the order of the names given to
    /// [`new`](Self::new).
    pub fn states(&self) -> impl Iterator<Item = (C, CheckState)> + '_ {
        self.names
            .iter()
            .zip(&self.states)
            .map(|(&name, state)| (name, CheckState::decode(state.load(Ordering::Acquire))))
    }
}

impl<C: Copy + Eq + fmt::Debug, const N: usize> SelfChecks<C, N> {
    /// The handles of every check, in the order of the names given to
    /// [`new`](Self::new).
    ///
    /// # Panics
    ///
    /// If the handles were handed out already, or two checks have the same
    /// name.
    pub fn handles(&self) -> [SelfCheck<'_, C>; N] {
        let first = !self.taken.swap(true, Ordering::AcqRel);
        assert!(first, "the self-checks' handles were handed out already");
        for (index, name) in self.names.iter().enumerate() {
            assert!(
                !self.names[..index].contains(name),
                "the self-check {name:?} is named twice"
            );
        }
        core::array::from_fn(|index| SelfCheck {
            name: self.names[index],
            state: &self.states[index],
        })
    }
}

impl<C: fmt::Debug, const N: usize> fmt::Debug for SelfChecks<C, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_map();
        for (name, state) in self.names.iter().zip(&self.states) {
            list.entry(name, &CheckState::decode(state.load(Ordering::Acquire)));
        }
        list.finish()
    }
}

/// The handle that decides one check of a [`SelfChecks`]: passes it or fails
/// it, once. Dropped without a verdict, it fails the check.
pub struct SelfCheck<'c, C> {
    name: C,
    state: &'c AtomicU8,
}

impl<C: Copy> SelfCheck<'_, C> {
    /// The check's name.
    #[must_use]
    pub const fn name(&self) -> C {
        self.name
    }
}

impl<C> SelfCheck<'_, C> {
    /// Passes the check.
    pub fn pass(self) {
        self.decide(PASSED);
    }

    /// Fails the check.
    pub fn fail(self) {
        self.decide(FAILED);
    }

    /// Moves the check from pending to `state`. A check that isn't pending
    /// was decided already, and stays as it is.
    fn decide(&self, state: u8) {
        // Only a handle decides its check, and it does so once, from
        // `pass`, `fail` or its drop, so the exchange fails only in the drop
        // that follows `pass` or `fail`.
        let _ = self
            .state
            .compare_exchange(PENDING, state, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<C> Drop for SelfCheck<'_, C> {
    fn drop(&mut self) {
        self.decide(FAILED);
    }
}

impl<C: fmt::Debug> fmt::Debug for SelfCheck<'_, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SelfCheck")
            .field("name", &self.name)
            .field(
                "state",
                &CheckState::decode(self.state.load(Ordering::Acquire)),
            )
            .finish()
    }
}

mod sealed {
    /// Implemented by [`SelfChecks`](super::SelfChecks) only.
    pub trait Sealed {}
}

impl<C, const N: usize> sealed::Sealed for SelfChecks<C, N> {}

/// What [`Ota::confirm`] reads to decide whether the running image may be
/// confirmed: a [`SelfChecks`], whatever the type of its names and their
/// number.
///
/// [`Ota::confirm`]: crate::Ota::confirm
pub trait ConfirmGate: sealed::Sealed + Sync {
    /// The state of the whole set, as [`SelfChecks::verdict`] gives it.
    fn verdict(&self) -> CheckState;
}

impl<C: Sync, const N: usize> ConfirmGate for SelfChecks<C, N> {
    fn verdict(&self) -> CheckState {
        Self::verdict(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Check {
        A,
        B,
    }

    #[test]
    fn an_empty_set_passes() {
        let checks = SelfChecks::<Check, 0>::new([]);
        assert_eq!(checks.verdict(), CheckState::Passed);
    }

    #[test]
    fn a_set_passes_once_every_check_passes() {
        let checks = SelfChecks::new([Check::A, Check::B]);
        let [a, b] = checks.handles();
        assert_eq!(checks.verdict(), CheckState::Pending);
        a.pass();
        assert_eq!(checks.verdict(), CheckState::Pending);
        b.pass();
        assert_eq!(checks.verdict(), CheckState::Passed);
    }

    #[test]
    fn a_failed_check_fails_the_set_even_with_others_pending() {
        let checks = SelfChecks::new([Check::A, Check::B]);
        let [a, b] = checks.handles();
        a.fail();
        assert_eq!(checks.verdict(), CheckState::Failed);
        b.pass();
        assert_eq!(checks.verdict(), CheckState::Failed);
    }

    #[test]
    fn a_handle_dropped_without_a_verdict_fails_its_check() {
        let checks = SelfChecks::new([Check::A, Check::B]);
        let [a, b] = checks.handles();
        drop(a);
        b.pass();
        assert_eq!(
            checks.states().collect::<Vec<_>>(),
            [
                (Check::A, CheckState::Failed),
                (Check::B, CheckState::Passed)
            ]
        );
    }

    #[test]
    fn checks_not_handed_out_are_pending() {
        let checks = SelfChecks::new([Check::A]);
        assert_eq!(checks.verdict(), CheckState::Pending);
    }

    #[test]
    #[should_panic(expected = "handed out already")]
    fn the_handles_are_handed_out_once() {
        let checks = SelfChecks::new([Check::A]);
        let _first = checks.handles();
        let _second = checks.handles();
    }

    #[test]
    #[should_panic(expected = "named twice")]
    fn a_name_is_used_once() {
        let checks = SelfChecks::new([Check::A, Check::A]);
        let _ = checks.handles();
    }
}
