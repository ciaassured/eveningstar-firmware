//! The app slots: the images the bootloader starts, verified in the
//! background.
//!
//! An [`AppSlot`] owns the reader of one app region: the slot that booted
//! ([`CurrentApp`]) or the other one ([`AlternateApp`]) in the layout and
//! boot tiers, or the app the chain located in the unverified tier
//! ([`FoundApp`]). [`App`] refers to the slots of the tier, which their
//! owner places, for example each in a `static`.
//!
//! A slot is verified by its check ([`AppSlot::check`], run for every slot
//! by [`App::run`]): through a snapshot, at the region's epoch, the image's
//! header, segments, checksum and appended SHA-256, hashed with the caller's
//! hasher and paced by the caller's pace, with the caller's progress hook
//! called after each KiB, and told before each wait for the slot to change
//! ([`Progress::idle`]). The check runs at boot and again after each write
//! session on the slot that ends at a new epoch. A check a session
//! interrupts keeps no result and runs again at the new epoch. The slot
//! keeps the last result, its model ([`AppModel`]), for the epoch it was
//! verified at; [`AppSlot::verdict`] gives the result at a given epoch once
//! the check has kept it, so no image is hashed twice for one epoch.
//!
//! A slot made with its signature region ([`AppSlot::signed`], the current
//! and the alternate slot) also judges the region's file under a trust set
//! for the image's file SHA-256, once the image verifies, and keeps that
//! [`Signature`] in the model with the region's epoch ([`SlotSignature`]).
//! A model is whole while both regions are stable at its epochs: a new epoch
//! of the signature region alone judges the signature again without hashing
//! the image, and [`AppSlot::model`] and [`AppSlot::verdict`] give only whole
//! models. The check reads the region through the caller's buffer.
//!
//! An update's first verification ([`AppSlot::verify_overlaid`]) runs the
//! same check over the alternate slot with the image's first word, which the
//! update holds back, laid over the slot's, and returns its model instead of
//! keeping it.
//!
//! The image's descriptor ([`AppSlot::descriptor`]) and how its layout
//! record compares with the compiled layout ([`AppSlot::layout`]) are read
//! again on request, through a snapshot at the model's epoch.
//!
//! The view of a slot is the image, the digest included, once verified at
//! the region's epoch now; the bytes after it are never published, and an
//! image without an appended SHA-256 publishes nothing.
//!
//! A model is of one slot's role:
//!
//! ```compile_fail,E0308
//! use eveningstar_boot::app::{AppModel, AppSlot};
//! use eveningstar_storage::roles::{AlternateApp, CurrentApp};
//!
//! fn export<'s>(slot: &'s AppSlot<'_, CurrentApp>, model: &AppModel<AlternateApp>) {
//!     let _ = slot.published(model);
//! }
//! ```
//!
//! and only a check makes one:
//!
//! ```compile_fail,E0451
//! use core::marker::PhantomData;
//! use eveningstar_boot::app::AppModel;
//! use eveningstar_boot::ImageFailure;
//! use eveningstar_storage::roles::CurrentApp;
//!
//! fn forge(model: AppModel<CurrentApp>) -> AppModel<CurrentApp> {
//!     AppModel { image: Err(ImageFailure::Magic), ..model }
//! }
//! ```
//!
//! [`Signature`]: crate::signature::Signature

use core::cell::RefCell;
use core::fmt;
use core::future::{Future, poll_fn};
use core::marker::PhantomData;
use core::pin::{Pin, pin};
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::CriticalSectionMutex;
use embassy_sync::waitqueue::MultiWakerRegistration;
use eveningstar_esp_formats::hash::Sha256Hasher;
use eveningstar_esp_formats::image::{self, AppDescriptor, Header, ImageError};
use eveningstar_esp_formats::pace::Pace;
use eveningstar_signify::TrustSet;
use eveningstar_storage::nor::{Published, Reader, Snapshot, WORD_SIZE};
use eveningstar_storage::roles::{
    AlternateApp, AlternateSignature, CurrentApp, CurrentSignature, FoundApp, Internal, Publishable,
};
use eveningstar_storage::{Epoch, LAYOUT_RECORD, ReadError, Source, Span, WriteState};

