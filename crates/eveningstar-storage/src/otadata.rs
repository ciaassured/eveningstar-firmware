//! The otadata partition: ESP-IDF's two copies of the boot selection, read
//! the way the ESP-IDF bootloader reads them, and the writes that change them
//! planned so that a power cut at any point leaves a bootable selection.
//!
//! The partition is two flash sectors, each holding one `esp_ota_select_entry_t`
//! in its first 32 bytes: the sequence number (`u32`), a 20-byte label the
//! bootloader ignores, the image state (`u32`) and a CRC (`u32`), all
//! little-endian. The CRC covers the sequence number only, not the state.
//!
//! The rules are those of ESP-IDF 5.5 built with `BOOTLOADER_APP_ROLLBACK_ENABLE`
//! and without anti-rollback or a factory app:
//!
//! - An entry is valid when its sequence number isn't `0xFFFFFFFF`, its CRC
//!   matches and its state is neither [`State::Invalid`] nor
//!   [`State::Aborted`]. Every other state, unknown values included, is valid.
//! - The active entry is the valid one with the higher sequence number; with
//!   two valid entries of equal sequence number, the first sector's.
//! - The active entry's sequence number `s` selects app slot `(s - 1) % 2`,
//!   computed in wrapping `u32` arithmetic, so 0 selects `ota_1`.
//! - At every boot the bootloader turns each [`State::PendingVerify`] entry
//!   into [`State::Aborted`], then turns an active [`State::New`] entry into
//!   [`State::PendingVerify`], each by erasing the entry's sector and writing
//!   it back.
//! - With no valid entry, it tries `ota_0` first, then `ota_1`.
//!
//! Each entry is read and judged on its own: a sector that can't be read or
//! holds garbage never hides the other entry, and no plan needs to read the
//! sector it overwrites.
//!
//! Plans are descriptions of one write: erase one sector of the partition, then
//! program one whole entry at its start. Nothing in this module touches the
//! flash.
//!
//! A power cut can leave the written sector erased, or with any prefix of the
//! entry programmed and the rest erased. Programming writes the state before
//! the CRC, and only sequence number 0 has a CRC of `0xFFFFFFFF`, the erased
//! value. So a partly programmed entry with a sequence number other than 0 is
//! never valid, and no plan writes a new entry with sequence number 0.

use crate::source::{Source, read_array};
use crate::{ReadError, Slot};

/// Size of one sector of the partition, which holds one entry.
pub const SECTOR_SIZE: u32 = 4096;

/// Length of one entry in bytes.
pub const ENTRY_LEN: usize = 32;

/// The sequence number of an erased entry, which is never valid.
pub const BLANK_SEQ: u32 = u32::MAX;

/// The CRC-32 polynomial, reflected.
const POLY: u32 = 0xEDB8_8320;

const _: () = assert!(crate::layout::OTADATA.size == 2 * SECTOR_SIZE);

/// The CRC an entry with sequence number `seq` stores: ESP-IDF's
/// `esp_rom_crc32_le(UINT32_MAX, &seq, 4)`, a reflected CRC-32 with the
/// register starting at 0 and the result inverted, over the four
/// little-endian bytes of `seq`.
#[must_use]
pub const fn crc(seq: u32) -> u32 {
    // Feeding the four little-endian bytes one at a time, low bit first, is
    // the same as feeding the word at once.
    let mut crc = seq;
    let mut bit = 0;
    while bit < 32 {
        crc = if crc & 1 == 0 {
            crc >> 1
        } else {
            (crc >> 1) ^ POLY
        };
        bit += 1;
    }
    !crc
}

/// The image state an entry stores (`esp_ota_img_states_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Selected, not yet booted (`0`). The bootloader turns it into
    /// [`PendingVerify`](Self::PendingVerify) when it boots the image.
    New,
    /// Booted once, not yet confirmed (`1`). The bootloader turns it into
    /// [`Aborted`](Self::Aborted) at the next boot.
    PendingVerify,
    /// Confirmed as working (`2`).
    Valid,
    /// Marked as not working (`3`). The entry is never valid.
    Invalid,
    /// Booted and never confirmed (`4`). The entry is never valid.
    Aborted,
    /// No state (`0xFFFFFFFF`, the erased value). The image boots without
    /// rollback.
    Undefined,
    /// Any other value, which ESP-IDF treats like a valid state.
    Unknown(u32),
}

