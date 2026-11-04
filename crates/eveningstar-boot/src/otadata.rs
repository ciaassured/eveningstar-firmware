//! The otadata: ESP-IDF's two copies of the boot selection.
//!
//! [`Otadata`] owns the reader of the otadata partition: the compiled
//! layout's in the layout and boot tiers, or the one the located table gives
//! in the unverified tier.
//!
//! Its model, [`OtadataModel`], holds both entries with every field as read,
//! labels included, and the epoch, and gives the active entry and the boot
//! state; it stays inside the device. [`OtadataModel::fields`] gives the
//! fields that may leave the device, as a type of its own
//! ([`OtadataFields`]), so a function that sends them takes that type and
//! nothing that holds more. For each entry, its validity always; and only
//! when its CRC matches, its sequence number, the slot it selects, and its
//! state when that is a value ESP-IDF defines. A blank entry is given as
//! blank, a corrupt one by its validity alone. The label and an unknown
//! state value are never given.
//!
//! [`Otadata::published`] gives the view of one entry of the compiled
//! otadata, its 32 bytes at the model's epoch, when the parser pins down
//! every byte of it:
//!
//! - an entry whose 32 bytes are all `0xFF`;
//! - an entry whose sequence number isn't `0xFFFFFFFF`, whose CRC matches it
//!   and whose state is a value ESP-IDF defines, with its 20-byte label
//!   served as `0xFF`, and listed as sanitized when the flash holds something
//!   else there.
//!
//! Each byte published is then proven by the CRC (the sequence number), a
//! function of it (the CRC), one of a known set of values (the state) or a
//! constant (the label, as served), so the entry reveals nothing beyond the
//! fields [`OtadataFields`] gives. The bootloader ignores the label, and the
//! planner writes it erased. Any other entry has no view: a corrupt one, one
//! with an unknown state value, and one whose sequence number is
//! `0xFFFFFFFF` while its other bytes aren't all `0xFF`, as its fields give
//! no state. The rest of each sector, after its entry, is never published,
//! and neither is the otadata a located table gives.
//!
//! A function that takes the public fields can't be given the model:
//!
//! ```compile_fail,E0308
//! use eveningstar_boot::otadata::{OtadataFields, OtadataModel};
//!
//! fn send(_: &OtadataFields) {}
//!
//! fn serve(model: &OtadataModel) {
//!     send(model);
//! }
//! ```
//!
//! The public fields have no label, no epoch and no unknown state:
//!
//! ```compile_fail,E0609
//! use eveningstar_boot::otadata::OtadataFields;
//! use eveningstar_esp_formats::otadata::Sector;
//!
//! fn send(fields: &OtadataFields) {
//!     let _ = fields.entry(Sector::First).unwrap().label;
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_boot::otadata::OtadataFields;
//!
//! fn send(fields: &OtadataFields) {
//!     let _ = fields.epoch();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! use eveningstar_boot::otadata::PublicState;
//!
//! const STATE: PublicState = PublicState::Unknown(5);
//! ```
//!
//! and only the module makes them:
//!
//! ```compile_fail,E0451
//! use eveningstar_boot::otadata::EntryFields;
//! use eveningstar_esp_formats::otadata::Validity;
//!
//! const FORGED: EntryFields = EntryFields { validity: Validity::Blank, seq: None, slot: None, state: None };
//! ```

use eveningstar_esp_formats::otadata::{
    self as format, BLANK_SEQ, BootState, Sector, SelectEntry, State, Validity,
};
use eveningstar_storage::nor::{Published, Reader};
use eveningstar_storage::roles::{self, FoundOtadata, Internal, Role};
use eveningstar_storage::{Epoch, ReadError, Slot, Span};

use crate::error::{LoadError, Unpublished, publish, whole};

/// Length of an entry, and of its view, in bytes.
const ENTRY_LEN: u32 = 32;

/// The label's span, relative to the entry.
const LABEL: Span = Span::new(4, 20);

/// Whether an entry judged `validity` has a sequence number other than
/// [`BLANK_SEQ`] and a CRC that matches it.
const fn crc_matches(validity: Validity) -> bool {
    matches!(
        validity,
        Validity::Valid | Validity::Invalid | Validity::Aborted
    )
}

/// Whether the parser pins down every byte of `entry`, the label served as
/// `0xFF`: all 32 bytes are `0xFF`, or its CRC matches and its state is a
/// value ESP-IDF defines.
const fn pinned(entry: &SelectEntry) -> bool {
    let erased = entry.seq == BLANK_SEQ
        && matches!(entry.state, State::Undefined)
        && entry.crc == u32::MAX
        && label_erased(&entry.label);
    erased || (crc_matches(entry.validity()) && PublicState::of(entry.state).is_some())
}

/// Whether every byte of `label` is `0xFF`.
const fn label_erased(label: &[u8; 20]) -> bool {
    let mut at = 0;
    while at < label.len() {
        if label[at] != 0xFF {
            return false;
        }
        at += 1;
    }
    true
}

