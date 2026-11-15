//! The image information block: the facts an update is judged by, which an
//! app image carries right after its layout record.
//!
//! The block is a [`Container`] of the shared record encoding
//! (`eveningstar-records`), with the magic [`MAGIC`] and at most [`MAX_LEN`]
//! bytes. Its types ([`TYPES`]):
//!
//! - **Board** (1): the board's name, up to 16 bytes, and the revisions the
//!   image supports, as a set of bits.
//! - **Compatibility** (2): the oldest configuration schema the image
//!   migrates from, and its own.
//! - **Trust** (3): the generation the image is signed with, and the keys
//!   its trust set holds, each with its generation, keynum and public key,
//!   in slots of which the first `count` are used. A generation is 0 for the
//!   development key. [`encode`] writes [`MAX_KEYS`] slots; a later image
//!   may append more, and a reader reads every slot `count` names that the
//!   value holds.
//!
//! A reader requires all three, skips types it doesn't know, and reads
//! fields a value ends before as absent. [`encode`] is a `const fn`, so a
//! firmware places its block in a `static`; [`read`] finds the block of an
//! app image, and [`Block::parse`] reads one in place.
//!
//! The committed `registry.txt` is the registry of the last release, or of
//! the current types before the first release; a test checks the current
//! types against it with [`registry::check`].
//!
//! [`Container`]: eveningstar_records::Container
//! [`registry::check`]: eveningstar_records::registry::check

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

use eveningstar_esp_formats::Source;
use eveningstar_esp_formats::image::{self, ImageError};
use eveningstar_records::registry::{Field, Type};
use eveningstar_records::{Container, ENTRY_HEADER_LEN, HEADER_LEN, TooShort, Value};
use eveningstar_signify::{Generation, Keynum, TrustSet, trust};

/// The block's magic, `ESII` in its little-endian bytes.
pub const MAGIC: u32 = 0x4949_5345;

/// The longest block a reader accepts, in bytes.
pub const MAX_LEN: usize = 1024;

/// The key slots [`encode`] writes, and so the most keys a [`Trust`] holds:
/// the three release generations a trust set holds, and the development key
/// a development build trusts beside them. A block may hold more slots,
/// appended after these.
pub const MAX_KEYS: usize = 4;

/// The longest board name, in bytes.
pub const NAME_LEN: usize = 16;

/// The board entry's type.
pub const BOARD: u16 = 1;
/// The compatibility entry's type.
pub const COMPATIBILITY: u16 = 2;
/// The trust entry's type.
pub const TRUST: u16 = 3;

/// Length of the board value: the name, then the revisions.
const BOARD_LEN: usize = NAME_LEN + 4;
/// Length of the compatibility value: the oldest schema, then the image's.
const COMPATIBILITY_LEN: usize = 8;
/// Length of one key slot: generation, keynum, public key.
const KEY_LEN: usize = 4 + 8 + 32;
/// Length of the trust value: signing generation, count, then the slots.
const TRUST_LEN: usize = 8 + MAX_KEYS * KEY_LEN;

/// Length of the block [`encode`] writes.
pub const LEN: usize =
    HEADER_LEN + 3 * ENTRY_HEADER_LEN + BOARD_LEN + COMPATIBILITY_LEN + TRUST_LEN;

const _: () = assert!(LEN <= MAX_LEN);
const _: () = assert!(
    BOARD_LEN.is_multiple_of(4)
        && COMPATIBILITY_LEN.is_multiple_of(4)
        && TRUST_LEN.is_multiple_of(4)
);

/// `n` as a `u16`, for the constant lengths and offsets of the block, which
/// fail the build if one doesn't fit.
#[expect(
    clippy::cast_possible_truncation,
    reason = "checked against u16::MAX first"
)]
const fn short(n: usize) -> u16 {
    assert!(n <= u16::MAX as usize);
    n as u16
}

/// The fields of one key slot, at the offset of slot `slot`.
macro_rules! key_fields {
    ($slot:literal, $generation:literal, $keynum:literal, $public_key:literal) => {
        [
            Field {
                name: $generation,
                offset: short(8 + $slot * KEY_LEN),
                len: 4,
            },
            Field {
                name: $keynum,
                offset: short(8 + $slot * KEY_LEN + 4),
                len: 8,
            },
            Field {
                name: $public_key,
                offset: short(8 + $slot * KEY_LEN + 12),
                len: 32,
            },
        ]
    };
}

