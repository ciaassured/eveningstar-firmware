//! An update being written to the alternate slot, and the images that
//! verified there.

use core::fmt;
use core::marker::PhantomData;

use eveningstar_boot::LoadError;
use eveningstar_boot::app::{AppModel, AppSlot, Layout};
use eveningstar_esp_formats::{Slot, Text};
use eveningstar_storage::nor::{SECTOR_SIZE, Session, WORD_SIZE};
use eveningstar_storage::roles::AlternateApp;
use eveningstar_storage::{AccessError, Epoch, WriteState};

use crate::error::OtaError;

/// Length of a word, the unit of programming, in bytes.
const WORD: usize = WORD_SIZE as usize;

/// An update being written to the alternate slot, from [`Ota::begin_update`].
///
/// It holds the slot's write session, so no snapshot of the slot can be taken
/// while it lives, and it borrows the [`Ota`], so nothing else changes the slot
/// or the boot selection meanwhile. Dropping it abandons the update and ends
/// the session: the slot stays partly written, and no valid otadata entry
/// selects it.
///
/// [`Ota::begin_update`]: crate::Ota::begin_update
/// [`Ota`]: crate::Ota
pub struct Update<'o, 'f> {
    session: Session<'o, AlternateApp>,
    /// The slot's module, whose check verifies what was written.
    module: &'f AppSlot<'f, AlternateApp>,
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
}

impl<'o, 'f> Update<'o, 'f> {
    /// The update of `slot` in `session`, whose first sector is erased,
    /// verified by `module`'s check, for an image of `project` in update
    /// `generation`.
    pub(crate) const fn new(
        session: Session<'o, AlternateApp>,
        module: &'f AppSlot<'f, AlternateApp>,
        project: Text<32>,
        generation: u32,
        slot: Slot,
    ) -> Self {
        Self {
            session,
            module,
            project,
            generation,
            slot,
            written: 0,
            erased: SECTOR_SIZE,
            partial: [0xFF; WORD],
            failed: false,
        }
    }

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

    /// Ends the update: the partial word is completed with `0xFF` and
    /// programmed, and the session ends at a new epoch of the slot. Then it
    /// waits for the slot's check to verify the slot at that epoch (header,
    /// chip, segments, checksum and the appended SHA-256), compares the
    /// image's length with the bytes written, its project name with the
    /// running app's and the layout record it embeds with the compiled
    /// layout's, and returns the [`VerifiedImage`] of that epoch.
    ///
    /// # Errors
    ///
    /// [`OtaError::Interrupted`] after a failed write, [`OtaError::Access`]
    /// when programming the partial word fails, [`OtaError::Image`] when the
    /// image doesn't verify or its descriptor doesn't read,
    /// [`OtaError::Length`] when the bytes written are more or fewer than the
    /// image, [`OtaError::Project`] when it names another project,
    /// [`OtaError::Layout`] when it embeds no layout record or one of another
    /// layout, and [`OtaError::Changed`] when the slot was written again
    /// before the check kept its result, or its write state records no epoch.
    pub async fn finish(mut self) -> Result<VerifiedImage<'f>, OtaError> {
        if self.failed {
            return Err(OtaError::Interrupted);
        }
        if let Err(error) = self.flush().await {
            return Err(error.into());
        }
        match self.end() {
            Ok((module, epoch, expected)) => checked(module, epoch, expected).await,
            Err(error) => Err(error),
        }
    }

    /// Ends the session, and gives the slot's module, its epoch after the
    /// session, and what the image written has to be. Never inlined, so the
    /// session stays out of the caller's frame.
    #[inline(never)]
    fn end(self) -> Result<(&'f AppSlot<'f, AlternateApp>, Epoch, Expected), OtaError> {
        let Self {
            session,
            module,
            project,
            generation,
            slot,
            written,
            ..
        } = self;
        let WriteState::Stable(epoch) = session.finish() else {
            return Err(OtaError::Changed);
        };
        let expected = Expected {
            project,
            written: Some(written),
            slot,
            generation,
        };
        Ok((module, epoch, expected))
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

/// What an image in the alternate slot has to be to verify, and where it
/// comes from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Expected {
    /// The running app's project name, which the image has to name.
    pub(crate) project: Text<32>,
    /// The bytes written, which the image's length has to be, when it was
    /// written by an update.
    pub(crate) written: Option<u32>,
    /// The slot that holds it.
    pub(crate) slot: Slot,
    /// The update it comes from.
    pub(crate) generation: u32,
}

/// The [`VerifiedImage`] of the image `module`'s check verified at `epoch`,
/// once the check has kept its result, when the image is as `expected`.
///
/// # Errors
///
/// As [`verified`] gives them, [`OtaError::Changed`] also when the slot
/// isn't stable at `epoch` while it waits.
pub(crate) async fn checked<'f>(
    module: &AppSlot<'_, AlternateApp>,
    epoch: Epoch,
    expected: Expected,
) -> Result<VerifiedImage<'f>, OtaError> {
    let verdict = module.verdict(epoch).await;
    verified(module, &verdict, &expected)
}

/// The version the descriptor of the image `model` describes names, once
/// its project name is checked to be `project`. Never inlined, so the
/// descriptor stays out of its caller's frame.
#[inline(never)]
fn version_of(
    module: &AppSlot<'_, AlternateApp>,
    model: &AppModel<AlternateApp>,
    project: &Text<32>,
) -> Result<Text<32>, OtaError> {
    match module.descriptor(model) {
        Ok(Ok(descriptor)) if descriptor.project_name().as_bytes() == project.as_bytes() => {
            Ok(descriptor.version())
        }
        Ok(Ok(_)) => Err(OtaError::Project),
        Ok(Err(failure)) => Err(OtaError::Image(failure)),
        Err(_) => Err(OtaError::Changed),
    }
}