/// An entry's image state, when it is a value ESP-IDF defines
/// (`esp_ota_img_states_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublicState {
    /// Selected, not yet booted.
    New,
    /// Booted once, not yet confirmed.
    PendingVerify,
    /// Confirmed as working.
    Valid,
    /// Marked as not working.
    Invalid,
    /// Booted and never confirmed.
    Aborted,
    /// No state: the erased value.
    Undefined,
}

impl PublicState {
    /// `state`, or `None` for a value ESP-IDF doesn't define.
    #[must_use]
    pub const fn of(state: State) -> Option<Self> {
        match state {
            State::New => Some(Self::New),
            State::PendingVerify => Some(Self::PendingVerify),
            State::Valid => Some(Self::Valid),
            State::Invalid => Some(Self::Invalid),
            State::Aborted => Some(Self::Aborted),
            State::Undefined => Some(Self::Undefined),
            State::Unknown(_) => None,
        }
    }
}

/// The fields of one otadata entry that may leave the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryFields {
    validity: Validity,
    seq: Option<u32>,
    slot: Option<Slot>,
    state: Option<PublicState>,
}

impl EntryFields {
    /// The fields of `entry`: its validity, and the rest only when its CRC
    /// matches.
    const fn of(entry: &SelectEntry) -> Self {
        let validity = entry.validity();
        if crc_matches(validity) {
            Self {
                validity,
                seq: Some(entry.seq),
                slot: Some(entry.slot()),
                state: PublicState::of(entry.state),
            }
        } else {
            Self {
                validity,
                seq: None,
                slot: None,
                state: None,
            }
        }
    }

    /// How the bootloader judges the entry.
    #[must_use]
    pub const fn validity(&self) -> Validity {
        self.validity
    }

    /// The sequence number, when the CRC matches.
    #[must_use]
    pub const fn seq(&self) -> Option<u32> {
        self.seq
    }

    /// The slot the sequence number selects, when the CRC matches.
    #[must_use]
    pub const fn slot(&self) -> Option<Slot> {
        self.slot
    }

    /// The image state, when the CRC matches and ESP-IDF defines the value.
    #[must_use]
    pub const fn state(&self) -> Option<PublicState> {
        self.state
    }
}

/// The boot state of the running image that may leave the device, as
/// [`BootState`] gives it, its state only when ESP-IDF defines the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootFields {
    running: Slot,
    selected: Option<Slot>,
    state: Option<PublicState>,
    rollback_target: bool,
    needs_confirmation: bool,
}

impl BootFields {
    /// The fields of `boot`.
    const fn of(boot: &BootState) -> Self {
        Self {
            running: boot.running,
            selected: boot.selected,
            state: match boot.state {
                Some(state) => PublicState::of(state),
                None => None,
            },
            rollback_target: boot.rollback_target,
            needs_confirmation: boot.needs_confirmation,
        }
    }

    /// The slot that booted.
    #[must_use]
    pub const fn running(&self) -> Slot {
        self.running
    }

    /// The slot the active entry selects ([`BootState::selected`]).
    #[must_use]
    pub const fn selected(&self) -> Option<Slot> {
        self.selected
    }

    /// The running image's state ([`BootState::state`]), when ESP-IDF
    /// defines the value.
    #[must_use]
    pub const fn state(&self) -> Option<PublicState> {
        self.state
    }

    /// Whether the other slot is a rollback target
    /// ([`BootState::rollback_target`]).
    #[must_use]
    pub const fn rollback_target(&self) -> bool {
        self.rollback_target
    }

    /// Whether the running image needs a confirmation
    /// ([`BootState::needs_confirmation`]).
    #[must_use]
    pub const fn needs_confirmation(&self) -> bool {
        self.needs_confirmation
    }
}

/// Whether the running image awaits a confirmation, `boot` being its boot
/// state: every image but one confirmed `Valid`
/// ([`BootState::needs_confirmation`]). With no boot state, as when there is
/// no otadata, it doesn't read or the running slot is unknown, the image
/// awaits one.
#[must_use]
pub const fn awaits_confirmation(boot: Option<&BootState>) -> bool {
    match boot {
        Some(boot) => boot.needs_confirmation,
        None => true,
    }
}

/// The fields of the otadata that may leave the device: no byte, no label,
/// no unknown state value, no epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtadataFields {
    entries: [Result<EntryFields, ReadError>; 2],
    active: Option<Sector>,
    boot: Option<BootFields>,
}

impl OtadataFields {
    /// The fields of the entry of `sector`.
    ///
    /// # Errors
    ///
    /// Why the entry couldn't be read.
    pub const fn entry(&self, sector: Sector) -> Result<EntryFields, ReadError> {
        match sector {
            Sector::First => self.entries[0],
            Sector::Second => self.entries[1],
        }
    }

    /// The sector of the active entry, the valid one with the higher
    /// sequence number.
    #[must_use]
    pub const fn active(&self) -> Option<Sector> {
        self.active
    }

    /// The boot state of the running image, when the slot it runs from was
    /// given.
    #[must_use]
    pub const fn boot(&self) -> Option<BootFields> {
        self.boot
    }
}