impl State {
    /// The state stored as `raw`.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::New,
            1 => Self::PendingVerify,
            2 => Self::Valid,
            3 => Self::Invalid,
            4 => Self::Aborted,
            u32::MAX => Self::Undefined,
            other => Self::Unknown(other),
        }
    }

    /// The value stored for the state.
    #[must_use]
    pub const fn raw(self) -> u32 {
        match self {
            Self::New => 0,
            Self::PendingVerify => 1,
            Self::Valid => 2,
            Self::Invalid => 3,
            Self::Aborted => 4,
            Self::Undefined => u32::MAX,
            Self::Unknown(raw) => raw,
        }
    }
}

/// How an entry is judged, checked in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity {
    /// The entry can select a slot.
    Valid,
    /// The sequence number is [`BLANK_SEQ`], as in an erased sector.
    Blank,
    /// The CRC doesn't match the sequence number.
    Corrupt,
    /// The state is [`State::Invalid`].
    Invalid,
    /// The state is [`State::Aborted`].
    Aborted,
}

/// One copy of the boot selection, `esp_ota_select_entry_t`, with every field
/// as stored, whether or not it is consistent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectEntry {
    /// The sequence number, which decides the active entry and the slot.
    pub seq: u32,
    /// The label, which the bootloader ignores.
    pub label: [u8; 20],
    /// The image state.
    pub state: State,
    /// The stored CRC of the sequence number.
    pub crc: u32,
}

impl SelectEntry {
    /// The entry the plans write: `seq` and `state`, an erased label and the
    /// matching CRC.
    #[must_use]
    pub const fn new(seq: u32, state: State) -> Self {
        Self {
            seq,
            label: [0xFF; 20],
            state,
            crc: crc(seq),
        }
    }

    /// The entry stored in `bytes`. Every 32 bytes are an entry; whether it is
    /// valid is [`validity`](Self::validity).
    #[must_use]
    pub fn from_bytes(bytes: &[u8; ENTRY_LEN]) -> Self {
        let [q0, q1, q2, q3, label @ .., s0, s1, s2, s3, c0, c1, c2, c3] = *bytes;
        Self {
            seq: u32::from_le_bytes([q0, q1, q2, q3]),
            label,
            state: State::from_raw(u32::from_le_bytes([s0, s1, s2, s3])),
            crc: u32::from_le_bytes([c0, c1, c2, c3]),
        }
    }

    /// The 32 bytes that store the entry.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ENTRY_LEN] {
        let mut bytes = [0; ENTRY_LEN];
        let (seq, rest) = bytes.split_at_mut(4);
        let (label, rest) = rest.split_at_mut(20);
        let (state, crc) = rest.split_at_mut(4);
        seq.copy_from_slice(&self.seq.to_le_bytes());
        label.copy_from_slice(&self.label);
        state.copy_from_slice(&self.state.raw().to_le_bytes());
        crc.copy_from_slice(&self.crc.to_le_bytes());
        bytes
    }

    /// How the bootloader judges the entry.
    #[must_use]
    pub const fn validity(&self) -> Validity {
        if self.seq == BLANK_SEQ {
            Validity::Blank
        } else if self.crc != crc(self.seq) {
            Validity::Corrupt
        } else {
            match self.state {
                State::Invalid => Validity::Invalid,
                State::Aborted => Validity::Aborted,
                _ => Validity::Valid,
            }
        }
    }

    /// Whether the entry can select a slot.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self.validity(), Validity::Valid)
    }

    /// The slot the sequence number selects, whether or not the entry is
    /// valid.
    #[must_use]
    pub const fn slot(&self) -> Slot {
        slot_of(self.seq)
    }

    /// The same entry with `state`, as an in-place rewrite stores it: the
    /// sequence number and label kept, the CRC recomputed.
    const fn with_state(&self, state: State) -> Self {
        Self {
            seq: self.seq,
            label: self.label,
            state,
            crc: crc(self.seq),
        }
    }
}