use crate::error::{ImageFailure, LoadError, Unpublished, at, failure, publish, verdict};
use crate::pace::{Paced, Progress};
use crate::signature::{FileBuf, SignatureRegion, SlotSignature};

mod sealed {
    /// Implemented by the app roles only.
    pub trait Sealed {}
}

/// A role of an app region: [`CurrentApp`], [`AlternateApp`] or
/// [`FoundApp`].
pub trait AppRole: Publishable<Device = Internal> + sealed::Sealed {}

impl sealed::Sealed for CurrentApp {}
impl sealed::Sealed for AlternateApp {}
impl sealed::Sealed for FoundApp {}
impl AppRole for CurrentApp {}
impl AppRole for AlternateApp {}
impl AppRole for FoundApp {}

/// The most tasks waiting for one slot's result at once. Registering one
/// more wakes every task held, each of which registers again when polled.
const WAITERS: usize = 4;

/// Length of the image's first word, which an update holds back.
const WORD: usize = WORD_SIZE as usize;

/// An app image that verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedApp {
    header: Header,
    size: u32,
    digest: [u8; 32],
    file_sha256: [u8; 32],
}

impl VerifiedApp {
    /// The image header.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// The image's length in bytes, the digest included.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }

    /// The SHA-256 digest the image stores and matches, of every byte
    /// before it.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    /// The SHA-256 of the whole image, the stored digest included.
    #[must_use]
    pub const fn file_sha256(&self) -> &[u8; 32] {
        &self.file_sha256
    }
}

/// The result of a slot's check, as of one epoch, for the slot with role
/// `R`.
pub struct AppModel<R> {
    epoch: Epoch,
    image: Result<VerifiedApp, ImageFailure>,
    signature: Option<SlotSignature>,
    /// The first word laid over the slot's, for an update's first
    /// verification ([`AppSlot::verify_overlaid`]).
    overlay: Option<[u8; WORD]>,
    role: PhantomData<fn() -> R>,
}

impl<R> AppModel<R> {
    /// The region's epoch the image was verified at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The image, or why it doesn't verify.
    ///
    /// # Errors
    ///
    /// The first check that fails.
    pub const fn image(&self) -> Result<&VerifiedApp, ImageFailure> {
        match &self.image {
            Ok(image) => Ok(image),
            Err(failure) => Err(*failure),
        }
    }

    /// The image's signature: `None` for a slot without a signature region,
    /// or an image that doesn't verify.
    #[must_use]
    pub const fn signature(&self) -> Option<&SlotSignature> {
        self.signature.as_ref()
    }

    /// Whether the model is of the slot with its first word replaced, from
    /// [`AppSlot::verify_overlaid`], and not of the bytes the slot holds.
    #[must_use]
    pub const fn is_overlaid(&self) -> bool {
        self.overlay.is_some()
    }
}

impl<R> Clone for AppModel<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for AppModel<R> {}

impl<R> PartialEq for AppModel<R> {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch
            && self.image == other.image
            && self.signature == other.signature
            && self.overlay == other.overlay
    }
}

impl<R> Eq for AppModel<R> {}

impl<R> fmt::Debug for AppModel<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppModel")
            .field("epoch", &self.epoch)
            .field("image", &self.image)
            .field("signature", &self.signature)
            .field("overlaid", &self.overlay.is_some())
            .finish()
    }
}

/// A slot's signature region.
#[derive(Clone, Copy)]
enum Region<'f> {
    Current(&'f SignatureRegion<'f, CurrentSignature>),
    Alternate(&'f SignatureRegion<'f, AlternateSignature>),
}

