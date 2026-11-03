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
//! called after each KiB. The check runs at boot and again after each write
//! session on the slot that ends at a new epoch. A check a session
//! interrupts keeps no result and runs again at the new epoch. The slot
//! keeps the last result, its model ([`AppModel`]), for the epoch it was
//! verified at; [`AppSlot::verdict`] gives the result at a given epoch once
//! the check has kept it, so no image is hashed twice for one epoch.
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
use eveningstar_storage::nor::{Published, Reader, Snapshot};
use eveningstar_storage::roles::{AlternateApp, CurrentApp, FoundApp, Internal, Publishable};
use eveningstar_storage::{Epoch, Span, WriteState};

use crate::error::{ImageFailure, LoadError, Unpublished, at, failure, publish, verdict, whole};
use crate::pace::{Paced, Progress};

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
}

impl<R> Clone for AppModel<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for AppModel<R> {}

impl<R> PartialEq for AppModel<R> {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch && self.image == other.image
    }
}

impl<R> Eq for AppModel<R> {}

impl<R> fmt::Debug for AppModel<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppModel")
            .field("epoch", &self.epoch)
            .field("image", &self.image)
            .finish()
    }
}

/// What a slot keeps: the last result and the tasks waiting for one.
struct Kept<R> {
    model: Option<AppModel<R>>,
    waiters: MultiWakerRegistration<WAITERS>,
}

/// The content module of one app region, with role `R`.
pub struct AppSlot<'f, R: AppRole> {
    reader: Reader<'f, R>,
    kept: CriticalSectionMutex<RefCell<Kept<R>>>,
}

impl<'f, R: AppRole> AppSlot<'f, R> {
    /// The module of the app region `reader` reads; nothing is verified
    /// yet.
    #[must_use]
    pub const fn new(reader: Reader<'f, R>) -> Self {
        Self {
            reader,
            kept: CriticalSectionMutex::new(RefCell::new(Kept {
                model: None,
                waiters: MultiWakerRegistration::new(),
            })),
        }
    }

