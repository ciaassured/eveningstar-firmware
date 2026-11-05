//! [`Ota`]: the boot writers, and the operations that write the alternate
//! slot and the otadata.

use core::fmt;

use eveningstar_boot::app::{AppModel, AppSlot};
use eveningstar_boot::{LoadError, Otadata};
use eveningstar_esp_formats::otadata::{self, BootState, Write};
use eveningstar_esp_formats::{Slot, Text};
use eveningstar_storage::chain::BootWriters;
use eveningstar_storage::nor::{SECTOR_SIZE, Writer};
use eveningstar_storage::roles::{self, AlternateApp, CurrentApp};
use eveningstar_storage::{AccessError, WriteState};

use crate::checks::{CheckState, ConfirmGate};
use crate::error::{OtaError, Unavailable};
use crate::update::{Expected, Update, VerifiedImage, checked};

/// The most otadata writes [`Ota::begin_update`] makes: the planner settles
/// the selection within two.
const SETTLE_WRITES: u32 = 2;

/// The OTA mechanism, over the boot writers: the alternate slot and the
/// otadata, which it alone writes, with the alternate slot's content module,
/// whose check verifies what the slot holds, and the otadata's, which gives
/// the boot state.
///
/// An update is [`begin_update`](Self::begin_update), then [`Update::write`]
/// for each part of the image and [`Update::finish`], which returns a
/// [`VerifiedImage`] once the slot's check has verified what was written.
/// [`activate`](Self::activate) selects it, and the bootloader starts it at the
/// next boot as [`State::PendingVerify`]. Unless [`confirm`](Self::confirm)
/// confirms it, the boot after that returns to the running slot;
/// [`roll_back`](Self::roll_back) returns to it on purpose. Nothing here
/// reboots: activating and rolling back take effect at the next boot.
///
/// The slots' checks run elsewhere ([`App::run`]): [`new`](Self::new),
/// [`Update::finish`], [`verify_other`](Self::verify_other) and
/// [`roll_back`](Self::roll_back) complete once a check has kept its result for
/// the slot's epoch.
///
/// Every write is refused until OTA is ready ([`readiness`](Self::readiness)).
/// An update begins, and an image is activated, only from a running image
/// that is confirmed `Valid` ([`BootState::needs_confirmation`]). Any other
/// booted image, whether booted `PendingVerify`, with no state, or as the
/// bootloader's fallback from a slot that didn't load, becomes `Valid` only
/// through `confirm`, and only once every self-check of the gate given to
/// [`new`](Self::new) has passed ([`SelfChecks`]).
///
/// A power cut at any point leaves a selection that boots the running image,
/// or an activated image that verified, started once with its rollback
/// armed: no valid otadata entry selects the alternate slot from the start
/// of an update until its activation, and the activation's entry becomes
/// valid only once whole, with state `New`, as its sequence number is
/// programmed last.
///
/// [`State::PendingVerify`]: otadata::State::PendingVerify
/// [`App::run`]: eveningstar_boot::App::run
/// [`SelfChecks`]: crate::SelfChecks
pub struct Ota<'f> {
    alternate: Writer<'f, AlternateApp>,
    otadata: Writer<'f, roles::Otadata>,
    /// The alternate slot's module, whose check verifies it.
    slot: &'f AppSlot<'f, AlternateApp>,
    /// The otadata's module, which gives the boot state.
    boot: &'f Otadata<'f>,
    running: Slot,
    /// The running app's project name, which an update has to name, or why
    /// there is none.
    project: Result<Text<32>, Unavailable>,
    /// Counts the updates begun, so a [`VerifiedImage`] from an earlier one
    /// is refused.
    generation: u32,
    /// The self-checks a confirmation waits for.
    gate: &'f dyn ConfirmGate,
}