impl Region<'_> {
    /// The region's write state now.
    fn write_state(self) -> WriteState {
        match self {
            Self::Current(region) => region.write_state(),
            Self::Alternate(region) => region.write_state(),
        }
    }

    /// Completes once a session on the region ends at a new epoch, from
    /// the region's state when called.
    fn changed<'a>(self) -> impl Future<Output = ()> + 'a
    where
        Self: 'a,
    {
        let (current, alternate) = match self {
            Self::Current(region) => (Some(region.changed()), None),
            Self::Alternate(region) => (None, Some(region.changed())),
        };
        async move {
            if let Some(changed) = current {
                changed.await;
            } else if let Some(changed) = alternate {
                changed.await;
            }
        }
    }

    /// Completes once no session is open on the region.
    async fn settle(self) {
        loop {
            // Taken before the look, so a session that ends after the look
            // completes it.
            let changed = self.changed();
            if self.write_state().is_stable() {
                return;
            }
            changed.await;
        }
    }

    /// The region's signature under `trust` for the image with the SHA-256
    /// `file_sha256`, read through `buf`, at the region's epoch now.
    fn judge(
        self,
        trust: &TrustSet,
        file_sha256: &[u8; 32],
        buf: &mut FileBuf,
    ) -> Result<SlotSignature, LoadError> {
        match self {
            Self::Current(region) => judge(region, trust, file_sha256, buf),
            Self::Alternate(region) => judge(region, trust, file_sha256, buf),
        }
    }
}

/// `region`'s signature under `trust` for the image with the SHA-256
/// `file_sha256`, read through `buf`, at the region's epoch now. Never
/// inlined, so the models stay out of the callers' frames.
#[inline(never)]
fn judge<R: crate::signature::SignatureRole>(
    region: &SignatureRegion<'_, R>,
    trust: &TrustSet,
    file_sha256: &[u8; 32],
    buf: &mut FileBuf,
) -> Result<SlotSignature, LoadError> {
    region.load(buf)?;
    let model = region.model().ok_or(LoadError::Stale)?;
    region.check(&model, trust, file_sha256, buf)
}

impl fmt::Debug for Region<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Current(region) => region.fmt(f),
            Self::Alternate(region) => region.fmt(f),
        }
    }
}

/// A slot's signature region and the trust set its signature is judged
/// under.
#[derive(Clone, Copy, Debug)]
struct Signed<'f> {
    region: Region<'f>,
    trust: &'static TrustSet,
}

/// What a slot's result kept lacks for an epoch.
enum Needs {
    /// Nothing: the result is whole.
    Nothing,
    /// The signature of the image with this whole-file SHA-256.
    Signature([u8; 32]),
    /// The image's verification.
    Image,
}

/// What a slot keeps: the last result and the tasks waiting for one.
struct Kept<R> {
    model: Option<AppModel<R>>,
    waiters: MultiWakerRegistration<WAITERS>,
}

impl<R> Kept<R> {
    /// What a slot keeps before its first check: no result, no task waiting.
    const fn empty() -> CriticalSectionMutex<RefCell<Self>> {
        CriticalSectionMutex::new(RefCell::new(Self {
            model: None,
            waiters: MultiWakerRegistration::new(),
        }))
    }
}

/// The content module of one app region, with role `R`.
pub struct AppSlot<'f, R: AppRole> {
    reader: Reader<'f, R>,
    signed: Option<Signed<'f>>,
    kept: CriticalSectionMutex<RefCell<Kept<R>>>,
}

impl<'f> AppSlot<'f, CurrentApp> {
    /// The module of the slot that booted, which `reader` reads, whose
    /// image's signature is the file in `region`, judged under `trust`;
    /// nothing is verified yet.
    #[must_use]
    pub const fn signed(
        reader: Reader<'f, CurrentApp>,
        region: &'f SignatureRegion<'f, CurrentSignature>,
        trust: &'static TrustSet,
    ) -> Self {
        Self::with(
            reader,
            Some(Signed {
                region: Region::Current(region),
                trust,
            }),
        )
    }
}

impl<'f> AppSlot<'f, AlternateApp> {
    /// The module of the other slot, which `reader` reads, whose image's
    /// signature is the file in `region`, judged under `trust`; nothing is
    /// verified yet.
    #[must_use]
    pub const fn signed(
        reader: Reader<'f, AlternateApp>,
        region: &'f SignatureRegion<'f, AlternateSignature>,
        trust: &'static TrustSet,
    ) -> Self {
        Self::with(
            reader,
            Some(Signed {
                region: Region::Alternate(region),
                trust,
            }),
        )
    }