/// The slot sequence number `seq` selects.
const fn slot_of(seq: u32) -> Slot {
    if seq.wrapping_sub(1).is_multiple_of(2) {
        Slot::Ota0
    } else {
        Slot::Ota1
    }
}

/// The smallest sequence number above `after` that selects `slot`, `None`
/// when that would reach [`BLANK_SEQ`] or overflow.
const fn seq_after(after: u32, slot: Slot) -> Option<u32> {
    let step = if slot_of(after).number() == slot.number() {
        2
    } else {
        1
    };
    match after.checked_add(step) {
        Some(seq) if seq != BLANK_SEQ => Some(seq),
        _ => None,
    }
}

/// One of the partition's two sectors, `otadata[0]` and `otadata[1]` in
/// ESP-IDF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sector {
    /// The first sector, at the start of the partition.
    First,
    /// The second sector, [`SECTOR_SIZE`] bytes in.
    Second,
}

impl Sector {
    /// Both sectors, in order.
    pub const ALL: [Self; 2] = [Self::First, Self::Second];

    /// The position of the sector: 0 or 1.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::First => 0,
            Self::Second => 1,
        }
    }

    /// The offset of the sector, and of its entry, from the start of the
    /// partition.
    #[must_use]
    pub const fn offset(self) -> u32 {
        match self {
            Self::First => 0,
            Self::Second => SECTOR_SIZE,
        }
    }

    /// The other sector.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

/// The boot state the otadata gives for the running image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootState {
    /// The slot that booted.
    pub running: Slot,
    /// The slot the active entry selects, or `None` when no entry is valid. In
    /// that case the bootloader tries `ota_0` first, then `ota_1`.
    pub selected: Option<Slot>,
    /// The running image's state: the active entry's when it selects the
    /// running slot. Otherwise, the state of the other entry when that one
    /// isn't blank, its CRC matches and it selects the running slot, which is
    /// the case after the bootloader fell back from the selected slot or while
    /// an activation hasn't booted yet; this state may be
    /// [`Invalid`](State::Invalid) or [`Aborted`](State::Aborted). `None` when
    /// no such entry exists.
    pub state: Option<State>,
    /// Whether the other slot is a rollback target: the active entry selects
    /// the running slot, and the other entry is valid and selects the other
    /// slot. Marking the running image [`Invalid`](State::Invalid) then makes
    /// the bootloader select the other slot.
    pub rollback_target: bool,
}

/// Why a write can't be planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Every sequence number above the active entry's that selects the slot
    /// is [`BLANK_SEQ`] or past it.
    SeqExhausted,
    /// The active entry is [`State::PendingVerify`]: the image it selects
    /// hasn't been confirmed.
    PendingVerify,
    /// The active entry doesn't select the running slot, or there is none.
    NotSelected,
    /// The other slot isn't a rollback target.
    NoRollbackTarget,
    /// The sector the write leaves alone, which decides the selection if the
    /// write is cut short, couldn't be read.
    Read {
        /// The sector.
        sector: Sector,
        /// Why reading it failed.
        error: ReadError,
    },
}

/// A planned write: erase `sector`, then program `entry` at its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    sector: Sector,
    entry: SelectEntry,
}

impl Write {
    /// The sector to erase and program.
    #[must_use]
    pub const fn sector(&self) -> Sector {
        self.sector
    }

    /// The entry to program.
    #[must_use]
    pub const fn entry(&self) -> &SelectEntry {
        &self.entry
    }

    /// The offset of the sector, and of the bytes to program, from the start
    /// of the partition.
    #[must_use]
    pub const fn offset(&self) -> u32 {
        self.sector.offset()
    }

    /// The bytes to program, in one go.
    #[must_use]
    pub fn bytes(&self) -> [u8; ENTRY_LEN] {
        self.entry.to_bytes()
    }
}

/// Both entries of the partition, each as read or the reason its read failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Otadata {
    /// The entries of the first and second sectors.
    pub entries: [Result<SelectEntry, ReadError>; 2],
}

