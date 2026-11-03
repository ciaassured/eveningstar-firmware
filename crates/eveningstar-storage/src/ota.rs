//! The OTA mechanism: the writer of an update to the alternate slot, the
//! images it verifies, and the writes to the otadata that select, confirm and
//! roll back an image.
//!
//! It holds the boot writers ([`BootWriters`]), which only the boot tier has,
//! and reads the slot and the otadata through them. Every erase and program
//! runs in a write session, which passes the guard of the protected spans,
//! the running slot among them, so no operation here reaches the running app
//! or the boot chain even when an offset is wrong: an [`Update`] holds the
//! alternate slot's session for its whole life, and each otadata write is a
//! session of its own. The otadata writes are the plans of the [`otadata`]
//! module, made one at a time.

use core::fmt;
use core::marker::PhantomData;

use crate::chain::BootWriters;
use crate::guard::AccessError;
use crate::image::{self, App, ImageError};
use crate::nor::{Reader, SECTOR_SIZE, Session, WORD_SIZE, Writer};
use crate::otadata::{self, BootState, PlanError, Write};
use crate::roles::{AlternateApp, CurrentApp, Otadata};
use crate::{Epoch, ReadError, Slot, Source, Span, Text, WriteState};

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
    /// The image in the alternate slot doesn't verify, or the running app's
    /// descriptor doesn't read.
    Image(ImageError),
    /// An erase or program was refused or failed.
    Access(AccessError),
    /// The [`VerifiedImage`] was made before the latest
    /// [`Ota::begin_update`].
    Stale,
    /// The alternate slot no longer holds the image the [`VerifiedImage`]
    /// describes: it was written since, its epoch moved, or it hashes to
    /// another image; or its write state can't record an epoch.
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

/// The OTA mechanism, over the boot writers: the alternate slot and the
/// otadata, which it alone writes, the slot that booted and the running
/// app's project name.
///
/// An update is [`begin_update`](Self::begin_update), then
/// [`Update::write`] for each part of the image and [`Update::finish`],
/// which verifies what was written and returns a [`VerifiedImage`].
/// [`activate`](Self::activate) selects it, and the bootloader starts it at
/// the next boot as [`State::PendingVerify`](otadata::State::PendingVerify).
/// Unless [`confirm`](Self::confirm) confirms it, the boot after that
/// returns to the running slot; [`roll_back`](Self::roll_back) returns to it
/// on purpose. Nothing here reboots: activating and rolling back take effect
/// at the next boot.
///
/// An update begins, and an image is activated, only from a running image
/// that is confirmed `Valid` ([`BootState::needs_confirmation`]). Any other
/// booted image, whether booted `PendingVerify`, with no state, or as the
/// bootloader's fallback from a slot that didn't load, becomes `Valid` only
/// through `confirm`.
///
/// A power cut at any point leaves a selection that boots the running image,
/// or an activated image that verified, started once with its rollback
/// armed: no valid otadata entry selects the alternate slot from the start
/// of an update until its activation, and the activation's entry becomes
/// valid only once whole, with state `New`, as its sequence number is
/// programmed last.
pub struct Ota<'f> {
    alternate: Writer<'f, AlternateApp>,
    otadata: Writer<'f, Otadata>,
    running: Slot,
    project: Text<32>,
    /// Counts the updates begun, so a [`VerifiedImage`] from an earlier one
    /// is refused.
    generation: u32,
}

impl<'f> Ota<'f> {
    /// The mechanism over `writers`, for the app the current slot holds, read
    /// from `current` through a snapshot for the project name an update has
    /// to match.
    ///
    /// # Errors
    ///
    /// [`ImageError`] when the running app's descriptor doesn't read: there
    /// is then no project name for an update to match.
    pub fn new(
        writers: BootWriters<'f>,
        current: &Reader<'_, CurrentApp>,
    ) -> Result<Self, ImageError> {
        let project = project_name(current)?;
        let running = writers.slot();
        let BootWriters {
            alternate_app,
            otadata,
            ..
        } = writers;
        Ok(Self {
            alternate: alternate_app,
            otadata,
            running,
            project,
            generation: 0,
        })
    }

    /// The boot state the otadata gives now.
    #[must_use]
    pub fn boot_state(&self) -> BootState {
        otadata::read(&self.otadata).boot_state(self.running)
    }

    /// The running app's project name, which an update has to name.
    #[must_use]
    pub const fn project(&self) -> &Text<32> {
        &self.project
    }