    /// The verification of the slot at `epoch` with its first word replaced
    /// by `first`, the word an update holds back, and its signature judged
    /// at the signature region's epoch now: an update's first verification.
    /// Hashes with `sha` and paces as [`check`](Self::check) does.
    ///
    /// The result is returned, never kept as the slot's: the slot's own
    /// check, at the same epoch, sees the bytes the slot holds. Its
    /// descriptor and layout entry ([`descriptor`](Self::descriptor),
    /// [`layout`](Self::layout)) read the slot with the same word laid over
    /// it, and it publishes nothing ([`published`](Self::published)).
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] when the slot isn't stable at `epoch`, now or
    /// while it is read, and [`LoadError::Busy`] while a session is open on
    /// it.
    #[expect(
        clippy::large_stack_frames,
        reason = "runs once per update; the frame holds the image's hashing future and the model it returns, in the update's future"
    )]
    pub async fn verify_overlaid<H, P, G>(
        &self,
        epoch: Epoch,
        first: [u8; WORD],
        sha: H,
        pace: &mut P,
        progress: &mut G,
        buf: &mut FileBuf,
    ) -> Result<AppModel<AlternateApp>, LoadError>
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        let snapshot = at(&self.reader, epoch)?;
        let source = Overlaid {
            snapshot: &snapshot,
            first: Some(first),
        };
        let verified = verify(&source, sha, Paced::new(pace, progress)).await;
        let image = image_of(&verified).ok_or(LoadError::Stale)?;
        let signature = match &image {
            Ok(image) => self.judged(&image.file_sha256, progress, buf).await,
            Err(_) => None,
        };
        Ok(AppModel {
            epoch,
            image,
            signature,
            overlay: Some(first),
            role: PhantomData,
        })
    }
}

impl<'f, R: AppRole> AppSlot<'f, R> {
    /// The module of the app region `reader` reads, without a signature
    /// region; nothing is verified yet.
    #[must_use]
    pub const fn new(reader: Reader<'f, R>) -> Self {
        Self::with(reader, None)
    }

    /// The module of the app region `reader` reads, whose image's signature
    /// is judged as `signed` says.
    const fn with(reader: Reader<'f, R>, signed: Option<Signed<'f>>) -> Self {
        Self {
            reader,
            signed,
            kept: Kept::empty(),
        }
    }