impl<'f> Ota<'f> {
    /// The mechanism over `writers`, with `alternate`, the alternate slot's
    /// module, and `otadata`, the otadata's, both over the regions the
    /// writers write. It completes once `current`, the running slot's module,
    /// has kept the result of its check, and is ready when the running app's
    /// image verified and its descriptor reads. [`confirm`](Self::confirm)
    /// writes only once every self-check of `gate` has passed.
    pub async fn new(
        writers: BootWriters<'f>,
        current: &AppSlot<'_, CurrentApp>,
        alternate: &'f AppSlot<'f, AlternateApp>,
        otadata: &'f Otadata<'f>,
        gate: &'f dyn ConfirmGate,
    ) -> Self {
        let project = project_name(current).await;
        let running = writers.slot();
        let BootWriters {
            alternate_app,
            otadata: otadata_writer,
            ..
        } = writers;
        Self {
            alternate: alternate_app,
            otadata: otadata_writer,
            slot: alternate,
            boot: otadata,
            running,
            project,
            generation: 0,
            gate,
        }
    }

    /// Whether OTA is ready: the running app's image verified and its
    /// descriptor reads.
    ///
    /// # Errors
    ///
    /// Why OTA refuses every write.
    pub fn readiness(&self) -> Result<(), Unavailable> {
        self.project.map(|_| ())
    }

    /// The running app's project name, which an update has to name.
    ///
    /// # Errors
    ///
    /// Why there is none: OTA is unavailable.
    pub const fn project(&self) -> Result<&Text<32>, Unavailable> {
        match &self.project {
            Ok(project) => Ok(project),
            Err(reason) => Err(*reason),
        }
    }