    /// Starts an update of the alternate slot.
    ///
    /// First the boot selection is settled on the running slot, as
    /// [`plan_begin_update`](otadata::Otadata::plan_begin_update) plans it:
    /// an activation not booted yet is cancelled, and an entry naming the
    /// alternate slot is replaced, so the bootloader never selects the slot
    /// while it is rewritten, whatever is cut short. Then a write session
    /// opens on the slot, which the [`Update`] holds, and the slot's first
    /// sector is erased. Every [`VerifiedImage`] made before goes stale,
    /// whether or not this succeeds.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] with [`PlanError::Unconfirmed`] until the running
    /// image is confirmed `Valid`, or with another error of the planner;
    /// [`OtaError::Unsettled`] when two writes don't settle the selection;
    /// [`OtaError::Access`] when an erase or program fails.
    pub async fn begin_update(&mut self) -> Result<Update<'_, 'f>, OtaError> {
        // Wraps only after more updates than the slot's first sector
        // survives erases, and activation checks the slot's epoch anyway.
        self.generation = self.generation.wrapping_add(1);
        self.settle().await?;
        let mut session = self.alternate.begin();
        session.erase(0, SECTOR_SIZE).await?;
        Ok(Update {
            session,
            project: self.project,
            generation: self.generation,
            slot: self.running.other(),
            written: 0,
            erased: SECTOR_SIZE,
            partial: [0xFF; WORD],
            failed: false,
            flash: PhantomData,
        })
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
    /// [`verify_other`](Self::verify_other) since then, the slot has to be
    /// at the epoch it verified at, and the slot is hashed again to check it
    /// still holds it. The entry selecting it is written whole to the
    /// otadata sector that isn't active, so a power cut leaves the running
    /// slot selected.
    ///
    /// # Errors
    ///
    /// [`OtaError::Stale`] for an image made before the latest
    /// [`begin_update`](Self::begin_update); [`OtaError::Changed`] when the
    /// slot was written since or no longer holds it; [`OtaError::Image`]
    /// when the slot can't be read; [`OtaError::Plan`] when the active entry
    /// doesn't select the running slot (an activation already made, a
    /// fallback) or the running image isn't confirmed `Valid`;
    /// [`OtaError::Access`] when the write fails.
    pub async fn activate(&mut self, image: VerifiedImage<'f>) -> Result<(), OtaError> {
        self.check(image).await?;
        let write = otadata::read(&self.otadata).plan_activate(self.running)?;
        Ok(self.execute(&write).await?)
    }

    /// Checks that `image` comes from the latest update, that the slot is at
    /// the epoch it verified at, and that the slot still holds it, hashing
    /// the slot again.
    async fn check(&self, image: VerifiedImage<'f>) -> Result<(), OtaError> {
        let VerifiedImage {
            slot,
            size,
            digest,
            generation,
            epoch,
            ..
        } = image;
        if generation != self.generation || slot != self.running.other() {
            return Err(OtaError::Stale);
        }
        if self.alternate.write_state() != WriteState::Stable(epoch) {
            return Err(OtaError::Changed);
        }
        match image::verify_app(&self.alternate).await {
            Ok(app) if app.size() == size && *app.digest() == digest => Ok(()),
            Err(ImageError::Read(error)) => Err(ImageError::Read(error).into()),
            _ => Err(OtaError::Changed),
        }
    }

    /// Verifies the image the alternate slot holds, as [`Update::finish`]
    /// does apart from the length written, to activate it again.
    ///
    /// # Errors
    ///
    /// [`OtaError::Image`] when the image doesn't verify,
    /// [`OtaError::Project`] when its project name isn't the running app's,
    /// and [`OtaError::Changed`] when the slot's write state records no
    /// epoch.
    pub async fn verify_other(&mut self) -> Result<VerifiedImage<'f>, OtaError> {
        let verified = verify(&self.alternate, &self.project).await?;
        self.image_at_epoch(&verified)
    }

