//! The OTA mechanism of the internal flash: the writer of an update to the
//! alternate slot, the images that verified there, and the writes to the
//! otadata that select, confirm and roll back an image.
//!
//! [`Ota`] holds the boot writers ([`BootWriters`]), which only the boot tier
//! has, and alone writes the alternate slot and the otadata. It reads the
//! verification of the slots from their content modules ([`AppSlot`]), whose
//! checks hash them, and the boot state from the otadata's ([`Otadata`]);
//! nothing here hashes a slot. Every erase and program runs in a write session,
//! which passes the guard of the protected spans, the running slot among them,
//! so no operation here reaches the running app or the boot chain even when an
//! offset is wrong.
//!
//! An update is written and verified ([`Ota::begin_update`], [`Update::write`],
//! [`Update::finish`]), the [`VerifiedImage`] it gives is selected for the next
//! boot ([`Ota::activate`]), and the image that boots is confirmed
//! ([`Ota::confirm`]) or rolled back ([`Ota::roll_back`]). Any booted image
//! that isn't `Valid` needs a confirmation ([`BootState::needs_confirmation`]),
//! and updates begin only from a `Valid` one. A power cut at any step leaves a
//! selection that starts a whole image: the one running, or an activated one
//! with its rollback armed. Nothing in the crate reboots: activating and
//! rolling back take effect at the next boot.
//!
//! An image verifies only when it embeds the record of the compiled partition
//! layout right after its descriptor ([`LAYOUT_RECORD`]): nothing here writes
//! the partition table or the bootloader, so an image built for another layout
//! would run on a layout it doesn't describe.
//!
//! [`Ota`] is ready ([`Ota::readiness`]) once the running app's image
//! verifies and its descriptor reads, which gives the project name an update
//! has to match; until then it refuses every write with the reason
//! ([`Unavailable`]).
//!
//! A confirmation also waits for a fixed set of self-checks ([`SelfChecks`]),
//! each passed or failed once by its handle ([`SelfCheck`]): [`Ota::confirm`]
//! refuses while one is pending ([`OtaError::ChecksPending`]) and for the
//! rest of the boot once one failed ([`OtaError::ChecksFailed`]). Rolling
//! back doesn't wait for them.
//!
//! The crate is `no_std`, allocation-free, and also builds on the host.
//!
//! [`BootWriters`]: eveningstar_storage::chain::BootWriters
//! [`AppSlot`]: eveningstar_boot::AppSlot
//! [`Otadata`]: eveningstar_boot::Otadata
//! [`BootState::needs_confirmation`]: eveningstar_esp_formats::otadata::BootState::needs_confirmation
//! [`LAYOUT_RECORD`]: eveningstar_storage::LAYOUT_RECORD

#![cfg_attr(not(test), no_std)]

mod checks;
mod error;
mod ota;
#[cfg(test)]
mod test_support;
mod update;

pub use checks::{CheckState, ConfirmGate, SelfCheck, SelfChecks};
pub use error::{OtaError, Unavailable};
pub use ota::Ota;
pub use update::{Update, VerifiedImage};