    /// The boot state the otadata gives now, read through its module.
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] when the otadata was written while it was read.
    pub fn boot_state(&self) -> Result<BootState, LoadError> {
        Ok(self.boot.parse()?.boot_state(self.running))
    }

    /// The running app's project name, once OTA is ready.
    fn ready(&self) -> Result<Text<32>, OtaError> {
        Ok(self.project?)
    }

    /// Starts an update of the alternate slot.
    ///
    /// First the boot selection is settled on the running slot, as
    /// [`plan_begin_update`] plans it: an activation not booted yet is
    /// cancelled, and an entry naming the alternate slot is replaced, so the
    /// bootloader never selects the slot while it is rewritten, whatever is cut
    /// short. Then a write session opens on the slot, which the [`Update`]
    /// holds, and the slot's first sector is erased. Every [`VerifiedImage`]
    /// made before goes stale, whether or not this succeeds once OTA is ready.
    ///
    /// # Errors
    ///
    /// [`OtaError::Unavailable`] until OTA is ready; [`OtaError::Plan`] with
    /// [`PlanError::Unconfirmed`] until the running image is confirmed `Valid`,
    /// or with another error of the planner; [`OtaError::Unsettled`] when two
    /// writes don't settle the selection; [`OtaError::Access`] when an erase or
    /// program fails.
    ///
    /// [`plan_begin_update`]: otadata::Otadata::plan_begin_update
    /// [`PlanError::Unconfirmed`]: otadata::PlanError::Unconfirmed
    pub async fn begin_update(&mut self) -> Result<Update<'_, 'f>, OtaError> {
        let project = self.ready()?;
        // Wraps only after more updates than the slot's first sector
        // survives erases, and activation checks the slot's epoch anyway.
        self.generation = self.generation.wrapping_add(1);
        self.settle().await?;
        let mut session = self.alternate.begin();
        session.erase(0, SECTOR_SIZE).await?;
        Ok(Update::new(
            session,
            self.slot,
            project,
            self.generation,
            self.running.other(),
        ))
    }

    /// Makes the writes `plan_begin_update` plans until it plans none.
    async fn settle(&mut self) -> Result<(), OtaError> {
        let mut writes = 0;
        while let Some(write) = otadata::read(&self.otadata).plan_begin_update(self.running)? {
            if writes == SETTLE_WRITES {
                return Err(OtaError::Unsettled);
            }
            self.execute(&write).await?;
            writes += 1;
        }
        Ok(())
    }

    /// Selects the image `image` describes for the next boot, with state
    /// `New`: the bootloader starts it once as `PendingVerify`, and rolls it
    /// back at the boot after unless it is confirmed.
    ///
    /// The image has to come from the latest update, or from
    /// [`verify_other`](Self::verify_other) since then, and the slot has to
    /// be at the epoch it verified at: no write has touched the slot since,
    /// so the slot isn't read or hashed again. The entry selecting it is
    /// written whole to the otadata sector that isn't active, so a power cut
    /// leaves the running slot selected.
    ///
    /// # Errors
    ///
    /// [`OtaError::Unavailable`] until OTA is ready; [`OtaError::Stale`] for
    /// an image made before the latest [`begin_update`](Self::begin_update);
    /// [`OtaError::Changed`] when the slot was written since;
    /// [`OtaError::Plan`] when the active entry doesn't select the running
    /// slot (an activation already made, a fallback) or the running image
    /// isn't confirmed `Valid`; [`OtaError::Access`] when the write fails.
    pub async fn activate(&mut self, image: VerifiedImage<'f>) -> Result<(), OtaError> {
        self.ready()?;
        self.check(&image)?;
        let write = otadata::read(&self.otadata).plan_activate(self.running)?;
        Ok(self.execute(&write).await?)
    }

    /// Checks that `image` comes from the latest update and that the slot is
    /// at the epoch it verified at.
    fn check(&self, image: &VerifiedImage<'f>) -> Result<(), OtaError> {
        if image.generation() != self.generation || image.slot() != self.running.other() {
            return Err(OtaError::Stale);
        }
        if self.alternate.write_state() != WriteState::Stable(image.epoch()) {
            return Err(OtaError::Changed);
        }
        Ok(())
    }

    /// The image the alternate slot holds, as its check verified it at the
    /// slot's epoch now, to activate it again: as [`Update::finish`] checks
    /// it, apart from the length written.
    ///
    /// # Errors
    ///
    /// [`OtaError::Unavailable`] until OTA is ready, [`OtaError::Image`] when
    /// the image doesn't verify, [`OtaError::Project`] when its project name
    /// isn't the running app's, [`OtaError::Layout`] when it embeds no layout
    /// record or one of another layout, and [`OtaError::Changed`] when the
    /// slot's write state records no epoch.
    pub async fn verify_other(&mut self) -> Result<VerifiedImage<'f>, OtaError> {
        let project = self.ready()?;
        self.verify(project).await
    }

    /// Whether the alternate slot holds an image that verifies, as
    /// [`verify`](Self::verify) checks it.
    async fn verifies(&self, project: Text<32>) -> Result<(), OtaError> {
        self.verify(project).await.map(drop)
    }

    /// [`verify_other`](Self::verify_other) for `project`, the running app's.
    async fn verify(&self, project: Text<32>) -> Result<VerifiedImage<'f>, OtaError> {
        let WriteState::Stable(epoch) = self.alternate.write_state() else {
            return Err(OtaError::Changed);
        };
        let expected = Expected {
            project,
            written: None,
            slot: self.running.other(),
            generation: self.generation,
        };
        checked(self.slot, epoch, expected).await
    }

    /// Confirms the running image: makes it `Valid` and the one the active
    /// entry selects, whatever state it booted in, as [`plan_confirm`] plans
    /// it. Booted `PendingVerify` or with another state, its entry is rewritten
    /// in place, which cancels its rollback; booted as a fallback from the
    /// alternate slot, or with no valid entry, a `Valid` entry for it is
    /// written to the sector that isn't active. Nothing is written when it is
    /// confirmed already, during an activation not booted yet included.
    ///
    /// An image that isn't confirmed already is confirmed only once every
    /// self-check of the gate has passed. A check that passed stays passed,
    /// so the gate read before the write holds when the write is made.
    ///
    /// A power cut leaves the boot selection as before the call: a rollback
    /// for a `PendingVerify` image, the same fallback for one booted as a
    /// fallback.
    ///
    /// # Errors
    ///
    /// [`OtaError::Unavailable`] until OTA is ready;
    /// [`OtaError::ChecksPending`] while a self-check is pending and none
    /// failed, and [`OtaError::ChecksFailed`] once one failed, in both cases
    /// with nothing written; [`OtaError::Plan`] when a sector the write
    /// leaves alone can't be read or the sequence numbers are exhausted, and
    /// [`OtaError::Access`] when the write fails.
    ///
    /// [`plan_confirm`]: otadata::Otadata::plan_confirm
    pub async fn confirm(&mut self) -> Result<(), OtaError> {
        self.ready()?;
        let Some(write) = otadata::read(&self.otadata).plan_confirm(self.running)? else {
            return Ok(());
        };
        match self.gate.verdict() {
            CheckState::Passed => {}
            CheckState::Pending => return Err(OtaError::ChecksPending),
            CheckState::Failed => return Err(OtaError::ChecksFailed),
        }
        Ok(self.execute(&write).await?)
    }

    /// Marks the running image `Invalid` in place, so the bootloader starts
    /// the alternate slot at the next boot. The alternate slot has to be a
    /// rollback target holding an image that verified, as
    /// [`verify_other`](Self::verify_other) checks it.
    ///
    /// A power cut leaves either entry selected, each naming an image that
    /// verified.
    ///
    /// # Errors
    ///
    /// [`OtaError::Unavailable`] until OTA is ready; [`OtaError::Plan`] when
    /// the alternate slot isn't a rollback target or the active entry doesn't
    /// select the running slot, checked before the image; [`OtaError::Image`],
    /// [`OtaError::Project`] and [`OtaError::Layout`] when the alternate
    /// slot's image doesn't verify; [`OtaError::Access`] when the write fails.
    pub async fn roll_back(&mut self) -> Result<(), OtaError> {
        let project = self.ready()?;
        let write = otadata::read(&self.otadata).plan_roll_back(self.running)?;
        // The image only has to verify: the write doesn't depend on it.
        self.verifies(project).await?;
        Ok(self.execute(&write).await?)
    }

    /// Makes the otadata write `write` in a session of its own: its sector
    /// erased, then its entry programmed in the order its programs come, the
    /// sequence number last.
    async fn execute(&mut self, write: &Write) -> Result<(), AccessError> {
        let at = write.offset();
        let mut session = self.otadata.begin();
        session.erase(at, at + otadata::SECTOR_SIZE).await?;
        for (offset, bytes) in write.programs() {
            session.program(offset, bytes).await?;
        }
        Ok(())
    }
}

