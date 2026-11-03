//! System services: requested reboots and the task watchdog supervisor.
//!
//! The board has no reset button and runs unattended, so it recovers on its
//! own. Faults (a panic, a supervised task missing its watchdog deadline, a
//! hung executor) reset the chip at once, because the system's state can't be
//! trusted. Everything else requests a reboot through the [`reboot`] service,
//! with a reason. The [`watchdog`] supervisor resets the chip when a
//! supervised task stops making progress, and feeds the chip's hardware
//! watchdogs, which reset it when the supervisor or the executor hangs.
//!
//! The crate holds the logic only and names no part of the chip or of the
//! firmware that uses it:
//!
//! - The chip's parts are traits taken at start-up: [`Reset`], the software
//!   reset, and [`watchdog::HardwareWatchdogs`].
//! - Reboot reasons are a type parameter. [`reboot::RebootRequest`] requests
//!   reboots with reasons of other types, which convert to it with `From`.
//! - Reboot holders are values of a type parameter, registered at start-up,
//!   each in one of a fixed number of slots, which the reboot
//!   [`Status`](reboot::Status) lists.
//! - Supervised tasks are registered at start-up, each in one of a fixed
//!   number of slots, with a name for the log and a marker type, which gives
//!   each task's handle a type of its own.
//!
//! The reboot service and the supervisor are the crate's only callers of
//! [`Reset::reset`].
//!
//! The crate is `no_std`, allocation-free, and also builds on the host. It
//! logs through `defmt` when its `defmt` feature is on, and otherwise not at
//! all.

#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate alloc;

#[macro_use]
mod fmt;
pub mod reboot;
#[cfg(test)]
mod test_support;
pub mod watchdog;

/// Why the reboot service or the watchdog supervisor resets the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ResetCause {
    /// A graceful reboot, once no hold was alive or its deadline passed.
    GracefulReboot,
    /// An immediate reboot.
    ImmediateReboot,
    /// A supervised task missed its watchdog deadline.
    MissedDeadline {
        /// The task's name, as registered.
        task: &'static str,
    },
}

/// The chip's software reset.
///
/// The reboot service ([`reboot::RebootService::start`]) and the watchdog
/// supervisor ([`watchdog::Supervisor::new`]) take it at start-up, and reset
/// the chip only through it.
pub trait Reset: Sync {
    /// Resets the chip at once. `cause` is what the crate resets for; the
    /// crate has logged it already.
    fn reset(&self, cause: ResetCause) -> !;
}