/// Reads both entries of the otadata partition `otadata`, a source that
/// starts at the start of the partition. A read that fails is kept as that
/// entry's error, and the other entry is still read.
pub fn read<S: Source + ?Sized>(otadata: &S) -> Otadata {
    let entry = |sector: Sector| {
        read_array(otadata, sector.offset()).map(|bytes| SelectEntry::from_bytes(&bytes))
    };
    Otadata {
        entries: [entry(Sector::First), entry(Sector::Second)],
    }
}

impl Otadata {
    /// The entry of `sector`, or why it couldn't be read.
    ///
    /// # Errors
    ///
    /// The error reading the sector returned.
    pub const fn entry(&self, sector: Sector) -> Result<SelectEntry, ReadError> {
        match sector {
            Sector::First => self.entries[0],
            Sector::Second => self.entries[1],
        }
    }

    /// The entry of `sector` if it was read and is valid.
    #[must_use]
    pub fn valid(&self, sector: Sector) -> Option<SelectEntry> {
        self.entry(sector).ok().filter(SelectEntry::is_valid)
    }

    /// The sector of the active entry: of two valid entries, the one with the
    /// higher sequence number or, when they are equal, the first; else the
    /// valid one; else `None`.
    #[must_use]
    pub fn active(&self) -> Option<Sector> {
        match (self.valid(Sector::First), self.valid(Sector::Second)) {
            (Some(first), Some(second)) if second.seq > first.seq => Some(Sector::Second),
            (Some(_), _) => Some(Sector::First),
            (None, Some(_)) => Some(Sector::Second),
            (None, None) => None,
        }
    }

    /// The active entry.
    #[must_use]
    pub fn active_entry(&self) -> Option<SelectEntry> {
        self.active().and_then(|sector| self.valid(sector))
    }

    /// The slot the active entry selects.
    #[must_use]
    pub fn selected(&self) -> Option<Slot> {
        self.active_entry().map(|entry| entry.slot())
    }

    /// The boot state for the image running from `running`.
    #[must_use]
    pub fn boot_state(&self, running: Slot) -> BootState {
        let active = self.active();
        let active_entry = self.active_entry();
        let selected = active_entry.map(|entry| entry.slot());
        let state = match active_entry {
            Some(entry) if entry.slot() == running => Some(entry.state),
            _ => self.fallback_state(active, running),
        };
        let rollback_target = selected == Some(running)
            && active
                .and_then(|sector| self.valid(sector.other()))
                .is_some_and(|other| other.slot() == running.other());
        BootState {
            running,
            selected,
            state,
            rollback_target,
        }
    }

    /// The state of the highest entry other than the active one that isn't
    /// blank, has a matching CRC and selects `running`.
    fn fallback_state(&self, active: Option<Sector>, running: Slot) -> Option<State> {
        let candidate = |sector: Sector| {
            self.entry(sector).ok().filter(|entry| {
                Some(sector) != active
                    && !matches!(entry.validity(), Validity::Blank | Validity::Corrupt)
                    && entry.slot() == running
            })
        };
        match (candidate(Sector::First), candidate(Sector::Second)) {
            (Some(first), Some(second)) if second.seq > first.seq => Some(second.state),
            (Some(entry), _) | (None, Some(entry)) => Some(entry.state),
            (None, None) => None,
        }
    }

    /// The entry of the sector a plan leaves alone, refusing the plan when it
    /// couldn't be read.
    fn kept(&self, sector: Sector) -> Result<SelectEntry, PlanError> {
        self.entry(sector)
            .map_err(|error| PlanError::Read { sector, error })
    }