    /// The region's span, from the start of the flash.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.reader.span()
    }

    /// The region's write state now.
    #[must_use]
    pub fn write_state(&self) -> WriteState {
        self.reader.write_state()
    }

    /// The last result, while the region is stable at its epoch and, for an
    /// image with a signature, the signature region at the signature's.
    /// Never inlined, so its copies stay out of the callers' frames.
    #[must_use]
    #[inline(never)]
    pub fn model(&self) -> Option<AppModel<R>> {
        let model = self.kept.lock(|kept| kept.borrow().model)?;
        (self.reader.write_state() == WriteState::Stable(model.epoch) && self.is_whole(&model))
            .then_some(model)
    }

    /// Whether `model`, a model of the slot's epoch, needs no signature
    /// judged again: the slot has no signature region, its image doesn't
    /// verify, or its signature is of the region's epoch now.
    #[inline(never)]
    fn is_whole(&self, model: &AppModel<R>) -> bool {
        match (&self.signed, &model.image, &model.signature) {
            (None, ..) | (Some(_), Err(_), _) => true,
            (Some(signed), Ok(_), Some(signature)) => {
                signed.region.write_state() == WriteState::Stable(signature.epoch())
            }
            (Some(_), Ok(_), None) => false,
        }
    }

    /// The result of the check at `epoch`: the model of the region as of
    /// `epoch`, once the slot's check has verified it and judged its
    /// signature at the signature region's epoch now.
    ///
    /// It completes at once when the result is kept, and otherwise once the
    /// check keeps it, so it waits for a check to run: [`App::run`] or
    /// [`check`](Self::check).
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] when the region isn't stable at `epoch`, now or
    /// while it waits: a session is open on it, or it has another epoch.
    pub async fn verdict(&self, epoch: Epoch) -> Result<AppModel<R>, LoadError> {
        // Wakes the task when a session on the region ends.
        let mut changed = pin!(self.reader.changed());
        poll_fn(|cx| self.poll_verdict(epoch, changed.as_mut(), cx)).await
    }

    /// Polls [`verdict`](Self::verdict): registers the task for the next
    /// result kept and for the region's change, then looks at both. Never
    /// inlined, so its copies stay out of the caller's frame.
    #[inline(never)]
    fn poll_verdict(
        &self,
        epoch: Epoch,
        changed: Pin<&mut impl Future<Output = ()>>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<AppModel<R>, LoadError>> {
        let kept = self.registered(epoch, cx);
        // Completes only once the region left `epoch` for good, which the
        // look at the state below then sees, so it is never polled again
        // after it completes.
        let _ = changed.poll(cx);
        if self.reader.write_state() != WriteState::Stable(epoch) {
            return Poll::Ready(Err(LoadError::Stale));
        }
        match kept {
            Some(model) => Poll::Ready(Ok(model)),
            None => Poll::Pending,
        }
    }

    /// Registers the task of `cx` for the next result kept, and gives the
    /// result kept, when it is of `epoch` and whole. Registered before the
    /// result is looked at, in one critical section with the look, so a
    /// result kept after it wakes the task. Never inlined, so its copies stay
    /// out of the caller's frame.
    #[inline(never)]
    fn registered(&self, epoch: Epoch, cx: &Context<'_>) -> Option<AppModel<R>> {
        self.kept
            .lock(|kept| {
                let mut kept = kept.borrow_mut();
                kept.waiters.register(cx.waker());
                kept.model
            })
            .filter(|model| model.epoch == epoch && self.is_whole(model))
    }

    /// Verifies the slot unless the result kept is of the region's epoch
    /// now, judges its image's signature unless the one kept is of the
    /// signature region's epoch now, and keeps the result. Hashes with a
    /// clone of `sha`, a hash not yet fed, and after each KiB hashed calls
    /// `progress`, then `pace`. Reads the signature region through `buf`.
    ///
    /// While a session is open on the region or on its signature region, it
    /// calls `progress`'s [`idle`](Progress::idle) and waits for the session
    /// to end. When a session writes the region during the verification, the
    /// verification keeps no result and runs again at the new epoch; a new
    /// epoch of the signature region alone judges the signature again,
    /// without hashing the image. It completes once a result of the
    /// region's epoch is kept.
    pub async fn check<H, P, G>(&self, sha: &H, pace: &mut P, progress: &mut G, buf: &mut FileBuf)
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        loop {
            match self.reader.write_state() {
                WriteState::Writing(_) => {
                    progress.idle();
                    self.settle().await;
                }
                WriteState::Stable(epoch) => match self.needs(epoch) {
                    Needs::Nothing => return,
                    Needs::Signature(file_sha256) => {
                        let signature = self.judged(&file_sha256, progress, buf).await;
                        self.keep_signature(epoch, signature);
                    }
                    Needs::Image => {
                        // A result of another epoch, or none when a read
                        // found the region written: the next round looks
                        // again.
                        let _ = self.verify_at(epoch, sha.clone(), pace, progress).await;
                    }
                },
            }
        }
    }

    /// What the result kept lacks for `epoch`. Never inlined, so the model
    /// stays out of the check's frame.
    #[inline(never)]
    fn needs(&self, epoch: Epoch) -> Needs {
        let Some(model) = self.kept(epoch) else {
            return Needs::Image;
        };
        if self.is_whole(&model) {
            return Needs::Nothing;
        }
        match model.image {
            Ok(image) => Needs::Signature(image.file_sha256),
            // An image that doesn't verify has no signature to judge, so its
            // model is whole.
            Err(_) => Needs::Nothing,
        }
    }

    /// Verifies the image at `epoch`, hashing with `sha` and pacing as
    /// [`check`](Self::check) does, and keeps the result without a
    /// signature, which a slot with a signature region then still needs.
    /// Returns whether it kept one.
    async fn verify_at<H, P, G>(&self, epoch: Epoch, sha: H, pace: &mut P, progress: &mut G) -> bool
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        // Refused only while a session is open, which the next round waits
        // out.
        let Ok(snapshot) = at(&self.reader, epoch) else {
            return false;
        };
        let source = Overlaid {
            snapshot: &snapshot,
            first: None,
        };
        let verified = verify(&source, sha, Paced::new(pace, progress)).await;
        self.keep_image(epoch, &verified)
    }

    /// The signature of the image whose whole file has the SHA-256
    /// `file_sha256` under the slot's trust set, judged at the signature
    /// region's epoch, once no session is open on it: `None` for a slot
    /// without a signature region.
    async fn judged<G: Progress>(
        &self,
        file_sha256: &[u8; 32],
        progress: &mut G,
        buf: &mut FileBuf,
    ) -> Option<SlotSignature> {
        let signed = self.signed?;
        loop {
            if !signed.region.write_state().is_stable() {
                progress.idle();
                signed.region.settle().await;
            }
            match signed.region.judge(signed.trust, file_sha256, buf) {
                Ok(signature) => return Some(signature),
                // A session opened or ended meanwhile: the next round waits
                // it out, or reads the new epoch.
                Err(LoadError::Busy | LoadError::Stale) => {}
                Err(LoadError::Absent) => return None,
            }
        }
    }

    /// Completes once no session is open on the region.
    async fn settle(&self) {
        loop {
            // Taken before the look, so a session that ends after the look
            // completes it.
            let changed = self.reader.changed();
            if self.reader.write_state().is_stable() {
                return;
            }
            changed.await;
        }
    }

    /// Completes once a session on the region or on its signature region
    /// ends at a new epoch, from their states when called.
    fn changed(&self) -> impl Future<Output = ()> + '_ {
        let slot = self.reader.changed();
        let signature = self.signed.map(|signed| signed.region.changed());
        async move {
            match signature {
                Some(signature) => either(slot, signature).await,
                None => slot.await,
            }
        }
    }

    /// The result kept, when it is of `epoch`.
    #[inline(never)]
    fn kept(&self, epoch: Epoch) -> Option<AppModel<R>> {
        self.kept
            .lock(|kept| kept.borrow().model)
            .filter(|model| model.epoch == epoch)
    }

    /// Keeps the result of the verification `verified` at `epoch`, without a
    /// signature, and wakes the tasks waiting for a result; nothing when a
    /// read found the region written, as that is no result for the epoch.
    /// Returns whether it kept one. Never inlined, so the model is assembled
    /// outside the check's frame.
    #[inline(never)]
    fn keep_image(&self, epoch: Epoch, verified: &Result<image::App, ImageError>) -> bool {
        let Some(image) = image_of(verified) else {
            return false;
        };
        let model = AppModel {
            epoch,
            image,
            signature: None,
            overlay: None,
            role: PhantomData,
        };
        self.kept.lock(|kept| {
            let mut kept = kept.borrow_mut();
            kept.model = Some(model);
            kept.waiters.wake();
        });
        true
    }

    /// Adds `signature` to the result kept, when it is of `epoch`, and
    /// wakes the tasks waiting for a result. Never inlined, so the model
    /// stays out of the check's frame.
    #[inline(never)]
    fn keep_signature(&self, epoch: Epoch, signature: Option<SlotSignature>) {
        self.kept.lock(|kept| {
            let mut kept = kept.borrow_mut();
            if let Some(model) = kept.model.as_mut().filter(|model| model.epoch == epoch) {
                model.signature = signature;
                kept.waiters.wake();
            }
        });
    }

    /// The descriptor of the image `model` describes, read again at the
    /// model's epoch, with its overlaid word.
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] once the region isn't stable at the model's
    /// epoch, and [`LoadError::Busy`] while a session is open on it; the
    /// image's failure when it doesn't verify.
    pub fn descriptor(
        &self,
        model: &AppModel<R>,
    ) -> Result<Result<AppDescriptor, ImageFailure>, LoadError> {
        if let Err(failure) = model.image {
            return Ok(Err(failure));
        }
        let snapshot = at(&self.reader, model.epoch)?;
        let source = Overlaid {
            snapshot: &snapshot,
            first: model.overlay,
        };
        verdict(image::app_descriptor(&source))
    }

    /// How the layout entry of the image `model` describes compares with
    /// the compiled layout ([`LAYOUT_RECORD`]), read again at the model's
    /// epoch, with its overlaid word.
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] once the region isn't stable at the model's
    /// epoch, and [`LoadError::Busy`] while a session is open on it; the
    /// image's failure when it doesn't verify.
    #[inline(never)]
    pub fn layout(&self, model: &AppModel<R>) -> Result<Result<Layout, ImageFailure>, LoadError> {
        if let Err(failure) = model.image {
            return Ok(Err(failure));
        }
        let snapshot = at(&self.reader, model.epoch)?;
        let source = Overlaid {
            snapshot: &snapshot,
            first: model.overlay,
        };
        verdict(compare_layout(&source))
    }

    /// The view of the image `model` describes, the digest included: the
    /// region's first [`VerifiedApp::size`] bytes.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] when the image doesn't verify, or the
    /// model is of an overlaid slot, whose bytes the slot doesn't hold;
    /// [`Unpublished::Stale`] once the region isn't stable at the model's
    /// epoch, and [`Unpublished::Refused`] for the storage crate's other
    /// refusals.
    pub fn published(&self, model: &AppModel<R>) -> Result<Published<'_, R>, Unpublished> {
        let (Ok(image), None) = (&model.image, model.overlay) else {
            return Err(Unpublished::Unverified);
        };
        publish(&self.reader, model.epoch, Span::new(0, image.size), &[]).map_err(Unpublished::from)
    }
}