/// The trust entry's fields.
const TRUST_FIELDS: [Field; 2 + 3 * MAX_KEYS] = {
    let slots = [
        key_fields!(0, "key_0_generation", "key_0_keynum", "key_0_public_key"),
        key_fields!(1, "key_1_generation", "key_1_keynum", "key_1_public_key"),
        key_fields!(2, "key_2_generation", "key_2_keynum", "key_2_public_key"),
        key_fields!(3, "key_3_generation", "key_3_keynum", "key_3_public_key"),
    ];
    let mut fields = [Field {
        name: "",
        offset: 0,
        len: 0,
    }; 2 + 3 * MAX_KEYS];
    fields[0] = Field {
        name: "signing",
        offset: 0,
        len: 4,
    };
    fields[1] = Field {
        name: "count",
        offset: 4,
        len: 4,
    };
    let mut i = 0;
    while i < MAX_KEYS * 3 {
        fields[2 + i] = slots[i / 3][i % 3];
        i += 1;
    }
    fields
};

/// The block's types, as the registry describes them.
pub const TYPES: &[Type] = &[
    Type {
        kind: BOARD,
        name: "board",
        min_len: short(BOARD_LEN),
        fields: &[
            Field {
                name: "name",
                offset: 0,
                len: short(NAME_LEN),
            },
            Field {
                name: "revisions",
                offset: short(NAME_LEN),
                len: 4,
            },
        ],
    },
    Type {
        kind: COMPATIBILITY,
        name: "compatibility",
        min_len: short(COMPATIBILITY_LEN),
        fields: &[
            Field {
                name: "oldest_schema",
                offset: 0,
                len: 4,
            },
            Field {
                name: "schema",
                offset: 4,
                len: 4,
            },
        ],
    },
    Type {
        kind: TRUST,
        name: "trust",
        min_len: short(TRUST_LEN),
        fields: &TRUST_FIELDS,
    },
];

/// The board an image is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// The name, zero-padded.
    name: [u8; NAME_LEN],
    /// Bit `n` set for each revision `n` the image supports.
    revisions: u32,
}

impl Board {
    /// The board `name`, 1 to [`NAME_LEN`] bytes of `[a-z0-9-]`, in the
    /// `revisions` given, each below 32. `None` for any other name or
    /// revision, or no revision.
    #[must_use]
    pub const fn new(name: &str, revisions: &[u8]) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > NAME_LEN || revisions.is_empty() {
            return None;
        }
        let mut padded = [0; NAME_LEN];
        let mut i = 0;
        while i < bytes.len() {
            if !matches!(bytes[i], b'a'..=b'z' | b'0'..=b'9' | b'-') {
                return None;
            }
            padded[i] = bytes[i];
            i += 1;
        }
        let mut set = 0_u32;
        let mut i = 0;
        while i < revisions.len() {
            if revisions[i] >= 32 {
                return None;
            }
            set |= 1 << revisions[i];
            i += 1;
        }
        Some(Self {
            name: padded,
            revisions: set,
        })
    }

    /// The name.
    #[must_use]
    pub fn name(&self) -> &str {
        let len = self
            .name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(NAME_LEN);
        // `new` and `parse` keep only ASCII names.
        self.name
            .get(..len)
            .and_then(|name| core::str::from_utf8(name).ok())
            .unwrap_or_default()
    }

    /// The revisions, bit `n` set for revision `n`.
    #[must_use]
    pub const fn revisions(&self) -> u32 {
        self.revisions
    }

    /// Whether the image supports `revision`.
    #[must_use]
    pub const fn supports(&self, revision: u8) -> bool {
        revision < 32 && self.revisions & (1 << revision) != 0
    }

    /// The board a value of the board type holds.
    #[inline(never)]
    fn parse(value: &Value<'_>) -> Result<Self, Error> {
        let name: [u8; NAME_LEN] = value.array(0).ok_or(Error::Board)?;
        let revisions = value.u32(NAME_LEN).ok_or(Error::Board)?;
        let len = name.iter().position(|byte| *byte == 0).unwrap_or(NAME_LEN);
        let (used, padding) = name.split_at(len);
        let valid = !used.is_empty()
            && used
                .iter()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
            && padding.iter().all(|byte| *byte == 0)
            && revisions != 0;
        if !valid {
            return Err(Error::Board);
        }
        Ok(Self { name, revisions })
    }
}

/// The configuration schemas an image works with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compatibility {
    oldest_schema: u32,
    schema: u32,
}

impl Compatibility {
    /// An image whose configuration schema is `schema`, which migrates a
    /// configuration from `oldest_schema` on. `None` when `oldest_schema`
    /// is past `schema`.
    #[must_use]
    pub const fn new(oldest_schema: u32, schema: u32) -> Option<Self> {
        if oldest_schema > schema {
            return None;
        }
        Some(Self {
            oldest_schema,
            schema,
        })
    }