    /// Plans the next write that takes the other slot out of the boot
    /// selection before it is rewritten; `None` once both entries are valid
    /// and select `running`, the slot that booted. Making each write planned
    /// and planning again reaches `None` after at most two writes.
    ///
    /// When the active entry selects the other slot and the other entry
    /// selects `running`, the active entry is rewritten in place with state
    /// [`State::Invalid`], which cancels an activation not yet booted. A
    /// power cut leaves it erased or partial, so the entry for `running` is
    /// selected, as after the write.
    ///
    /// Otherwise the write is an entry selecting `running` with state
    /// [`State::Valid`], in the sector that isn't active (the first when no
    /// entry is valid), with the smallest sequence number above the active
    /// entry's that selects `running`, or `running`'s number plus 1 when no
    /// entry is valid. When the active entry already selects `running`, that
    /// is its sequence number plus 2, where ESP-IDF's `esp_rewrite_ota_data`
    /// would write an equal one, which loses the tie from the second sector.
    /// It replaces what that sector held: an entry for the other slot, which
    /// is about to stop holding the image it named, a second entry for
    /// `running`, or nothing valid. A power cut leaves the
    /// sector erased or partial, which isn't valid, so the active entry is the
    /// one before the write and the bootloader does what it would have done
    /// without it. No write replaces the only valid entry for `running` while
    /// the other slot's entry is the active one, as a cut would then leave
    /// nothing to roll back to.
    ///
    /// Once settled, no valid entry names the other slot, so neither the
    /// bootloader nor [`plan_roll_back`](Self::plan_roll_back) can select it
    /// while it is rewritten, and a cut in-place rewrite of either entry
    /// leaves the other one selecting `running`. With no valid entry at all,
    /// the bootloader would try `ota_0` first, whichever slot is running.
    ///
    /// # Errors
    ///
    /// [`PlanError::PendingVerify`] when the active entry selects `running`
    /// and hasn't been confirmed, as ESP-IDF's `esp_ota_begin` refuses then
    /// too, [`PlanError::SeqExhausted`] when the active entry's sequence
    /// number is too high, and [`PlanError::Read`] when no entry is valid and
    /// the second sector, which then stays, couldn't be read.
    pub fn plan_begin_update(&self, running: Slot) -> Result<Option<Write>, PlanError> {
        let (Some(sector), Some(active)) = (self.active(), self.active_entry()) else {
            return self.plan_select(running, State::Valid).map(Some);
        };
        let other_runs = self
            .valid(sector.other())
            .is_some_and(|other| other.slot() == running);
        if active.slot() == running {
            if active.state == State::PendingVerify {
                return Err(PlanError::PendingVerify);
            }
            if other_runs {
                return Ok(None);
            }
        } else if other_runs {
            return Ok(Some(Write {
                sector,
                entry: active.with_state(State::Invalid),
            }));
        }
        self.plan_select(running, State::Valid).map(Some)
    }

    /// Plans the activation of the slot that didn't boot: an entry selecting
    /// it with state [`State::New`], so the bootloader boots it once as
    /// [`State::PendingVerify`] and rolls back unless it is confirmed.
    ///
    /// The active entry has to select `running`. The new entry goes to the
    /// sector that isn't active, with the active entry's sequence number plus
    /// 1, and replaces the entry there, which isn't needed while the active
    /// one stays. State, sequence number and CRC are written together, so the
    /// entry is never valid with a state other than `New`, and the bootloader
    /// never boots the activated image without being able to roll it back.
    ///
    /// A power cut leaves the sector erased or holding a partial entry, which
    /// isn't valid, so the active entry still selects `running`.
    ///
    /// # Errors
    ///
    /// [`PlanError::NotSelected`] when the active entry doesn't select
    /// `running` or no entry is valid, [`PlanError::PendingVerify`] when the
    /// running image hasn't been confirmed, as ESP-IDF's `esp_ota_begin`
    /// refuses then too, and [`PlanError::SeqExhausted`] when the active
    /// entry's sequence number is too high.
    pub fn plan_activate(&self, running: Slot) -> Result<Write, PlanError> {
        let active = self.active_entry().ok_or(PlanError::NotSelected)?;
        if active.slot() != running {
            return Err(PlanError::NotSelected);
        }
        if active.state == State::PendingVerify {
            return Err(PlanError::PendingVerify);
        }
        self.plan_select(running.other(), State::New)
    }

