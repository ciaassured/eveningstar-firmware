//! The OTA mechanism: the writer of an update to the app slot that didn't
//! boot, the images it verifies, and the writes to the otadata that select,
//! confirm and roll back an image.
//!
//! Every erase and program goes through a [`Region`] of the other slot or of
//! the otadata, with the running slot passed to the protected-region guard,
//! so no operation here reaches the running app or the boot chain even when
//! an offset is wrong. The otadata writes are the plans of the [`otadata`]
//! module, made one at a time.

use core::fmt;
use core::marker::PhantomData;

use crate::flash::{SECTOR_SIZE, WORD_SIZE};
use crate::image::{self, App, ImageError};
use crate::otadata::{self, BootState, PlanError, Write};
use crate::region::{AccessError, Region};
use crate::{Slot, Text, split};

/// The most otadata writes [`Ota::begin_update`] makes: the planner settles
/// the selection within two.
const SETTLE_WRITES: u32 = 2;

/// Length of a word, the unit of programming, in bytes.
const WORD: usize = WORD_SIZE as usize;

/// Why an OTA operation was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaError {
    /// The otadata doesn't allow the operation, or a sector it keeps couldn't
    /// be read.
    Plan(PlanError),
    /// The image in the other slot doesn't verify.
    Image(ImageError),
    /// An erase or program was refused or failed.
    Access(AccessError),
    /// The [`VerifiedImage`] was made before the latest
    /// [`Ota::begin_update`].
    Stale,
    /// The other slot no longer holds the image the [`VerifiedImage`]
    /// describes.
    Changed,
    /// The image's project name isn't the running app's.
    Project,
    /// The bytes written aren't exactly the image: fewer or more of them.
    Length {
        /// The bytes written.
        written: u32,
        /// The image's length.
        image: u32,
    },
    /// The bytes would run past the end of the slot. Nothing was written.
    TooLarge,
    /// An earlier write of the update failed, so what the slot holds isn't
    /// known.
    Interrupted,
    /// The otadata still didn't select the running slot alone after the
    /// writes that settle it.
    Unsettled,
}

impl From<PlanError> for OtaError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<ImageError> for OtaError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl From<AccessError> for OtaError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

/// The OTA mechanism, in the full tier only: the app slot that didn't boot
/// and the otadata, which it alone writes, the running slot and the running
/// app's project name.
///
/// An update is [`begin_update`](Self::begin_update), then
/// [`Update::write`] for each part of the image and [`Update::finish`],
/// which verifies what was written and returns a [`VerifiedImage`].
/// [`activate`](Self::activate) selects it, and the bootloader starts it at
/// the next boot as [`State::PendingVerify`](otadata::State::PendingVerify).
/// Unless [`mark_valid`](Self::mark_valid) confirms it, the boot after that
/// returns to the running slot; [`roll_back`](Self::roll_back) returns to it
/// on purpose. Nothing here reboots: activating and rolling back take effect
/// at the next boot.
///
/// A power cut at any point leaves a selection that boots the running image,
/// or an activated image that verified, started once with its rollback
/// armed: no valid otadata entry selects the other slot from the start of an
/// update until its activation, and the activation's entry becomes valid only
/// once whole, with state `New`, as its sequence number is programmed last.
pub struct Ota<'f> {
    other: Region<'f>,
    otadata: Region<'f>,
    running: Slot,
    project: Text<32>,
    /// Counts the updates begun, so a [`VerifiedImage`] from an earlier one
    /// is refused.
    generation: u32,
}

impl<'f> Ota<'f> {
    /// The mechanism for the app running from `running`, with `other` the
    /// other slot and `otadata` the otadata partition.
    pub(crate) const fn new(
        other: Region<'f>,
        otadata: Region<'f>,
        running: Slot,
        project: Text<32>,
    ) -> Self {
        Self {
            other,
            otadata,
            running,
            project,
            generation: 0,
        }
    }

    /// The boot state the otadata gives now.
    #[must_use]
    pub fn boot_state(&self) -> BootState {
        otadata::read(&self.otadata).boot_state(self.running)
    }