impl<R: AppRole> fmt::Debug for AppSlot<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppSlot")
            .field("reader", &self.reader)
            .field("signed", &self.signed)
            .field("model", &self.kept.lock(|kept| kept.borrow().model))
            .finish()
    }
}

/// A slot read through a snapshot, with its first word replaced by `first`
/// when given.
struct Overlaid<'s, 'r, R: AppRole> {
    snapshot: &'s Snapshot<'r, R>,
    first: Option<[u8; WORD]>,
}

impl<R: AppRole> Source for Overlaid<'_, '_, R> {
    fn size(&self) -> u32 {
        self.snapshot.size()
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        self.snapshot.read(offset, buf)?;
        if let Some(first) = &self.first {
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            for (at, byte) in buf.iter_mut().enumerate() {
                match start.checked_add(at).and_then(|at| first.get(at)) {
                    Some(word_byte) => *byte = *word_byte,
                    None => break,
                }
            }
        }
        Ok(())
    }
}

/// The image of the verification `verified`, without the values read when
/// it doesn't verify; `None` when a read found the region written, as that
/// is no result for the snapshot's epoch.
fn image_of(
    verified: &Result<image::App, ImageError>,
) -> Option<Result<VerifiedApp, ImageFailure>> {
    match verified {
        Ok(app) => Some(Ok(VerifiedApp {
            header: *app.header(),
            size: app.size(),
            digest: *app.digest(),
            file_sha256: *app.file_sha256(),
        })),
        Err(error) => failure(*error).ok().map(Err),
    }
}