    /// The [`VerifiedImage`] of `verified`, the alternate slot's image, at
    /// the slot's epoch now.
    fn image_at_epoch(&self, verified: &Verified) -> Result<VerifiedImage<'f>, OtaError> {
        let WriteState::Stable(epoch) = self.alternate.write_state() else {
            return Err(OtaError::Changed);
        };
        Ok(VerifiedImage::of(
            verified,
            self.running.other(),
            self.generation,
            epoch,
        ))
    }

    /// Confirms the running image: makes it `Valid` and the one the active
    /// entry selects, whatever state it booted in, as
    /// [`plan_confirm`](otadata::Otadata::plan_confirm) plans it. Booted
    /// `PendingVerify` or with another state, its entry is rewritten in
    /// place, which cancels its rollback; booted as a fallback from the
    /// alternate slot, or with no valid entry, a `Valid` entry for it is
    /// written to the sector that isn't active. Nothing is written when it is
    /// confirmed already, during an activation not booted yet included.
    ///
    /// A power cut leaves the boot selection as before the call: a rollback
    /// for a `PendingVerify` image, the same fallback for one booted as a
    /// fallback.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] when a sector the write leaves alone can't be read
    /// or the sequence numbers are exhausted, and [`OtaError::Access`] when
    /// the write fails.
    pub async fn confirm(&mut self) -> Result<(), OtaError> {
        if let Some(write) = otadata::read(&self.otadata).plan_confirm(self.running)? {
            self.execute(&write).await?;
        }
        Ok(())
    }

    /// Marks the running image `Invalid` in place, so the bootloader starts
    /// the alternate slot at the next boot. The alternate slot has to be a
    /// rollback target holding an image that verifies, as
    /// [`verify_other`](Self::verify_other) checks it.
    ///
    /// A power cut leaves either entry selected, each naming an image that
    /// verified.
    ///
    /// # Errors
    ///
    /// [`OtaError::Plan`] when the alternate slot isn't a rollback target or
    /// the active entry doesn't select the running slot, checked before the
    /// image; [`OtaError::Image`] and [`OtaError::Project`] when the
    /// alternate slot's image doesn't verify; [`OtaError::Access`] when the
    /// write fails.
    pub async fn roll_back(&mut self) -> Result<(), OtaError> {
        let write = otadata::read(&self.otadata).plan_roll_back(self.running)?;
        self.execute_verified(&write).await
    }

    /// Makes `write` once the alternate slot's image verifies.
    async fn execute_verified(&mut self, write: &Write) -> Result<(), OtaError> {
        // The image only has to verify: the write doesn't depend on it.
        verify(&self.alternate, &self.project).await?;
        Ok(self.execute(write).await?)
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
            .finish()
    }
}

/// The project name of the app `current` holds, read through a snapshot.
fn project_name(current: &Reader<'_, CurrentApp>) -> Result<Text<32>, ImageError> {
    let snapshot = current
        .snapshot(Span::new(0, current.len()))
        .map_err(|_| ImageError::Read(ReadError::Stale))?;
    Ok(names(&snapshot)?.project)
}

/// The project name and version an app's descriptor names.
struct Names {
    project: Text<32>,
    version: Text<32>,
}

/// The names in the descriptor of the app image in `source`. Never inlined,
/// so the descriptor stays out of its callers' frames.
#[inline(never)]
fn names<S: Source + ?Sized>(source: &S) -> Result<Names, ImageError> {
    let descriptor = image::app_descriptor(source)?;
    Ok(Names {
        project: descriptor.project_name(),
        version: descriptor.version(),
    })
}

/// An image that verified: its length and digest, and the version its
/// descriptor names.
struct Verified {
    size: u32,
    digest: [u8; 32],
    version: Text<32>,
}

/// Verifies the app image `slot` holds and checks that its project name is
/// `project`.
async fn verify<S: Source + ?Sized>(slot: &S, project: &Text<32>) -> Result<Verified, OtaError> {
    match image::verify_app(slot).await {
        Ok(app) => identify(slot, project, &app),
        Err(error) => Err(error.into()),
    }
}

/// The image `app`, which `slot` holds, once its project name is checked to
/// be `project`.
fn identify<S: Source + ?Sized>(
    slot: &S,
    project: &Text<32>,
    app: &App,
) -> Result<Verified, OtaError> {
    let names = names(slot)?;
    if names.project.as_bytes() != project.as_bytes() {
        return Err(OtaError::Project);
    }
    Ok(Verified {
        size: app.size(),
        digest: *app.digest(),
        version: names.version,
    })
}

/// An update being written to the alternate slot, from
/// [`Ota::begin_update`].
///
/// It holds the slot's write session, so no snapshot of the slot can be
/// taken while it lives, and it borrows the [`Ota`], so nothing else changes
/// the slot or the boot selection meanwhile. Dropping it abandons the
/// update and ends the session: the slot stays partly written, and no valid
/// otadata entry selects it.
pub struct Update<'o, 'f> {
    session: Session<'o, AlternateApp>,
    project: Text<32>,
    generation: u32,
    /// The slot written, the alternate one.
    slot: Slot,
    /// The bytes taken so far, the partial word's included.
    written: u32,
    /// The offset up to which the update has erased the slot.
    erased: u32,
    /// The bytes of the last word, of which `written % WORD_SIZE` are taken.
    partial: [u8; WORD],
    /// An erase or program failed.
    failed: bool,
    flash: PhantomData<&'f ()>,
}