    /// The oldest schema the image migrates a configuration from.
    #[must_use]
    pub const fn oldest_schema(&self) -> u32 {
        self.oldest_schema
    }

    /// The image's own schema.
    #[must_use]
    pub const fn schema(&self) -> u32 {
        self.schema
    }

    /// The compatibility a value of the compatibility type holds.
    #[inline(never)]
    fn parse(value: &Value<'_>) -> Result<Self, Error> {
        let oldest = value.u32(0).ok_or(Error::Compatibility)?;
        let schema = value.u32(4).ok_or(Error::Compatibility)?;
        Self::new(oldest, schema).ok_or(Error::Compatibility)
    }
}

/// A key an image trusts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    generation: Generation,
    keynum: Keynum,
    ed25519: [u8; 32],
}

impl Key {
    /// The key's generation, as the block states it. Whether a key is the
    /// development key is decided by its public key ([`is_development`]),
    /// not by this.
    ///
    /// [`is_development`]: Self::is_development
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// The key's keynum.
    #[must_use]
    pub const fn keynum(&self) -> &Keynum {
        &self.keynum
    }

    /// The key's Ed25519 public key.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; 32] {
        &self.ed25519
    }

    /// Whether the key is the development key: whether its public key is the
    /// development key's, whatever generation the block states.
    #[must_use]
    pub fn is_development(&self) -> bool {
        self.ed25519 == *trust::DEVELOPMENT_KEY.public_key()
    }
}

/// What an image trusts, and the generation it is signed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trust {
    signing: Generation,
    keys: [Key; MAX_KEYS],
    count: usize,
}

/// An unused key slot.
const NO_KEY: Key = Key {
    generation: Generation::Development,
    keynum: [0; 8],
    ed25519: [0; 32],
};

impl Trust {
    /// An image that trusts the keys of `set` and is signed with a key of
    /// generation `signing`. `None` for no key or more than [`MAX_KEYS`], or
    /// a release generation 0, which would read as the development key.
    #[must_use]
    pub const fn new(set: &TrustSet, signing: Generation) -> Option<Self> {
        let keys = set.keys();
        if keys.is_empty() || keys.len() > MAX_KEYS || generation_number(signing).is_none() {
            return None;
        }
        let mut slots = [NO_KEY; MAX_KEYS];
        let mut i = 0;
        while i < keys.len() {
            let key = &keys[i];
            if generation_number(key.generation()).is_none() {
                return None;
            }
            slots[i] = Key {
                generation: key.generation(),
                keynum: *key.keynum(),
                ed25519: *key.public_key(),
            };
            i += 1;
        }
        Some(Self {
            signing,
            keys: slots,
            count: keys.len(),
        })
    }

    /// The generation the image is signed with.
    #[must_use]
    pub const fn signing(&self) -> Generation {
        self.signing
    }

    /// The keys the image trusts.
    #[must_use]
    pub fn keys(&self) -> &[Key] {
        self.keys.get(..self.count).unwrap_or_default()
    }

    /// Whether the image trusts the development key.
    #[must_use]
    pub fn trusts_development_key(&self) -> bool {
        self.keys().iter().any(Key::is_development)
    }
}

/// A generation as the block stores it: 0 for the development key; `None`
/// for release generation 0, which has no number of its own.
const fn generation_number(generation: Generation) -> Option<u32> {
    match generation {
        Generation::Development => Some(0),
        Generation::Release(0) => None,
        Generation::Release(n) => Some(n as u32),
    }
}

/// The generation the block stores as `number`; `None` past `u16::MAX`.
fn generation_of(number: u32) -> Option<Generation> {
    match number {
        0 => Some(Generation::Development),
        n => u16::try_from(n).ok().map(Generation::Release),
    }
}

/// The facts an image carries in its block, as [`encode`] writes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageInfo {
    /// The board the image is built for.
    pub board: Board,
    /// The configuration schemas it works with.
    pub compatibility: Compatibility,
    /// What it trusts.
    pub trust: Trust,
}

/// Why a block doesn't read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The container doesn't read, or a required type is missing or appears
    /// twice.
    Records(eveningstar_records::Error),
    /// A value is shorter than its type's minimum.
    TooShort {
        /// The entry's type.
        kind: u16,
    },
    /// The block runs past [`MAX_LEN`] or past the image's first segment.
    TooLong,
    /// The board's name isn't 1 to 16 bytes of `[a-z0-9-]` zero-padded, or
    /// it names no revision.
    Board,
    /// The oldest schema is past the image's own.
    Compatibility,
    /// The signing generation or a key's generation is past `u16::MAX`, or
    /// the count is 0 or names more slots than the value holds.
    Trust,
}