    /// Starts an update of the other slot (O3).
    ///
    /// First the boot selection is settled on the running slot, as
    /// [`plan_begin_update`](otadata::Otadata::plan_begin_update) plans it:
    /// an activation not booted yet is cancelled, and an entry naming the
    /// other slot is replaced, so the bootloader never selects the slot
    /// while it is rewritten, whatever is cut short. Then the slot's first
    /// sector is erased. Every [`VerifiedImage`] made before goes stale,
    /// whether or not this succeeds.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] with [`PlanError::PendingVerify`] while the running
    /// image hasn't been confirmed, or with another error of the planner;
    /// [`OtaError::Unsettled`] when two writes don't settle the selection;
    /// [`OtaError::Access`] when an erase or program fails.
    pub async fn begin_update(&mut self) -> Result<Update<'_, 'f>, OtaError> {
        // Wraps only after more updates than the slot's first sector
        // survives erases, and activation hashes the slot again anyway.
        self.generation = self.generation.wrapping_add(1);
        self.settle().await?;
        self.other.erase(0, SECTOR_SIZE, Some(self.running)).await?;
        Ok(Update {
            ota: self,
            written: 0,
            erased: SECTOR_SIZE,
            partial: [0xFF; WORD],
            failed: false,
        })
    }

    /// Makes the writes `plan_begin_update` plans until it plans none.
    async fn settle(&self) -> Result<(), OtaError> {
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
    /// `New` (O6): the bootloader starts it once as `PendingVerify`, and
    /// rolls it back at the boot after unless it is confirmed.
    ///
    /// The image has to come from the latest update, or from
    /// [`verify_other`](Self::verify_other) since then, and the slot is
    /// hashed again to check it still holds it. The entry selecting it is
    /// written whole to the otadata sector that isn't active, so a power cut
    /// leaves the running slot selected.
    ///
    /// # Errors
    ///
    /// [`OtaError::Stale`] for an image made before the latest
    /// [`begin_update`](Self::begin_update); [`OtaError::Changed`] when the
    /// slot no longer holds it; [`OtaError::Image`] when the slot can't be
    /// read; [`OtaError::Plan`] when the active entry doesn't select the
    /// running slot (an activation already made, a fallback) or the running
    /// image hasn't been confirmed; [`OtaError::Access`] when the write fails.
    pub async fn activate(&mut self, image: VerifiedImage<'f>) -> Result<(), OtaError> {
        self.check(image).await?;
        let write = otadata::read(&self.otadata).plan_activate(self.running)?;
        Ok(self.execute(&write).await?)
    }

    /// Checks that `image` comes from the latest update and that the slot
    /// still holds it, hashing the slot again.
    async fn check(&self, image: VerifiedImage<'f>) -> Result<(), OtaError> {
        let VerifiedImage {
            slot,
            size,
            digest,
            generation,
            ..
        } = image;
        if generation != self.generation || slot != self.running.other() {
            return Err(OtaError::Stale);
        }
        match image::verify_app(&self.other).await {
            Ok(app) if app.size() == size && *app.digest() == digest => Ok(()),
            Err(ImageError::Read(error)) => Err(ImageError::Read(error).into()),
            _ => Err(OtaError::Changed),
        }
    }

    /// Verifies the image the other slot holds, as [`Update::finish`] does
    /// apart from the length written, to activate it again.
    ///
    /// # Errors
    ///
    /// [`OtaError::Image`] when the image doesn't verify, and
    /// [`OtaError::Project`] when its project name isn't the running app's.
    pub async fn verify_other(&mut self) -> Result<VerifiedImage<'f>, OtaError> {
        self.verify().await
    }

    /// Confirms the running image (O7): its entry, the active one, is
    /// rewritten in place with state `Valid`, which cancels its rollback.
    /// Nothing is written when it already is `Valid`.
    ///
    /// A power cut leaves the other entry active, which rolls back at the
    /// next boot.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] when the active entry doesn't select the running
    /// slot or the other sector can't be read, and [`OtaError::Access`] when
    /// the write fails.
    pub async fn mark_valid(&mut self) -> Result<(), OtaError> {
        if let Some(write) = otadata::read(&self.otadata).plan_mark_valid(self.running)? {
            self.execute(&write).await?;
        }
        Ok(())
    }

    /// Marks the running image `Invalid` in place (O8), so the bootloader
    /// starts the other slot at the next boot. The other slot has to be a
    /// rollback target holding an image that verifies, as
    /// [`verify_other`](Self::verify_other) checks it.
    ///
    /// A power cut leaves either entry selected, each naming an image that
    /// verified.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] when the other slot isn't a rollback target or the
    /// active entry doesn't select the running slot, checked before the
    /// image; [`OtaError::Image`] and [`OtaError::Project`] when the other
    /// slot's image doesn't verify; [`OtaError::Access`] when the write
    /// fails.
    pub async fn roll_back(&mut self) -> Result<(), OtaError> {
        let write = otadata::read(&self.otadata).plan_roll_back(self.running)?;
        self.execute_verified(&write).await
    }

    /// Makes `write` once the other slot's image verifies.
    async fn execute_verified(&self, write: &Write) -> Result<(), OtaError> {
        // The image only has to verify: the write doesn't depend on it.
        let _target = self.verify().await?;
        Ok(self.execute(write).await?)
    }