    /// Plans marking the running image [`State::Valid`], which cancels its
    /// rollback; `None` when it already is.
    ///
    /// The active entry has to select `running`. It is rewritten in place,
    /// as ESP-IDF does, with its sequence number and label kept: the other
    /// entry, the rollback target, stays.
    ///
    /// A power cut leaves the active sector erased or holding a partial entry,
    /// which isn't valid unless its sequence number is 0. The other entry then
    /// decides: for a [`State::PendingVerify`] image, as it would at the next
    /// boot without the write, since the bootloader aborts the image there;
    /// otherwise it rolls back. An entry with sequence number 0 may stay valid
    /// with a partly written state, which is [`State::Unknown`] and boots
    /// `running` without rollback.
    ///
    /// # Errors
    ///
    /// [`PlanError::NotSelected`] when the active entry doesn't select
    /// `running` or no entry is valid, and [`PlanError::Read`] when the other
    /// sector couldn't be read.
    pub fn plan_mark_valid(&self, running: Slot) -> Result<Option<Write>, PlanError> {
        let (sector, active) = self.running_entry(running)?;
        if active.state == State::Valid {
            return Ok(None);
        }
        Ok(Some(Write {
            sector,
            entry: active.with_state(State::Valid),
        }))
    }

    /// Plans marking the running image [`State::Invalid`], so the bootloader
    /// selects the other slot at the next boot.
    ///
    /// The other slot has to be a rollback target (see
    /// [`BootState::rollback_target`]); whether its image verifies isn't
    /// checked here. The active entry is rewritten in place, as ESP-IDF
    /// does.
    ///
    /// A power cut leaves the active sector erased or holding a partial entry,
    /// which isn't valid unless its sequence number is 0, so the rollback
    /// target is selected as it would be after the write. An entry with
    /// sequence number 0 may stay valid with a partly written state, which
    /// is [`State::Unknown`], so `running` boots again.
    ///
    /// # Errors
    ///
    /// [`PlanError::NotSelected`] when the active entry doesn't select
    /// `running` or no entry is valid, [`PlanError::Read`] when the other
    /// sector couldn't be read, and [`PlanError::NoRollbackTarget`] when the
    /// other slot isn't a rollback target.
    pub fn plan_roll_back(&self, running: Slot) -> Result<Write, PlanError> {
        let (sector, active) = self.running_entry(running)?;
        if !self.boot_state(running).rollback_target {
            return Err(PlanError::NoRollbackTarget);
        }
        Ok(Write {
            sector,
            entry: active.with_state(State::Invalid),
        })
    }

    /// The active entry and its sector when it selects `running`, and the
    /// other sector could be read.
    fn running_entry(&self, running: Slot) -> Result<(Sector, SelectEntry), PlanError> {
        let sector = self.active().ok_or(PlanError::NotSelected)?;
        let active = self.valid(sector).ok_or(PlanError::NotSelected)?;
        if active.slot() != running {
            return Err(PlanError::NotSelected);
        }
        self.kept(sector.other())?;
        Ok((sector, active))
    }

    /// Plans an entry selecting `slot` with `state`, in the sector that isn't
    /// active (the first when no entry is valid), with the smallest sequence
    /// number above the active entry's that selects `slot`, or `slot`'s number
    /// plus 1 when no entry is valid.
    ///
    /// ESP-IDF's `esp_rewrite_ota_data` picks the smallest such number at or
    /// above the active one's, so when `slot` is already selected it writes an
    /// equal number to the other sector, which loses the tie when that is the
    /// second. This picks a strictly greater one, so the new entry is always
    /// the active one once written.
    fn plan_select(&self, slot: Slot, state: State) -> Result<Write, PlanError> {
        let (sector, seq) =
            if let (Some(sector), Some(active)) = (self.active(), self.active_entry()) {
                let seq = seq_after(active.seq, slot).ok_or(PlanError::SeqExhausted)?;
                (sector.other(), seq)
            } else {
                self.kept(Sector::Second)?;
                (Sector::First, slot.number() + 1)
            };
        Ok(Write {
            sector,
            entry: SelectEntry::new(seq, state),
        })
    }
}

#[cfg(test)]
mod tests;