impl From<eveningstar_records::Error> for Error {
    fn from(error: eveningstar_records::Error) -> Self {
        Self::Records(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Records(error) => write!(f, "image information: {error}"),
            Self::TooShort { kind } => write!(f, "image information type {kind} is too short"),
            Self::TooLong => f.write_str("image information runs past its room"),
            Self::Board => f.write_str("image information names no valid board"),
            Self::Compatibility => f.write_str("image information's oldest schema is past its own"),
            Self::Trust => f.write_str("image information's trust entry doesn't read"),
        }
    }
}

/// A block that parsed, read in place from its bytes: every field it holds
/// was checked, so the accessors decode them without failing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block<'b> {
    board: Board,
    compatibility: Compatibility,
    trust: Value<'b>,
    signing: Generation,
    count: usize,
}

impl<'b> Block<'b> {
    /// The block in `bytes`, which start with its header; bytes past its
    /// total length are ignored.
    ///
    /// # Errors
    ///
    /// The first [`Error`] found.
    pub fn parse(bytes: &'b [u8]) -> Result<Self, Error> {
        let container = Container::parse(bytes, MAGIC)?;
        if container.total_len() > MAX_LEN {
            return Err(Error::TooLong);
        }
        let board = Board::parse(&value_of(&container, BOARD, BOARD_LEN)?)?;
        let compatibility =
            Compatibility::parse(&value_of(&container, COMPATIBILITY, COMPATIBILITY_LEN)?)?;
        let trust = value_of(&container, TRUST, TRUST_LEN)?;
        let signing = trust.u32(0).and_then(generation_of).ok_or(Error::Trust)?;
        // Every slot the count names must lie in the value; slots appended
        // after the ones `encode` writes are read too.
        let slots = trust.bytes().len().saturating_sub(8) / KEY_LEN;
        let count = trust
            .u32(4)
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| (1..=slots).contains(count))
            .ok_or(Error::Trust)?;
        let block = Self {
            board,
            compatibility,
            trust,
            signing,
            count,
        };
        if (0..count).any(|index| block.key(index).is_none()) {
            return Err(Error::Trust);
        }
        Ok(block)
    }

    /// The board the image is built for.
    #[must_use]
    pub const fn board(&self) -> Board {
        self.board
    }

    /// The configuration schemas the image works with.
    #[must_use]
    pub const fn compatibility(&self) -> Compatibility {
        self.compatibility
    }

    /// The generation the image is signed with.
    #[must_use]
    pub const fn signing(&self) -> Generation {
        self.signing
    }

    /// The keys the image trusts, in the block's order.
    pub fn keys(&self) -> impl Iterator<Item = Key> + 'b {
        let block = *self;
        (0..self.count).filter_map(move |index| block.key(index))
    }

    /// Whether the image trusts the development key.
    #[must_use]
    pub fn trusts_development_key(&self) -> bool {
        self.keys().any(|key| key.is_development())
    }

    /// Key slot `index`; `None` when its generation is past `u16::MAX`.
    fn key(&self, index: usize) -> Option<Key> {
        let at = 8 + index * KEY_LEN;
        Some(Key {
            generation: self.trust.u32(at).and_then(generation_of)?,
            keynum: self.trust.array(at + 4)?,
            ed25519: self.trust.array(at + 12)?,
        })
    }
}

/// The value of the one entry of type `kind` in `container`, at least `min`
/// bytes long. Never inlined, so the entries' iterator stays out of the
/// caller's frame.
#[inline(never)]
fn value_of<'a>(container: &Container<'a>, kind: u16, min: usize) -> Result<Value<'a>, Error> {
    let entry = container.require(kind)?;
    Value::new(entry.value, min).map_err(|TooShort { .. }| Error::TooShort { kind })
}

/// Writes `bytes` at `at` in `out`. Never inlined, so the encoder's frame
/// holds one block.
#[inline(never)]
const fn put(out: &mut [u8; LEN], at: usize, bytes: &[u8]) {
    let mut i = 0;
    while i < bytes.len() {
        out[at + i] = bytes[i];
        i += 1;
    }
}

/// Writes an entry's header for `kind` and a value `len` bytes long at `at`
/// in `out`, and returns the offset of its value.
const fn entry(out: &mut [u8; LEN], at: usize, kind: u16, len: usize) -> usize {
    put(out, at, &kind.to_le_bytes());
    put(out, at + 2, &short(len).to_le_bytes());
    at + ENTRY_HEADER_LEN
}