/// The otadata as read, as of one epoch: both entries with every field,
/// labels included. It stays inside the device; [`fields`](Self::fields)
/// gives the fields that may leave it, and [`Otadata::published`] the
/// entries whose bytes may.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtadataModel {
    epoch: Epoch,
    otadata: format::Otadata,
}

impl OtadataModel {
    /// The region's epoch the model was parsed at.
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Both entries, each as read or the reason its read failed.
    #[must_use]
    pub const fn otadata(&self) -> &format::Otadata {
        &self.otadata
    }

    /// The sector of the active entry ([`format::Otadata::active`]).
    #[must_use]
    pub fn active(&self) -> Option<Sector> {
        self.otadata.active()
    }

    /// The slot the active entry selects.
    #[must_use]
    pub fn selected(&self) -> Option<Slot> {
        self.otadata.selected()
    }

    /// The boot state of the image running from `running`.
    #[must_use]
    pub fn boot_state(&self, running: Slot) -> BootState {
        self.otadata.boot_state(running)
    }

    /// The fields that may leave the device, with the boot state of the
    /// image running from `running` when it is given.
    #[must_use]
    pub fn fields(&self, running: Option<Slot>) -> OtadataFields {
        let entry = |sector: Sector| {
            self.otadata
                .entry(sector)
                .map(|entry| EntryFields::of(&entry))
        };
        OtadataFields {
            entries: [entry(Sector::First), entry(Sector::Second)],
            active: self.otadata.active(),
            boot: running.map(|running| BootFields::of(&self.otadata.boot_state(running))),
        }
    }
}

/// The reader of the otadata partition.
#[derive(Debug)]
enum OtadataReader<'f> {
    /// The compiled layout's.
    Compiled(Reader<'f, roles::Otadata>),
    /// The located table's, in the unverified tier.
    Found(Reader<'f, FoundOtadata>),
}

/// The otadata's content module.
#[derive(Debug)]
pub struct Otadata<'f> {
    reader: OtadataReader<'f>,
}

impl<'f> Otadata<'f> {
    /// The module of the compiled layout's otadata, in the layout and boot
    /// tiers.
    #[must_use]
    pub const fn compiled(reader: Reader<'f, roles::Otadata>) -> Self {
        Self {
            reader: OtadataReader::Compiled(reader),
        }
    }

    /// The module of the otadata the located table gives, in the unverified
    /// tier.
    #[must_use]
    pub const fn found(reader: Reader<'f, FoundOtadata>) -> Self {
        Self {
            reader: OtadataReader::Found(reader),
        }
    }

    /// Whether the otadata is the one a located table gives rather than the
    /// compiled layout's.
    #[must_use]
    pub const fn is_found(&self) -> bool {
        matches!(self.reader, OtadataReader::Found(_))
    }

    /// The region's span, from the start of the flash.
    #[must_use]
    pub const fn span(&self) -> Span {
        match &self.reader {
            OtadataReader::Compiled(reader) => reader.span(),
            OtadataReader::Found(reader) => reader.span(),
        }
    }

    /// Reads both entries through one snapshot.
    ///
    /// # Errors
    ///
    /// [`LoadError::Busy`] while a write session is open on the region, and
    /// [`LoadError::Stale`] when one wrote it while it was read.
    pub fn parse(&self) -> Result<OtadataModel, LoadError> {
        match &self.reader {
            OtadataReader::Compiled(reader) => parse(reader),
            OtadataReader::Found(reader) => parse(reader),
        }
    }

    /// The view of the entry of `sector` that `model` describes: its 32
    /// bytes, the label served as `0xFF`, when the parser pins down every
    /// byte of it, as the module's documentation describes.
    ///
    /// # Errors
    ///
    /// [`Unpublished::Unverified`] for the otadata a located table gives, and
    /// for an entry that couldn't be read or isn't pinned down,
    /// [`Unpublished::Stale`] once the region isn't stable at the model's
    /// epoch, and [`Unpublished::Refused`] for the storage crate's other
    /// refusals.
    pub fn published(
        &self,
        model: &OtadataModel,
        sector: Sector,
    ) -> Result<Published<'_, roles::Otadata>, Unpublished> {
        let OtadataReader::Compiled(reader) = &self.reader else {
            return Err(Unpublished::Unverified);
        };
        let entry = model
            .otadata
            .entry(sector)
            .map_err(|_| Unpublished::Unverified)?;
        if !pinned(&entry) {
            return Err(Unpublished::Unverified);
        }
        let span = Span::new(sector.offset(), ENTRY_LEN);
        publish(reader, model.epoch, span, &[LABEL]).map_err(Unpublished::from)
    }
}

/// Reads both entries of the otadata `reader` reads, through one snapshot.
fn parse<R: Role<Device = Internal>>(reader: &Reader<'_, R>) -> Result<OtadataModel, LoadError> {
    let snapshot = whole(reader)?;
    let otadata = format::read(&snapshot);
    if otadata.entries.contains(&Err(ReadError::Stale)) {
        return Err(LoadError::Stale);
    }
    Ok(OtadataModel {
        epoch: snapshot.epoch(),
        otadata,
    })
}

#[cfg(test)]
mod tests;