impl fmt::Debug for Ota<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ota")
            .field("alternate", &self.alternate)
            .field("otadata", &self.otadata)
            .field("running", &self.running)
            .field("project", &self.project)
            .field("generation", &self.generation)
            .field("checks", &self.gate.verdict())
            .finish_non_exhaustive()
    }
}

/// The project name of the app `current` holds, once its check has kept the
/// result of the slot's epoch, or why there is none.
async fn project_name(current: &AppSlot<'_, CurrentApp>) -> Result<Text<32>, Unavailable> {
    let WriteState::Stable(epoch) = current.write_state() else {
        return Err(Unavailable::Unstable);
    };
    let model = current
        .verdict(epoch)
        .await
        .map_err(|_| Unavailable::Unstable)?;
    named(current, &model)
}

/// The project name the descriptor of the image `model` describes names.
/// Never inlined, so the descriptor stays out of its caller's frame.
#[inline(never)]
fn named(
    current: &AppSlot<'_, CurrentApp>,
    model: &AppModel<CurrentApp>,
) -> Result<Text<32>, Unavailable> {
    match current.descriptor(model) {
        Ok(Ok(descriptor)) => Ok(descriptor.project_name()),
        Ok(Err(failure)) => Err(Unavailable::Descriptor(failure)),
        Err(_) => Err(Unavailable::Unstable),
    }
}

#[cfg(test)]
mod tests;