/// How the layout entry of an app image's information block compares with
/// the compiled layout ([`LAYOUT_RECORD`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The entry equals the compiled layout in every byte and in length.
    Compiled,
    /// The entry describes another layout.
    Other,
    /// No block lies right after the image's descriptor, the block's framing
    /// doesn't read, or it holds no layout entry or more than one.
    Missing,
}

/// How the layout entry of the image `source` reads compares with the
/// compiled layout.
fn compare_layout<S: Source + ?Sized>(source: &S) -> Result<Layout, ImageError> {
    match image::info_entry(source, image::LAYOUT_ENTRY)? {
        image::InfoFound::Entry(entry) if LAYOUT_RECORD.matches(source, &entry)? => {
            Ok(Layout::Compiled)
        }
        image::InfoFound::Entry(_) => Ok(Layout::Other),
        image::InfoFound::NoBlock | image::InfoFound::Invalid(_) | image::InfoFound::Missing => {
            Ok(Layout::Missing)
        }
    }
}

/// Verifies the image `source` reads, hashing with `sha` and pausing with
/// `paced`.
async fn verify<S: Source + ?Sized, P: Pace>(
    source: &S,
    sha: impl Sha256Hasher,
    mut paced: P,
) -> Result<image::App, ImageError> {
    image::verify_app_with(source, sha, &mut paced).await
}