/// The block of `info`, as an image carries it: the header, then the board,
/// compatibility and trust entries, unused key slots zero.
#[must_use]
pub const fn encode(info: &ImageInfo) -> [u8; LEN] {
    let mut out = [0; LEN];
    put(&mut out, 0, &MAGIC.to_le_bytes());
    put(&mut out, 4, &short(LEN).to_le_bytes());
    let at = encode_board(&mut out, HEADER_LEN, &info.board);
    let at = encode_compatibility(&mut out, at, info.compatibility);
    encode_trust(&mut out, at, &info.trust);
    out
}

/// Writes the board entry of `board` at `at` in `out`, and returns the offset
/// after it.
#[inline(never)]
const fn encode_board(out: &mut [u8; LEN], at: usize, board: &Board) -> usize {
    let at = entry(out, at, BOARD, BOARD_LEN);
    put(out, at, &board.name);
    put(out, at + NAME_LEN, &board.revisions.to_le_bytes());
    at + BOARD_LEN
}

/// Writes the compatibility entry of `compatibility` at `at` in `out`, and
/// returns the offset after it.
#[inline(never)]
const fn encode_compatibility(
    out: &mut [u8; LEN],
    at: usize,
    compatibility: Compatibility,
) -> usize {
    let at = entry(out, at, COMPATIBILITY, COMPATIBILITY_LEN);
    put(out, at, &compatibility.oldest_schema.to_le_bytes());
    put(out, at + 4, &compatibility.schema.to_le_bytes());
    at + COMPATIBILITY_LEN
}

/// Writes the trust entry of `trust` at `at` in `out`. A generation without
/// a number, which [`Trust::new`] refuses, is written as 0.
#[inline(never)]
const fn encode_trust(out: &mut [u8; LEN], at: usize, trust: &Trust) {
    let at = entry(out, at, TRUST, TRUST_LEN);
    let signing = match generation_number(trust.signing) {
        Some(number) => number,
        None => 0,
    };
    put(out, at, &signing.to_le_bytes());
    put(out, at + 4, &(short(trust.count) as u32).to_le_bytes());
    let mut i = 0;
    while i < trust.count {
        let key = &trust.keys[i];
        let slot = at + 8 + i * KEY_LEN;
        let generation = match generation_number(key.generation) {
            Some(number) => number,
            None => 0,
        };
        put(out, slot, &generation.to_le_bytes());
        put(out, slot + 4, &key.keynum);
        put(out, slot + 12, &key.ed25519);
        i += 1;
    }
}

/// What [`read`] finds after an image's layout record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found<'b> {
    /// The image has no layout record, or no block follows it: the bytes
    /// there don't start with [`MAGIC`], as in an image built before blocks
    /// existed.
    Absent,
    /// A block's bytes, its total length long, for [`Block::parse`].
    Block(&'b [u8]),
    /// A header that starts with [`MAGIC`] and gives a total length shorter
    /// than the header, or past [`MAX_LEN`] or the image's first segment.
    BadLength,
}

/// Finds the block of the app image in `source`, right after its layout
/// record, inside the image's first segment, and reads its bytes into
/// `buf`.
///
/// # Errors
///
/// [`ImageError`] when the image's headers don't read or the source fails a
/// read.
pub fn read<'b, S: Source + ?Sized>(
    source: &S,
    buf: &'b mut [u8; MAX_LEN],
) -> Result<Found<'b>, ImageError> {
    let Some(layout) = image::app_layout(source)? else {
        return Ok(Found::Absent);
    };
    let room = usize::try_from(layout.segment_end().saturating_sub(layout.end()))
        .unwrap_or(usize::MAX)
        .min(MAX_LEN);
    let mut header = [0; HEADER_LEN];
    if room < HEADER_LEN {
        return Ok(Found::Absent);
    }
    source.read(layout.end(), &mut header)?;
    let [m0, m1, m2, m3, l0, l1] = header;
    if u32::from_le_bytes([m0, m1, m2, m3]) != MAGIC {
        return Ok(Found::Absent);
    }
    let total = usize::from(u16::from_le_bytes([l0, l1]));
    let Some(block) = buf
        .get_mut(..total)
        .filter(|_| (HEADER_LEN..=room).contains(&total))
    else {
        return Ok(Found::BadLength);
    };
    source.read(layout.end(), block)?;
    Ok(Found::Block(block))
}

#[cfg(test)]
mod tests;
