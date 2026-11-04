//! Why OTA is unavailable, and why an operation was refused or failed.

use eveningstar_boot::ImageFailure;
use eveningstar_esp_formats::otadata::PlanError;
use eveningstar_storage::AccessError;

/// Why [`Ota`](crate::Ota) refuses every write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// The running app's image doesn't verify, or its descriptor doesn't
    /// read: there is no project name for an update to match.
    Descriptor(ImageFailure),
    /// The running app's slot wasn't stable at one epoch while its check and
    /// its descriptor were read.
    Unstable,
}

/// Why an OTA operation was refused or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaError {
    /// OTA is unavailable. Nothing was written.
    Unavailable(Unavailable),
    /// The otadata doesn't allow the operation, or a sector it keeps couldn't
    /// be read.
    Plan(PlanError),
    /// The image in the alternate slot doesn't verify, or its descriptor
    /// doesn't read.
    Image(ImageFailure),
    /// An erase or program was refused or failed.
    Access(AccessError),
    /// The [`VerifiedImage`](crate::VerifiedImage) was made before the latest
    /// [`Ota::begin_update`](crate::Ota::begin_update).
    Stale,
    /// The alternate slot was written since it verified: its epoch moved, or
    /// its write state can't record an epoch.
    Changed,
    /// The image's project name isn't the running app's.
    Project,
    /// The image embeds no layout record right after its descriptor, or one
    /// of a layout other than the compiled one.
    Layout,
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

impl From<Unavailable> for OtaError {
    fn from(reason: Unavailable) -> Self {
        Self::Unavailable(reason)
    }
}

impl From<PlanError> for OtaError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

impl From<ImageFailure> for OtaError {
    fn from(failure: ImageFailure) -> Self {
        Self::Image(failure)
    }
}

impl From<AccessError> for OtaError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}