/// Whether the image `model` describes embeds the compiled layout's record,
/// read at the model's epoch. Never inlined, so the reads stay out of its
/// caller's frame.
#[inline(never)]
fn built_for_the_layout(
    module: &AppSlot<'_, AlternateApp>,
    model: &AppModel<AlternateApp>,
) -> Result<(), OtaError> {
    match module.layout(model) {
        Ok(Ok(Layout::Compiled)) => Ok(()),
        Ok(Ok(Layout::Other | Layout::Missing)) => Err(OtaError::Layout),
        Ok(Err(failure)) => Err(OtaError::Image(failure)),
        Err(_) => Err(OtaError::Changed),
    }
}

/// The [`VerifiedImage`] of `verdict`, the result of `module`'s check, when
/// it is as `expected`, checked in this order. Never inlined, so the model
/// and the descriptor stay out of the callers' frames.
///
/// # Errors
///
/// [`OtaError::Changed`] when the verdict found the slot written,
/// [`OtaError::Image`] when the image doesn't verify or its descriptor
/// doesn't read, [`OtaError::Length`] when its length isn't the bytes
/// written, [`OtaError::Project`] when it names another project, and
/// [`OtaError::Layout`] when it doesn't embed the compiled layout's record.
#[inline(never)]
fn verified<'f>(
    module: &AppSlot<'_, AlternateApp>,
    verdict: &Result<AppModel<AlternateApp>, LoadError>,
    expected: &Expected,
) -> Result<VerifiedImage<'f>, OtaError> {
    let Ok(model) = verdict else {
        return Err(OtaError::Changed);
    };
    let image = match model.image() {
        Ok(image) => image,
        Err(failure) => return Err(OtaError::Image(failure)),
    };
    if let Some(written) = expected.written
        && image.size() != written
    {
        return Err(OtaError::Length {
            written,
            image: image.size(),
        });
    }
    let version = version_of(module, model, &expected.project)?;
    built_for_the_layout(module, model)?;
    Ok(VerifiedImage {
        slot: expected.slot,
        size: image.size(),
        digest: *image.digest(),
        generation: expected.generation,
        epoch: model.epoch(),
        version,
        flash: PhantomData,
    })
}

/// An image in the alternate slot that verified, from [`Update::finish`] or
/// [`Ota::verify_other`]: an ESP32-C6 app of the running app's project, built
/// for the compiled partition layout, intact, at the slot's epoch when the
/// slot's check verified it. It says nothing of who made it.
///
/// [`Ota::activate`] takes it, and only while no update has begun since it was
/// made and the slot is still at that epoch. It borrows the flash as the
/// handles do, so it can't outlive them into a later [`split`]. Switching back
/// to the image in the alternate slot, while the slots' checks run:
///
/// ```
/// use eveningstar_boot::{AppSlot, Otadata};
/// use eveningstar_ota::{Ota, OtaError};
/// use eveningstar_storage::chain::{Handles, split};
/// use eveningstar_storage::nor::{NorDevice, State};
///
/// async fn switch_back(
///     flash: &mut dyn NorDevice,
///     state: &mut State,
///     booted: u32,
/// ) -> Result<(), OtaError> {
///     let storage = split(flash, state, booted).await;
///     if let Handles::Boot(handles, writers) = storage.handles {
///         let current = AppSlot::new(handles.current_app);
///         let alternate = AppSlot::new(handles.alternate_app);
///         let otadata = Otadata::compiled(handles.otadata);
///         let mut ota = Ota::new(writers, &current, &alternate, &otadata).await;
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
/// use eveningstar_boot::{AppSlot, Otadata};
/// use eveningstar_ota::{Ota, OtaError};
/// use eveningstar_storage::chain::{Handles, split};
/// use eveningstar_storage::nor::{NorDevice, State};
///
/// async fn switch_back(
///     flash: &mut dyn NorDevice,
///     state: &mut State,
///     booted: u32,
/// ) -> Result<(), OtaError> {
///     let Handles::Boot(handles, writers) = split(flash, state, booted).await.handles else {
///         return Ok(());
///     };
///     let current = AppSlot::new(handles.current_app);
///     let alternate = AppSlot::new(handles.alternate_app);
///     let otadata = Otadata::compiled(handles.otadata);
///     let image = Ota::new(writers, &current, &alternate, &otadata).await.verify_other().await?;
///     let Handles::Boot(handles, writers) = split(flash, state, booted).await.handles else {
///         return Ok(());
///     };
///     let current = AppSlot::new(handles.current_app);
///     let alternate = AppSlot::new(handles.alternate_app);
///     let otadata = Otadata::compiled(handles.otadata);
///     Ota::new(writers, &current, &alternate, &otadata).await.activate(image).await
/// }
/// ```
///
/// [`Ota::verify_other`]: crate::Ota::verify_other
/// [`Ota::activate`]: crate::Ota::activate
/// [`split`]: eveningstar_storage::chain::split
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct VerifiedImage<'f> {
    slot: Slot,
    size: u32,
    digest: [u8; 32],
    generation: u32,
    /// The slot's epoch the image verified at.
    epoch: Epoch,
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

    /// The update it comes from.
    pub(crate) const fn generation(&self) -> u32 {
        self.generation
    }
}