/// The slots of a tier.
#[derive(Clone, Copy, Debug)]
enum Slots<'a, 'f> {
    /// The layout and boot tiers' slots.
    Layout {
        current: &'a AppSlot<'f, CurrentApp>,
        alternate: &'a AppSlot<'f, AlternateApp>,
    },
    /// The app the chain located in the unverified tier, if any.
    Found(Option<&'a AppSlot<'f, FoundApp>>),
}

/// The app slots of the tier the board reached, each placed by its owner,
/// for example in a `static`.
#[derive(Clone, Copy, Debug)]
pub struct App<'a, 'f> {
    slots: Slots<'a, 'f>,
}

impl<'a, 'f> App<'a, 'f> {
    /// The slots of the layout and boot tiers: `current`, the slot that
    /// booted, and `alternate`, the other one.
    #[must_use]
    pub const fn layout(
        current: &'a AppSlot<'f, CurrentApp>,
        alternate: &'a AppSlot<'f, AlternateApp>,
    ) -> Self {
        Self {
            slots: Slots::Layout { current, alternate },
        }
    }

    /// The slot of the unverified tier: the app the chain located, if any.
    #[must_use]
    pub const fn found(found: Option<&'a AppSlot<'f, FoundApp>>) -> Self {
        Self {
            slots: Slots::Found(found),
        }
    }

    /// The slot that booted, in the layout and boot tiers.
    #[must_use]
    pub const fn current(&self) -> Option<&'a AppSlot<'f, CurrentApp>> {
        match self.slots {
            Slots::Layout { current, .. } => Some(current),
            Slots::Found(_) => None,
        }
    }

    /// The other slot, in the layout and boot tiers.
    #[must_use]
    pub const fn alternate(&self) -> Option<&'a AppSlot<'f, AlternateApp>> {
        match self.slots {
            Slots::Layout { alternate, .. } => Some(alternate),
            Slots::Found(_) => None,
        }
    }

    /// The app the chain located, in the unverified tier.
    #[must_use]
    pub const fn found_app(&self) -> Option<&'a AppSlot<'f, FoundApp>> {
        match self.slots {
            Slots::Found(found) => found,
            Slots::Layout { .. } => None,
        }
    }

    /// The checks of every slot, for good: verifies each slot
    /// ([`AppSlot::check`]), the current one first, then waits until a
    /// session on one of them or on its signature region ends at a new
    /// epoch, and checks again. Hashes with clones of `sha`, a hash not yet
    /// fed, and after each KiB hashed calls `progress`, then `pace`; reads
    /// the signature regions through `buf`. Before each wait, for a change
    /// or for an open session to end, it calls `progress`'s
    /// [`idle`](Progress::idle): once every slot's result of its epoch is
    /// kept, and with no slot, at once.
    pub async fn run<H, P, G>(&self, sha: H, pace: &mut P, progress: &mut G, buf: &mut FileBuf) -> !
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        loop {
            match self.slots {
                Slots::Layout { current, alternate } => {
                    // Taken before the checks, so a session that ends during
                    // them completes the wait.
                    let current_changed = current.changed();
                    let alternate_changed = alternate.changed();
                    current.check(&sha, pace, progress, buf).await;
                    alternate.check(&sha, pace, progress, buf).await;
                    progress.idle();
                    either(current_changed, alternate_changed).await;
                }
                Slots::Found(Some(found)) => {
                    let changed = found.changed();
                    found.check(&sha, pace, progress, buf).await;
                    progress.idle();
                    changed.await;
                }
                Slots::Found(None) => {
                    progress.idle();
                    core::future::pending::<()>().await;
                }
            }
        }
    }
}

/// Completes once `a` or `b` does.
async fn either(a: impl Future<Output = ()>, b: impl Future<Output = ()>) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    poll_fn(|cx| {
        if a.as_mut().poll(cx).is_ready() || b.as_mut().poll(cx).is_ready() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}

#[cfg(test)]
mod tests;
