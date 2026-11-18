//! The signature regions: the signature file each app slot's image was
//! installed with.
//!
//! A [`SignatureRegion`] owns the reader of one signature region: the one
//! paired with the slot that booted ([`CurrentSignature`]) or with the other
//! one ([`AlternateSignature`]), in the layout and boot tiers.
//!
//! Its model ([`SignatureModel`]) is what the region holds, as of one epoch
//! ([`Content`]):
//!
//! - every byte `0xFF`: no file ([`Content::Unsigned`]);
//! - a signature file that parses ([`eveningstar_signify::parse`]), then
//!   `0xFF` to the end of the region ([`Content::File`]). The file ends after
//!   its last line: at the first `0xFF` that follows a `\n`, as no line of a
//!   file starts with one;
//! - anything else ([`Content::Invalid`]), such as a region that starts with
//!   `0xFF` and holds other bytes later, as a wipe a power cut interrupts can
//!   leave it.
//!
//! Its view is the file, once it parsed, whatever key signed it, with the
//! comment's text sanitized: the comment isn't signed, and is never read. The
//! padding and the rest of the region are never published.
//!
//! Whether the file verifies under a trust set and lists an image's SHA-256
//! is separate ([`SignatureRegion::check`]), and read again at the model's
//! epoch on each call ([`Signature`]).
//!
//! Reading a region takes a buffer of [`FILE_MAX`] bytes from the caller, so
//! no frame holds one.
//!
//! A model is of one region's role:
//!
//! ```compile_fail,E0308
//! use eveningstar_boot::signature::{SignatureModel, SignatureRegion};
//! use eveningstar_storage::roles::{AlternateSignature, CurrentSignature};
//!
//! fn export<'s>(
//!     region: &'s SignatureRegion<'_, CurrentSignature>,
//!     model: &SignatureModel<AlternateSignature>,
//! ) {
//!     let _ = region.published(model);
//! }
//! ```
//!
//! and only its parser makes one:
//!
//! ```compile_fail,E0451
//! use eveningstar_boot::signature::{Content, SignatureModel};
//! use eveningstar_storage::roles::CurrentSignature;
//!
//! fn forge(model: SignatureModel<CurrentSignature>) -> SignatureModel<CurrentSignature> {
//!     SignatureModel { content: Content::Unsigned, ..model }
//! }
//! ```
//!
//! [`FILE_MAX`]: eveningstar_signify::FILE_MAX

use core::cell::Cell;
use core::fmt;
use core::marker::PhantomData;

use embassy_sync::blocking_mutex::CriticalSectionMutex;
use eveningstar_signify::{FILE_MAX, Keynum, ManifestError, SigError, TrustSet, TrustedKey};
use eveningstar_storage::nor::{Published, Reader, Snapshot};
use eveningstar_storage::roles::{AlternateSignature, CurrentSignature, Internal, Publishable};
use eveningstar_storage::{Epoch, ReadError, Source as _, Span, WriteState, layout};

use crate::error::{LoadError, Unpublished, at, publish, whole};

mod sealed {
    /// Implemented by the signature roles only.
    pub trait Sealed {}
}

/// A role of a signature region: [`CurrentSignature`] or
/// [`AlternateSignature`].
pub trait SignatureRole: Publishable<Device = Internal> + sealed::Sealed {}

impl sealed::Sealed for CurrentSignature {}
impl sealed::Sealed for AlternateSignature {}
impl SignatureRole for CurrentSignature {}
impl SignatureRole for AlternateSignature {}

// A region holds one file of the longest a reader accepts, and is read whole
// into a buffer of that length.
const _: () = assert!(layout::SIG_0.size as usize == FILE_MAX);
const _: () = assert!(layout::SIG_1.size as usize == FILE_MAX);

/// The buffer a region is read into: [`FILE_MAX`] bytes.
///
/// [`FILE_MAX`]: eveningstar_signify::FILE_MAX
pub type FileBuf = [u8; FILE_MAX];

/// A signature file that parsed in a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignatureFile {
    len: u32,
    keynum: Keynum,
    comment: Span,
}

impl SignatureFile {
    /// The file's length in bytes, from the start of the region.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// Whether the file is empty, which a file that parsed never is.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The keynum of the key the file names.
    #[must_use]
    pub const fn keynum(&self) -> &Keynum {
        &self.keynum
    }

    /// Where the comment's text lies, from the start of the region.
    #[must_use]
    pub const fn comment(&self) -> Span {
        self.comment
    }
}

/// Why a region's signature is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// A read of the region failed.
    Read,
    /// A byte after the file isn't `0xFF`.
    Padding,
    /// The file doesn't parse: the first check that fails.
    Format(SigError),
    /// The signature doesn't verify under the trusted key with its keynum.
    Signature,
    /// More than one line of the verified manifest has the image's digest.
    Ambiguous,
}

