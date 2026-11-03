//! The bootloader: the image at 0x0, where the ROM loads it from.
//!
//! [`Bootloader`] owns the reader of the bootloader's region, from 0x0 up to
//! the located table. Its model is the header, read whether or not the image
//! verifies, and the image verified within the region: its length, the
//! SHA-256 it stores, the SHA-256 of the whole image and its entry among the
//! known bootloaders. Its descriptor is read again at the model's epoch
//! ([`Bootloader::descriptor`]), so the model stays small. Its view is the
//! image, the digest included, once that digest verifies, whether or not the
//! bootloader is a known one; nothing else of the region is published.
//!
//! An image that runs past the region, into the table, doesn't verify within
//! it, and so isn't published. An image that doesn't verify keeps no value
//! read from it but its header's ([`ImageFailure`]).
//!
//! Only the module's parser makes a model:
//!
//! ```compile_fail,E0451
//! use eveningstar_boot::bootloader::BootloaderModel;
//!
//! fn forge(model: BootloaderModel) -> BootloaderModel {
//!     BootloaderModel { flash_size_code: Some(2), ..model }
//! }
//! ```

use core::cell::Cell;
use core::fmt;

use embassy_sync::blocking_mutex::CriticalSectionMutex;
use eveningstar_esp_formats::hash::Sha256Hasher;
use eveningstar_esp_formats::image::{self, BootloaderDescriptor, HEADER_LEN, Header, ImageError};
use eveningstar_esp_formats::pace::Pace;
use eveningstar_storage::known::{self, Known};
use eveningstar_storage::nor::{Published, Reader, Snapshot};
use eveningstar_storage::{Epoch, Source as _, Span, WriteState, roles};

use crate::error::{ImageFailure, LoadError, Unpublished, at, failure, publish, verdict, whole};
use crate::pace::{Paced, Progress};

/// A bootloader image that verified within its region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifiedBootloader {
    size: u32,
    digest: [u8; 32],
    file_sha256: [u8; 32],
    known: Option<&'static Known>,
}

impl VerifiedBootloader {
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

    /// Its entry among the known bootloaders, found by its digest.
    #[must_use]
    pub const fn known(&self) -> Option<&'static Known> {
        self.known
    }
}

/// What the bootloader's region holds, as of one epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootloaderModel {
    epoch: Epoch,
    flash_size_code: Option<u8>,
    header: Result<Header, ImageFailure>,
    image: Result<VerifiedBootloader, ImageFailure>,
}

impl BootloaderModel {
    /// The region's epoch the model was parsed at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// The header's flash size code, read whatever the rest of the header
    /// holds; `None` when the header can't be read.
    #[must_use]
    pub const fn flash_size_code(&self) -> Option<u8> {
        self.flash_size_code
    }

    /// The chip size the bootloader checks the table against, which the
    /// flash size code gives ([`image::flash_size`]).
    #[must_use]
    pub const fn chip_size(&self) -> Option<u32> {
        match self.flash_size_code {
            Some(code) => Some(image::flash_size(code)),
            None => None,
        }
    }

    /// The header, or why it isn't one.
    ///
    /// # Errors
    ///
    /// Why the header doesn't parse, or the read that failed.
    pub const fn header(&self) -> Result<&Header, ImageFailure> {
        match &self.header {
            Ok(header) => Ok(header),
            Err(failure) => Err(*failure),
        }
    }

    /// The image, verified within the region, or why it doesn't verify.
    ///
    /// # Errors
    ///
    /// The first check that fails.
    pub const fn image(&self) -> Result<&VerifiedBootloader, ImageFailure> {
        match &self.image {
            Ok(image) => Ok(image),
            Err(failure) => Err(*failure),
        }
    }
}

/// The bootloader's content module: its region's reader and the model last
/// parsed.
pub struct Bootloader<'f> {
    reader: Reader<'f, roles::Bootloader>,
    model: CriticalSectionMutex<Cell<Option<BootloaderModel>>>,
}