    /// The region's span, from the start of the flash.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.reader.span()
    }

    /// The region's write state now: the epoch to ask
    /// [`verdict`](Self::verdict) for.
    #[must_use]
    pub fn write_state(&self) -> WriteState {
        self.reader.write_state()
    }

    /// The last result, while the region is stable at its epoch. Never
    /// inlined, so its copies stay out of the callers' frames.
    #[must_use]
    #[inline(never)]
    pub fn model(&self) -> Option<AppModel<R>> {
        let model = self.kept.lock(|kept| kept.borrow().model)?;
        (self.reader.write_state() == WriteState::Stable(model.epoch)).then_some(model)
    }

    /// The result of the check at `epoch`: the model of the region as of
    /// `epoch`, once the slot's check has verified it.
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
        // Registered before the result is looked at, in one critical section
        // with the look, so a result kept after it wakes the task.
        let kept = self.kept.lock(|kept| {
            let mut kept = kept.borrow_mut();
            kept.waiters.register(cx.waker());
            kept.model
        });
        // Completes only once the region left `epoch` for good, which the
        // look at the state below then sees, so it is never polled again
        // after it completes.
        let _ = changed.poll(cx);
        if self.reader.write_state() != WriteState::Stable(epoch) {
            return Poll::Ready(Err(LoadError::Stale));
        }
        match kept {
            Some(model) if model.epoch == epoch => Poll::Ready(Ok(model)),
            _ => Poll::Pending,
        }
    }

    /// Verifies the slot unless the result kept is of the region's epoch
    /// now, and keeps the result. Hashes with a clone of `sha`, a hash not
    /// yet fed, and after each KiB hashed calls `progress`, then `pace`.
    ///
    /// While a session is open on the region, it waits for the session to
    /// end. When a session writes the region during the verification, the
    /// verification keeps no result and runs again at the new epoch. It
    /// completes once a result of the region's epoch is kept.
    pub async fn check<H, P, G>(&self, sha: &H, pace: &mut P, progress: &mut G)
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        loop {
            match self.reader.write_state() {
                WriteState::Writing(_) => self.settle().await,
                WriteState::Stable(epoch) => {
                    if self.has(epoch) {
                        return;
                    }
                    // Refused only while a session is open, which the next
                    // round waits out.
                    if let Ok(snapshot) = whole(&self.reader) {
                        let verified =
                            verify(&snapshot, sha.clone(), Paced::new(pace, progress)).await;
                        self.keep(&snapshot, &verified);
                    }
                }
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

    /// Whether the result kept is of `epoch`.
    #[inline(never)]
    fn has(&self, epoch: Epoch) -> bool {
        self.kept.lock(|kept| {
            kept.borrow()
                .model
                .is_some_and(|model| model.epoch == epoch)
        })
    }

    /// Keeps the result of the image `snapshot` reads, which verified as
    /// `verified` says, and wakes the tasks waiting for one; nothing when a
    /// read found the region written, as that is no result for the epoch.
    /// Never inlined, so the model is assembled outside the check's frame.
    #[inline(never)]
    fn keep(&self, snapshot: &Snapshot<'_, R>, verified: &Result<image::App, ImageError>) {
        let image = match verified {
            Ok(app) => Ok(VerifiedApp {
                header: *app.header(),
                size: app.size(),
                digest: *app.digest(),
                file_sha256: *app.file_sha256(),
            }),
            Err(error) => match failure(*error) {
                Ok(failure) => Err(failure),
                Err(_) => return,
            },
        };
        let model = AppModel {
            epoch: snapshot.epoch(),
            image,
            role: PhantomData,
        };
        self.kept.lock(|kept| {
            let mut kept = kept.borrow_mut();
            kept.model = Some(model);
            kept.waiters.wake();
        });
    }

    /// The descriptor of the image `model` describes, read again at the
    /// model's epoch.
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
        verdict(image::app_descriptor(&snapshot))
    }

    /// The view of the image `model` describes, the digest included: the
    /// region's first [`VerifiedApp::size`] bytes.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] when the image doesn't verify,
    /// [`Unpublished::Stale`] once the region isn't stable at the model's
    /// epoch, and [`Unpublished::Refused`] for the storage crate's other
    /// refusals.
    pub fn published(&self, model: &AppModel<R>) -> Result<Published<'_, R>, Unpublished> {
        let Ok(image) = &model.image else {
            return Err(Unpublished::Unverified);
        };
        publish(&self.reader, model.epoch, Span::new(0, image.size), &[]).map_err(Unpublished::from)
    }
}

impl<R: AppRole> fmt::Debug for AppSlot<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppSlot")
            .field("reader", &self.reader)
            .field("model", &self.kept.lock(|kept| kept.borrow().model))
            .finish()
    }
}

/// Verifies the image `snapshot` reads, hashing with `sha` and pausing with
/// `paced`.
async fn verify<R: AppRole, P: Pace>(
    snapshot: &Snapshot<'_, R>,
    sha: impl Sha256Hasher,
    mut paced: P,
) -> Result<image::App, ImageError> {
    image::verify_app_with(snapshot, sha, &mut paced).await
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
    /// session on one of them ends at a new epoch, and verifies again.
    /// Hashes with clones of `sha`, a hash not yet fed, and after each KiB
    /// hashed calls `progress`, then `pace`.
    pub async fn run<H, P, G>(&self, sha: H, pace: &mut P, progress: &mut G) -> !
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
                    let current_changed = current.reader.changed();
                    let alternate_changed = alternate.reader.changed();
                    current.check(&sha, pace, progress).await;
                    alternate.check(&sha, pace, progress).await;
                    either(current_changed, alternate_changed).await;
                }
                Slots::Found(Some(found)) => {
                    let changed = found.reader.changed();
                    found.check(&sha, pace, progress).await;
                    changed.await;
                }
                Slots::Found(None) => core::future::pending().await,
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