/// What a signature region holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// Every byte is `0xFF`.
    Unsigned,
    /// A signature file that parses, then `0xFF` to the end of the region.
    File(SignatureFile),
    /// Anything else: [`Invalid::Read`], [`Invalid::Padding`] or
    /// [`Invalid::Format`].
    Invalid(Invalid),
}

/// What a signature region holds, as of one epoch, for the region with role
/// `R`.
pub struct SignatureModel<R> {
    epoch: Epoch,
    content: Content,
    role: PhantomData<fn() -> R>,
}

impl<R> SignatureModel<R> {
    /// The region's epoch the model was parsed at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// What the region holds.
    #[must_use]
    pub const fn content(&self) -> &Content {
        &self.content
    }
}

impl<R> Clone for SignatureModel<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for SignatureModel<R> {}

impl<R> PartialEq for SignatureModel<R> {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch && self.content == other.content
    }
}

impl<R> Eq for SignatureModel<R> {}

impl<R> fmt::Debug for SignatureModel<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignatureModel")
            .field("epoch", &self.epoch)
            .field("content", &self.content)
            .finish()
    }
}

/// A region's signature, judged under a trust set for an image's SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    /// The file verifies under `key`, and exactly one line of its manifest,
    /// the `line`th from 0, has the image's digest.
    Verified {
        /// The key the signature verified under.
        key: &'static TrustedKey,
        /// The index of the line with the image's digest.
        line: usize,
    },
    /// The file verifies under `key`, but no line of its manifest has the
    /// image's digest: a file another image left, or no image's.
    Stale {
        /// The key the signature verified under.
        key: &'static TrustedKey,
    },
    /// The file parses, but no key of the trust set has its keynum.
    UnknownKey {
        /// The keynum the file names.
        keynum: Keynum,
    },
    /// The region holds no file.
    Unsigned,
    /// The region holds something other than a file, or the file's
    /// signature doesn't verify, or names the image twice.
    Invalid(Invalid),
}

/// A signature region's content module: its reader and the model last
/// parsed.
pub struct SignatureRegion<'f, R: SignatureRole> {
    reader: Reader<'f, R>,
    model: CriticalSectionMutex<Cell<Option<SignatureModel<R>>>>,
}

impl<'f, R: SignatureRole> SignatureRegion<'f, R> {
    /// The module of the region `reader` reads; nothing is parsed yet.
    #[must_use]
    pub const fn new(reader: Reader<'f, R>) -> Self {
        Self {
            reader,
            model: CriticalSectionMutex::new(Cell::new(None)),
        }
    }

    /// The region's span, from the start of the flash.
    #[must_use]
    pub const fn span(&self) -> Span {
        self.reader.span()
    }

    /// The model last parsed, while the region is stable at its epoch.
    #[must_use]
    pub fn model(&self) -> Option<SignatureModel<R>> {
        let model = self.model.lock(Cell::get)?;
        (self.reader.write_state() == WriteState::Stable(model.epoch)).then_some(model)
    }

    /// Parses the region, read whole into `buf`, unless the model kept is of
    /// its epoch now, and keeps the new model, which
    /// [`model`](Self::model) then gives.
    ///
    /// # Errors
    ///
    /// [`LoadError::Busy`] while a write session is open on the region, and
    /// [`LoadError::Stale`] when one wrote it while it was read.
    pub fn load(&self, buf: &mut FileBuf) -> Result<(), LoadError> {
        if self.model().is_some() {
            return Ok(());
        }
        let snapshot = whole(&self.reader)?;
        let content = content_of(&snapshot, buf)?;
        let model = SignatureModel {
            epoch: snapshot.epoch(),
            content,
            role: PhantomData,
        };
        self.model.lock(|kept| kept.set(Some(model)));
        Ok(())
    }