impl<'f> Bootloader<'f> {
    /// The module of the bootloader's region, which `reader` reads; nothing
    /// is parsed yet.
    #[must_use]
    pub const fn new(reader: Reader<'f, roles::Bootloader>) -> Self {
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
    /// Never inlined, so its copies stay out of the callers' frames.
    #[must_use]
    #[inline(never)]
    pub fn model(&self) -> Option<BootloaderModel> {
        let model = self.model.lock(Cell::get)?;
        (self.reader.write_state() == WriteState::Stable(model.epoch)).then_some(model)
    }

    /// Parses the region unless the model kept is of its epoch now, and
    /// keeps the new model, which [`model`](Self::model) then gives. Parsing
    /// reads the header, then verifies the image within the region, hashing
    /// with `sha`, a hash not yet fed, and after each KiB hashed calling
    /// `progress`, then `pace`.
    ///
    /// # Errors
    ///
    /// [`LoadError::Busy`] while a write session is open on the region, and
    /// [`LoadError::Stale`] when one wrote it while it was parsed.
    pub async fn load<H, P, G>(
        &self,
        sha: H,
        pace: &mut P,
        progress: &mut G,
    ) -> Result<(), LoadError>
    where
        H: Sha256Hasher,
        P: Pace,
        G: Progress,
    {
        if self.is_current() {
            return Ok(());
        }
        let snapshot = whole(&self.reader)?;
        let verified = verify(&snapshot, sha, Paced::new(pace, progress)).await;
        self.keep(&snapshot, &verified)
    }

    /// Whether the model kept is of the region's epoch now.
    #[inline(never)]
    fn is_current(&self) -> bool {
        self.model().is_some()
    }

    /// Makes the model of the region `snapshot` reads, whose image verified
    /// as `verified` says, and keeps it. Never inlined, so the model is
    /// assembled outside the caller's frame.
    #[inline(never)]
    fn keep(
        &self,
        snapshot: &Snapshot<'_, roles::Bootloader>,
        verified: &Result<image::Bootloader, ImageError>,
    ) -> Result<(), LoadError> {
        let model = model_of(snapshot, verified)?;
        self.model.lock(|kept| kept.set(Some(model)));
        Ok(())
    }

    /// The descriptor of the image `model` describes, read again at the
    /// model's epoch: `None` for a bootloader without one, as ESP-IDF
    /// releases before 5.3 build them.
    ///
    /// # Errors
    ///
    /// [`LoadError::Stale`] once the region isn't stable at the model's
    /// epoch, and [`LoadError::Busy`] while a session is open on it; the
    /// image's failure when it doesn't verify.
    pub fn descriptor(
        &self,
        model: &BootloaderModel,
    ) -> Result<Result<Option<BootloaderDescriptor>, ImageFailure>, LoadError> {
        if let Err(failure) = model.image {
            return Ok(Err(failure));
        }
        let snapshot = at(&self.reader, model.epoch)?;
        verdict(image::bootloader_descriptor(&snapshot))
    }

    /// The view of the image `model` describes, the digest included: the
    /// region's first [`VerifiedBootloader::size`] bytes.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] when the image doesn't verify,
    /// [`Unpublished::Stale`] once the region isn't stable at the model's
    /// epoch, and [`Unpublished::Refused`] for the storage crate's other
    /// refusals.
    pub fn published(
        &self,
        model: &BootloaderModel,
    ) -> Result<Published<'_, roles::Bootloader>, Unpublished> {
        let Ok(image) = &model.image else {
            return Err(Unpublished::Unverified);
        };
        publish(&self.reader, model.epoch, Span::new(0, image.size), &[]).map_err(Unpublished::from)
    }
}

/// Verifies the image `snapshot` reads, hashing with `sha` and pausing with
/// `paced`.
async fn verify<P: Pace>(
    snapshot: &Snapshot<'_, roles::Bootloader>,
    sha: impl Sha256Hasher,
    mut paced: P,
) -> Result<image::Bootloader, ImageError> {
    image::verify_bootloader_with(snapshot, sha, &mut paced).await
}

/// The model of the bootloader's region that `snapshot` reads, whose image
/// verified as `verified` says. Never inlined, so the model is assembled
/// outside the parse's frame.
#[inline(never)]
fn model_of(
    snapshot: &Snapshot<'_, roles::Bootloader>,
    verified: &Result<image::Bootloader, ImageError>,
) -> Result<BootloaderModel, LoadError> {
    let mut bytes = [0; HEADER_LEN as usize];
    let (flash_size_code, header) = match snapshot.read(0, &mut bytes) {
        Ok(()) => (
            Some(image::flash_size_code(&bytes)),
            Header::parse(&bytes).map_err(ImageFailure::from),
        ),
        Err(error) => (None, Err(failure(error.into())?)),
    };
    let image = match verified {
        Ok(verified) => Ok(VerifiedBootloader {
            size: verified.size(),
            digest: *verified.digest(),
            file_sha256: *verified.file_sha256(),
            known: known::find(verified.digest()),
        }),
        Err(error) => Err(failure(*error)?),
    };
    Ok(BootloaderModel {
        epoch: snapshot.epoch(),
        flash_size_code,
        header,
        image,
    })
}

impl fmt::Debug for Bootloader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bootloader")
            .field("reader", &self.reader)
            .field("model", &self.model.lock(Cell::get))
            .finish()
    }
}

#[cfg(test)]
mod tests;