impl<'f> Update<'_, 'f> {
    /// The bytes taken so far.
    #[must_use]
    pub const fn written(&self) -> u32 {
        self.written
    }

    /// Takes the next `bytes` of the image, which arrives in order, in parts
    /// of any length.
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
            .is_some_and(|end| end <= self.session.len());
        if !fits {
            return Err(OtaError::TooLarge);
        }
        let taken = self.take(bytes).await;
        self.failed = taken.is_err();
        Ok(taken?)
    }

    /// Verifies what was written: the partial word is completed with `0xFF`
    /// and programmed, the slot's image is verified (header, chip, segments,
    /// checksum and the appended SHA-256), its project name compared with the
    /// running app's, and its length with the bytes written. Then the
    /// session ends, and the [`VerifiedImage`] records the slot's epoch
    /// after it.
    ///
    /// # Errors
    ///
    /// [`OtaError::Image`] when the image doesn't verify,
    /// [`OtaError::Project`] when it names another project,
    /// [`OtaError::Length`] when the bytes written are more or fewer than the
    /// image, [`OtaError::Interrupted`] after a failed write,
    /// [`OtaError::Access`] when programming the partial word fails, and
    /// [`OtaError::Changed`] when the slot's write state records no epoch.
    pub async fn finish(mut self) -> Result<VerifiedImage<'f>, OtaError> {
        if self.failed {
            return Err(OtaError::Interrupted);
        }
        self.flush().await?;
        // Read through the session, which alone writes the slot, so the
        // bytes are those it leaves.
        let verified = verify(&self.session, &self.project).await?;
        self.complete(&verified)
    }

    /// Checks the length of `verified`, the image written, ends the session,
    /// and records the slot's epoch after it.
    fn complete(self, verified: &Verified) -> Result<VerifiedImage<'f>, OtaError> {
        if verified.size != self.written {
            return Err(OtaError::Length {
                written: self.written,
                image: verified.size,
            });
        }
        let Self {
            session,
            slot,
            generation,
            ..
        } = self;
        let WriteState::Stable(epoch) = session.finish() else {
            return Err(OtaError::Changed);
        };
        Ok(VerifiedImage::of(verified, slot, generation, epoch))
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
        let end = offset
            .checked_add(len_u32(bytes))
            .ok_or(AccessError::OutOfRange)?;
        while self.erased < end {
            let next = self
                .erased
                .checked_add(SECTOR_SIZE)
                .ok_or(AccessError::OutOfRange)?;
            self.session.erase(self.erased, next).await?;
            self.erased = next;
        }
        self.session.program(offset, bytes).await
    }
}

impl fmt::Debug for Update<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Update")
            .field("slot", &self.slot)
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

/// An image in the alternate slot that verified, from [`Update::finish`] or
/// [`Ota::verify_other`]: an ESP32-C6 app of the running app's project,
/// intact, at the slot's epoch after it was written. It says nothing of who
/// made it.
///
/// [`Ota::activate`] takes it, and only while no update has begun since it
/// was made and the slot is still at that epoch. It borrows the flash as the
/// handles do, so it can't outlive them into a later
/// [`split`](crate::chain::split). Switching back to the image in the
/// alternate slot:
///
/// ```
/// use eveningstar_storage::chain::{Handles, split};
/// use eveningstar_storage::nor::{NorDevice, State};
/// use eveningstar_storage::{Ota, OtaError};
///
/// async fn switch_back(flash: &mut dyn NorDevice, state: &mut State, booted: u32) -> Result<(), OtaError> {
///     let storage = split(flash, state, booted).await;
///     if let Handles::Boot(handles, writers) = storage.handles {
///         let mut ota = Ota::new(writers, &handles.current_app)?;
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
/// use eveningstar_storage::chain::{Handles, split};
/// use eveningstar_storage::nor::{NorDevice, State};
/// use eveningstar_storage::{Ota, OtaError};
///
/// async fn switch_back(flash: &mut dyn NorDevice, state: &mut State, booted: u32) -> Result<(), OtaError> {
///     let Handles::Boot(handles, writers) = split(flash, state, booted).await.handles else {
///         return Ok(());
///     };
///     let mut ota = Ota::new(writers, &handles.current_app)?;
///     let image = ota.verify_other().await?;
///     drop((ota, handles));
///     let Handles::Boot(handles, writers) = split(flash, state, booted).await.handles else {
///         return Ok(());
///     };
///     Ota::new(writers, &handles.current_app)?.activate(image).await
/// }
/// ```
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct VerifiedImage<'f> {
    slot: Slot,
    size: u32,
    digest: [u8; 32],
    generation: u32,
    /// The slot's epoch after the image was written.
    epoch: Epoch,
    version: Text<32>,
    flash: PhantomData<&'f ()>,
}

impl VerifiedImage<'_> {
    /// The image `verified`, in `slot`, verified in update `generation` at
    /// the slot's `epoch`.
    fn of(verified: &Verified, slot: Slot, generation: u32, epoch: Epoch) -> Self {
        Self {
            slot,
            size: verified.size,
            digest: verified.digest,
            generation,
            epoch,
            version: verified.version,
            flash: PhantomData,
        }
    }

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

    /// The slot's epoch the image verified at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The version its app descriptor gives.
    #[must_use]
    pub const fn version(&self) -> &Text<32> {
        &self.version
    }
}

#[cfg(test)]
mod tests;