    /// The signature of the region `model` describes, under `trust`, for the
    /// image whose whole file has the SHA-256 `file_sha256`. A file is read
    /// again into `buf` at the model's epoch and verified.
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] once the region isn't stable at the model's
    /// epoch, and [`LoadError::Busy`] while a session is open on it.
    pub fn check(
        &self,
        model: &SignatureModel<R>,
        trust: &TrustSet,
        file_sha256: &[u8; 32],
        buf: &mut FileBuf,
    ) -> Result<Signature, LoadError> {
        let file = match model.content {
            Content::Unsigned => return Ok(Signature::Unsigned),
            Content::Invalid(invalid) => return Ok(Signature::Invalid(invalid)),
            Content::File(file) => file,
        };
        let snapshot = at(&self.reader, model.epoch)?;
        let Some(bytes) = buf.get_mut(..file.len as usize) else {
            return Ok(Signature::Invalid(Invalid::Read));
        };
        match snapshot.read(0, bytes) {
            Ok(()) => {}
            Err(ReadError::Stale) => return Err(LoadError::Stale),
            Err(ReadError::Device | ReadError::OutOfRange) => {
                return Ok(Signature::Invalid(Invalid::Read));
            }
        }
        Ok(judge(bytes, trust, file_sha256))
    }

    /// The view of the file `model` describes, its comment's text served as
    /// `0xFF`: the region's first [`SignatureFile::len`] bytes.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] when the region holds no file that
    /// parsed, [`Unpublished::Stale`] once the region isn't stable at the
    /// model's epoch, and [`Unpublished::Refused`] for the storage crate's
    /// other refusals.
    pub fn published(&self, model: &SignatureModel<R>) -> Result<Published<'_, R>, Unpublished> {
        let Content::File(file) = model.content else {
            return Err(Unpublished::Unverified);
        };
        publish(
            &self.reader,
            model.epoch,
            Span::new(0, file.len),
            &[file.comment],
        )
        .map_err(Unpublished::from)
    }
}

/// What the region `snapshot` reads holds, read whole into `buf`. Never
/// inlined, so the parse stays out of the caller's frame.
///
/// # Errors
///
/// [`LoadError::Stale`] when the region was written while it was read.
#[inline(never)]
fn content_of<R: SignatureRole>(
    snapshot: &Snapshot<'_, R>,
    buf: &mut FileBuf,
) -> Result<Content, LoadError> {
    let len = usize::try_from(snapshot.size()).unwrap_or(usize::MAX);
    let Some(bytes) = buf.get_mut(..len) else {
        return Ok(Content::Invalid(Invalid::Read));
    };
    match snapshot.read(0, bytes) {
        Ok(()) => Ok(parse_region(bytes)),
        Err(ReadError::Stale) => Err(LoadError::Stale),
        Err(ReadError::Device | ReadError::OutOfRange) => Ok(Content::Invalid(Invalid::Read)),
    }
}

/// What the region whose bytes are `region` holds.
fn parse_region(region: &[u8]) -> Content {
    if region.iter().all(|byte| *byte == 0xFF) {
        return Content::Unsigned;
    }
    // No line of a file starts with `0xFF`, so the file ends at the first
    // one that follows a newline, or at the end of the region.
    let end = region
        .windows(2)
        .position(|pair| pair == [b'\n', 0xFF])
        .map_or(region.len(), |at| at + 1);
    let (file, padding) = region.split_at(end);
    if padding.iter().any(|byte| *byte != 0xFF) {
        return Content::Invalid(Invalid::Padding);
    }
    match eveningstar_signify::parse(file) {
        Ok(parsed) => {
            let comment = parsed.comment();
            Content::File(SignatureFile {
                len: span_u32(end),
                keynum: *parsed.keynum(),
                comment: Span::new(span_u32(comment.start), span_u32(comment.len())),
            })
        }
        Err(error) => Content::Invalid(Invalid::Format(error)),
    }
}

/// The signature of the file `file`, which parsed, under `trust`, for the
/// image with the SHA-256 `file_sha256`. Never inlined, so the verification
/// stays out of the caller's frame.
#[inline(never)]
fn judge(file: &[u8], trust: &TrustSet, file_sha256: &[u8; 32]) -> Signature {
    let verified = match eveningstar_signify::verify(file, trust) {
        Ok(verified) => verified,
        Err(SigError::UnknownKey { keynum }) => return Signature::UnknownKey { keynum },
        Err(SigError::Signature) => return Signature::Invalid(Invalid::Signature),
        // The file parsed at this epoch, so this is no other refusal; one
        // would still be a format.
        Err(error) => return Signature::Invalid(Invalid::Format(error)),
    };
    let key = verified.key();
    match verified.line(file_sha256) {
        Ok(found) => {
            let line = verified
                .lines()
                .position(|line| line == found)
                .unwrap_or_default();
            Signature::Verified { key, line }
        }
        Err(ManifestError::NoMatch) => Signature::Stale { key },
        Err(ManifestError::Ambiguous) => Signature::Invalid(Invalid::Ambiguous),
    }
}

/// `value`, an offset or length within a region of [`FILE_MAX`] bytes.
///
/// [`FILE_MAX`]: eveningstar_signify::FILE_MAX
fn span_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl<R: SignatureRole> fmt::Debug for SignatureRegion<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignatureRegion")
            .field("reader", &self.reader)
            .field("model", &self.model.lock(Cell::get))
            .finish()
    }
}

#[cfg(test)]
mod tests;