    /// Verifies the other slot's image and checks its project name.
    async fn verify(&self) -> Result<VerifiedImage<'f>, OtaError> {
        match image::verify_app(&self.other).await {
            Ok(app) => self.identify(&app),
            Err(error) => Err(error.into()),
        }
    }

    /// The [`VerifiedImage`] of `app`, the other slot's image, once its
    /// project name is checked to be the running app's.
    fn identify(&self, app: &App) -> Result<VerifiedImage<'f>, OtaError> {
        let id = split::app_id(&self.other)?;
        if id.project_name.as_bytes() != self.project.as_bytes() {
            return Err(OtaError::Project);
        }
        Ok(VerifiedImage {
            slot: self.running.other(),
            size: app.size(),
            digest: *app.digest(),
            generation: self.generation,
            version: id.version,
            flash: PhantomData,
        })
    }

    /// Makes the otadata write `write`: its sector erased, then its entry
    /// programmed in the order its programs come, the sequence number last.
    async fn execute(&self, write: &Write) -> Result<(), AccessError> {
        let at = write.offset();
        let running = Some(self.running);
        self.otadata
            .erase(at, at + otadata::SECTOR_SIZE, running)
            .await?;
        for (offset, bytes) in write.programs() {
            self.otadata.program(offset, bytes, running).await?;
        }
        Ok(())
    }
}

impl fmt::Debug for Ota<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ota")
            .field("other", &self.other)
            .field("otadata", &self.otadata)
            .field("running", &self.running)
            .field("project", &self.project)
            .field("generation", &self.generation)
            .finish()
    }
}

/// An update being written to the other slot, from [`Ota::begin_update`].
///
/// It borrows the [`Ota`], so nothing else changes the slot or the boot
/// selection while it lives. Dropping it abandons the update: the slot stays
/// partly written, and no valid otadata entry selects it.
pub struct Update<'o, 'f> {
    ota: &'o mut Ota<'f>,
    /// The bytes taken so far, the partial word's included.
    written: u32,
    /// The offset up to which the update has erased the slot.
    erased: u32,
    /// The bytes of the last word, of which `written % WORD_SIZE` are taken.
    partial: [u8; WORD],
    /// An erase or program failed.
    failed: bool,
}

impl<'f> Update<'_, 'f> {
    /// The bytes taken so far.
    #[must_use]
    pub const fn written(&self) -> u32 {
        self.written
    }

    /// Takes the next `bytes` of the image (O4), which arrives in order, in
    /// parts of any length.
    ///
    /// Each sector is erased just before the first bytes programmed into
    /// it, and whole words are programmed as they arrive. The last bytes of
    /// a word are held until the next write, or [`finish`](Self::finish),
    /// completes it. Yields after every sector erased and every 4 KiB
    /// programmed.
    ///
    /// # Errors
    ///
    /// [`OtaError::TooLarge`] when the bytes would run past the end of the
    /// slot, before any is written; [`OtaError::Access`] when an erase or
    /// program fails, after which the update takes nothing more and every
    /// later call returns [`OtaError::Interrupted`].
    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), OtaError> {
        if self.failed {
            return Err(OtaError::Interrupted);
        }
        let fits = u32::try_from(bytes.len())
            .ok()
            .and_then(|len| self.written.checked_add(len))
            .is_some_and(|end| end <= self.ota.other.len());
        if !fits {
            return Err(OtaError::TooLarge);
        }
        let taken = self.take(bytes).await;
        self.failed = taken.is_err();
        Ok(taken?)
    }

    /// Verifies what was written (O5): the partial word is completed with
    /// `0xFF` and programmed, then the slot's image is verified (header,
    /// chip, segments, checksum and the appended SHA-256), its project name
    /// compared with the running app's, and its length with the bytes
    /// written.
    ///
    /// # Errors
    ///
    /// [`OtaError::Image`] when the image doesn't verify,
    /// [`OtaError::Project`] when it names another project,
    /// [`OtaError::Length`] when the bytes written are more or fewer than the
    /// image, [`OtaError::Interrupted`] after a failed write, and
    /// [`OtaError::Access`] when programming the partial word fails.
    pub async fn finish(mut self) -> Result<VerifiedImage<'f>, OtaError> {
        if self.failed {
            return Err(OtaError::Interrupted);
        }
        self.flush().await?;
        let written = self.written;
        match self.ota.verify().await {
            Ok(image) if image.size != written => Err(OtaError::Length {
                written,
                image: image.size,
            }),
            verified => verified,
        }
    }

    /// Programs the partial word, completed with `0xFF`.
    async fn flush(&mut self) -> Result<(), AccessError> {
        let held = self.held();
        if held == 0 {
            return Ok(());
        }
        let mut word = [0xFF; WORD];
        if let (Some(to), Some(from)) = (word.get_mut(..held), self.partial.get(..held)) {
            to.copy_from_slice(from);
        }
        self.program(self.written - self.written % WORD_SIZE, &word)
            .await
    }

    /// The bytes of the partial word taken.
    const fn held(&self) -> usize {
        (self.written % WORD_SIZE) as usize
    }

    /// Takes `bytes`, which fit the slot: completes the partial word, then
    /// programs the whole words up to each sector's end at a time, and holds
    /// the rest.
    async fn take(&mut self, bytes: &[u8]) -> Result<(), AccessError> {
        let mut bytes = bytes;
        let held = self.held();
        if held > 0 {
            let (completing, rest) = bytes.split_at(bytes.len().min(WORD - held));
            self.hold(completing);
            bytes = rest;
            if self.held() == 0 {
                let word = self.partial;
                self.program(self.written - WORD_SIZE, &word).await?;
            }
        }
        let (mut words, tail) = bytes.split_at(bytes.len() - bytes.len() % WORD);
        while !words.is_empty() {
            let room = (SECTOR_SIZE - self.written % SECTOR_SIZE) as usize;
            let (piece, rest) = words.split_at(words.len().min(room));
            self.program(self.written, piece).await?;
            self.written += len_u32(piece);
            words = rest;
        }
        self.hold(tail);
        Ok(())
    }

    /// Adds `bytes`, which fit the partial word, to it.
    fn hold(&mut self, bytes: &[u8]) {
        let held = self.held();
        if let Some(to) = self.partial.get_mut(held..held + bytes.len()) {
            to.copy_from_slice(bytes);
            self.written += len_u32(bytes);
        }
    }

    /// Programs `bytes`, which lie in one sector at `offset`, after erasing
    /// the sectors up to theirs.
    async fn program(&mut self, offset: u32, bytes: &[u8]) -> Result<(), AccessError> {
        let running = Some(self.ota.running);
        let end = offset
            .checked_add(len_u32(bytes))
            .ok_or(AccessError::OutOfRange)?;
        while self.erased < end {
            let next = self
                .erased
                .checked_add(SECTOR_SIZE)
                .ok_or(AccessError::OutOfRange)?;
            self.ota.other.erase(self.erased, next, running).await?;
            self.erased = next;
        }
        self.ota.other.program(offset, bytes, running).await
    }
}

impl fmt::Debug for Update<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("slot", &self.ota.running.other())
            .field("written", &self.written)
            .field("erased", &self.erased)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

/// The length of `bytes`, which lie in a slot.
fn len_u32(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap_or(u32::MAX)
}

/// An image in the other slot that verified, from [`Update::finish`] or
/// [`Ota::verify_other`]: an ESP32-C6 app of the running app's project,
/// intact. It says nothing of who made it.
///
/// [`Ota::activate`] takes it, and only while no update has begun since it
/// was made. It borrows the flash as the handles do, so it can't outlive
/// them into a later [`split`](crate::split). Switching back to the image in
/// the other slot:
///
/// ```
/// use eveningstar_storage::{Flash, OtaError, split};
///
/// async fn switch_back(flash: &mut dyn Flash, booted: u32) -> Result<(), OtaError> {
///     let storage = split(flash, booted).await;
///     if let Some(mut ota) = storage.data.and_then(|data| data.ota) {
///         let image = ota.verify_other().await?;
///         ota.activate(image).await?;
///     }
///     Ok(())
/// }
/// ```
///
/// but not with an image kept from before a later `split`:
///
/// ```compile_fail,E0499
/// use eveningstar_storage::{Flash, OtaError, split};
///
/// async fn switch_back(flash: &mut dyn Flash, booted: u32) -> Result<(), OtaError> {
///     let mut ota = split(flash, booted).await.data.unwrap().ota.unwrap();
///     let image = ota.verify_other().await?;
///     drop(ota);
///     let mut again = split(flash, booted).await.data.unwrap().ota.unwrap();
///     again.activate(image).await
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct VerifiedImage<'f> {
    slot: Slot,
    size: u32,
    digest: [u8; 32],
    generation: u32,
    version: Text<32>,
    flash: PhantomData<&'f ()>,
}

impl VerifiedImage<'_> {
    /// The slot that holds the image.
    #[must_use]
    pub const fn slot(&self) -> Slot {
        self.slot
    }

    /// The image's length in bytes, the digest included.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// The image's SHA-256 digest, which the image stores and matches.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// The version its app descriptor gives.
    #[must_use]
    pub const fn version(&self) -> &Text<32> {
        &self.version
    }
}

#[cfg(test)]
mod tests;
