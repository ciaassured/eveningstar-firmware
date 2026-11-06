//! The storage API: the flash as a medium. Where the regions are, whether the
//! partition table checks out, and which views of a region's bytes may leave
//! the device. What the bytes mean is answered by the API of the service that
//! owns them.
//!
//! The device writes [`Regions`] and [`Layout`] in parts, each element of
//! their lists serialized on its own between the literal parts their `START`,
//! `END` and other constants give, so no whole document is built at once.

#![allow(
    clippy::large_stack_frames,
    reason = "the derived `Deserialize` impls hold every field of a document at once; a client \
    deserializes the documents, never the device"
)]

use core::convert::Infallible;

use serde::{Deserialize, Serialize};

use super::endpoint::{Endpoint, Json, Method, PathParam};
use super::{ErrorCode, Errors, Scope};

/// Most bytes of a partition label.
pub const LABEL_MAX_LEN: usize = 16;

/// Most bytes of a known bootloader's name.
pub const BOOTLOADER_NAME_MAX_LEN: usize = 48;

/// Most bytes of a view's entity tag, quotes included: a 64-bit value in hex,
/// a hyphen and a 64-bit count in decimal.
pub const ETAG_MAX_LEN: usize = 40;

/// Most storage devices.
pub const DEVICES_MAX: usize = 2;

/// Most regions: one of each [`RegionName`].
pub const REGIONS_MAX: usize = RegionName::ALL.len();

/// Most views of a region.
pub const VIEWS_MAX: usize = 2;

/// Most sanitized spans of a view.
pub const SANITIZED_MAX: usize = 4;

/// Most partitions of the compiled layout.
pub const EXPECTED_MAX: usize = 16;

/// Most rows the bootloader loads from a table: the entries of a table sector
/// before its end entry.
pub const FOUND_MAX: usize = 95;

/// Most search candidates reported.
pub const CANDIDATES_MAX: usize = 2;

/// Most fields a comparison names.
pub const FIELDS_MAX: usize = 5;

/// The endpoint answering with the devices, the verification chain's result
/// and what its tier allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetStorage;

impl Endpoint for GetStorage {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get the storage devices and the verification chain's result";

    type Params = ();
    type Request = super::Empty;
    type Response = Json<Storage>;
    type Error = Infallible;
}

/// The endpoint answering with the located partition table, the compiled
/// layout and their comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetLayout;

impl Endpoint for GetLayout {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/layout";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get the located partition table and the compiled layout";
    const DESCRIPTION: Option<&'static str> = Some(
        "The body is sent as it is built, with chunked transfer coding, the \
        found partitions read from the flash as they are written. A read that \
        fails partway ends the body before its JSON does; the client asks \
        again.",
    );

    type Params = ();
    type Request = super::Empty;
    type Response = Json<Layout>;
    type Error = Infallible;
}

/// The endpoint answering with every listed region's document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetRegions;

impl Endpoint for GetRegions {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/regions";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get every region of the storage this boot";

    type Params = ();
    type Request = super::Empty;
    type Response = Json<Regions>;
    type Error = Infallible;
}

/// The endpoint answering with one region's document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetRegion;

impl Endpoint for GetRegion {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/regions/{region}";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get a region of the storage";
    const DESCRIPTION: Option<&'static str> = Some(
        "A region that isn't listed this boot is answered with `404`, as is a \
        name that isn't a region's.",
    );

    type Params = RegionName;
    type Request = super::Empty;
    type Response = Json<Region>;
    type Error = RegionError;
}

/// The endpoint answering with a published view's bytes, whole or by range,
/// with their digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetView;

impl Endpoint for GetView {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/regions/{region}/views/{view}";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Download a view of a region";
    const DESCRIPTION: Option<&'static str> = Some(
        "The view's bytes as the region's document describes it, sanitized \
        bytes as `0xFF`. A sensitive region is answered with `403` whether or \
        not it is listed, a view that isn't published with `409`, and a \
        request while another download or digest is in progress with `503`. \
        A region written while the body is sent ends the body before its \
        length: the client sees a short body, never torn contents. Every \
        answer closes the connection.",
    );

    type Params = (RegionName, ViewName);
    type Request = super::Empty;
    type Response = super::Bytes;
    type Error = ViewError;
}

/// The errors of [`GetView`], besides those of a binary body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// [`ErrorCode::NotFound`]: the region isn't listed this boot, or doesn't
    /// define the view.
    NotFound,
    /// [`ErrorCode::Sensitive`]: the region's bytes never leave the device.
    Sensitive,
    /// [`ErrorCode::Unpublished`]: the view isn't published now; the
    /// region's document says why.
    Unpublished,
    /// [`ErrorCode::Busy`]: another download or digest is in progress.
    Busy,
}

impl Errors for ViewError {
    const CODES: &'static [ErrorCode] = &[
        ErrorCode::NotFound,
        ErrorCode::Sensitive,
        ErrorCode::Unpublished,
        ErrorCode::Busy,
    ];

    fn code(self) -> ErrorCode {
        match self {
            Self::NotFound => ErrorCode::NotFound,
            Self::Sensitive => ErrorCode::Sensitive,
            Self::Unpublished => ErrorCode::Unpublished,
            Self::Busy => ErrorCode::Busy,
        }
    }
}

impl RegionName {
    /// The name as the API writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bootloader => "bootloader",
            Self::PartitionTable => "partition_table",
            Self::Factory => "factory",
            Self::Otadata => "otadata",
            Self::AppCurrent => "app_current",
            Self::AppAlternate => "app_alternate",
            Self::ConfigCurrent => "config_current",
            Self::ConfigAlternate => "config_alternate",
            Self::Diag => "diag",
            Self::Reserved => "reserved",
        }
    }

    /// The views the region's format defines, in order: none for a region
    /// that isn't publishable.
    #[must_use]
    pub const fn views(self) -> &'static [ViewName] {
        match self {
            Self::Bootloader | Self::AppCurrent | Self::AppAlternate => &[ViewName::Image],
            Self::PartitionTable => &[ViewName::Table],
            Self::Otadata => &[ViewName::Entry0, ViewName::Entry1],
            Self::Factory
            | Self::ConfigCurrent
            | Self::ConfigAlternate
            | Self::Diag
            | Self::Reserved => &[],
        }
    }

    /// Whether the region's bytes never leave the device whatever the
    /// board's tier: `factory`, the configurations, `diag` and `reserved`.
    /// The otadata a located table gives is sensitive too, which its
    /// document's class says.
    #[must_use]
    pub const fn is_sensitive(self) -> bool {
        matches!(
            self,
            Self::Factory
                | Self::ConfigCurrent
                | Self::ConfigAlternate
                | Self::Diag
                | Self::Reserved
        )
    }
}

impl ViewName {
    /// The name as the API writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Table => "table",
            Self::Entry0 => "entry_0",
            Self::Entry1 => "entry_1",
        }
    }
}

/// The errors of [`GetRegion`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionError {
    /// [`ErrorCode::NotFound`]: the region isn't listed this boot.
    NotFound,
}

impl Errors for RegionError {
    const CODES: &'static [ErrorCode] = &[ErrorCode::NotFound];

    fn code(self) -> ErrorCode {
        match self {
            Self::NotFound => ErrorCode::NotFound,
        }
    }
}

/// The storage devices, the internal flash's verification chain and what its
/// tier allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Storage {
    /// Every storage device of the board.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<Device>", length(max = DEVICES_MAX))
    )]
    pub devices: heapless::Vec<Device, DEVICES_MAX>,
    /// The internal flash's verification chain.
    pub chain: Chain,
    /// What the chain's tier allows.
    pub allows: Allows,
}

/// A storage device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Device {
    /// Which device.
    pub device: DeviceName,
    /// What kind of memory it is.
    pub kind: DeviceKind,
    /// Whether this firmware reaches it.
    pub status: DeviceStatus,
    /// Its size in bytes, `null` when the device doesn't give it.
    pub capacity: Option<u32>,
}

/// A storage device of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceName {
    /// The flash the chip boots from.
    Internal,
    /// The NAND flash beside the chip.
    External,
}

/// The kind of memory a storage device is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// NOR flash.
    NorFlash,
    /// NAND flash.
    NandFlash,
}

/// Whether this firmware reaches a storage device. Later versions may add
/// statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    /// The firmware reads it.
    Available,
    /// The board has it, and this firmware has no driver for it.
    NotSupported,
    /// A status this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The result of the internal flash's verification chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Chain {
    /// How far the board is trusted.
    pub tier: Tier,
    /// The step that kept the tier below the boot tier, `null` in the boot
    /// tier.
    pub failed: Option<Step>,
    /// Every step's result.
    pub steps: Steps,
    /// The bootloader the chain verified.
    pub bootloader: ChainBootloader,
    /// The offset of the app the bootloader started, `null` when the MMU
    /// doesn't map one.
    pub booted_offset: Option<u32>,
    /// The label of the located table's partition at the booted offset,
    /// `null` when there is none or its label isn't covered.
    pub booted_label: Option<Label>,
}

/// How far the board is trusted, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Only what the chain located is read, and nothing is written.
    Unverified,
    /// The table is the compiled layout and the app booted from a slot: the
    /// configurations are written.
    Layout,
    /// As the layout tier, with a known bootloader that supports rollback:
    /// updates are written too.
    Boot,
}

/// A step of the verification chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The bootloader verifies and is a known one that supports rollback.
    Bootloader,
    /// A single table is located.
    TableOffset,
    /// The located table is valid as the bootloader judges it.
    Table,
    /// The located table is the compiled layout at the compiled offset.
    Layout,
    /// The flash is at least the compiled flash size.
    FlashSize,
    /// The app booted from an OTA slot.
    BootedSlot,
}

/// Whether each step of the verification chain passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one result per step of the chain"
)]
pub struct Steps {
    /// The bootloader verifies and is a known one that supports rollback.
    pub bootloader: bool,
    /// A single table is located.
    pub table_offset: bool,
    /// The located table is valid as the bootloader judges it.
    pub table: bool,
    /// The located table is the compiled layout at the compiled offset.
    pub layout: bool,
    /// The flash is at least the compiled flash size.
    pub flash_size: bool,
    /// The app booted from an OTA slot.
    pub booted_slot: bool,
}

/// The bootloader as the verification chain found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChainBootloader {
    /// Whether it verifies and is listed.
    pub status: BootloaderStatus,
    /// The known bootloader's name, `null` unless the status is `known`.
    #[cfg_attr(
        feature = "schema",
        schemars(
            with = "Option<alloc::string::String>",
            length(max = BOOTLOADER_NAME_MAX_LEN)
        )
    )]
    pub name: Option<heapless::String<BOOTLOADER_NAME_MAX_LEN>>,
    /// Whether the known bootloader supports rollback, `null` unless the
    /// status is `known`.
    pub rollback: Option<bool>,
    /// Whether the image ends at or before the end of the bootloader's
    /// region, `null` when it doesn't verify.
    pub fits: Option<bool>,
}

/// Whether the bootloader verifies and is listed among the known ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BootloaderStatus {
    /// It verifies and is listed.
    Known,
    /// It verifies and isn't listed.
    Unlisted,
    /// It doesn't verify.
    Invalid,
}

/// What the chain's tier allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Allows {
    /// Whether the configuration persists: the layout tier or above.
    pub persistent_config: bool,
    /// Whether firmware updates are written: the boot tier.
    pub updates: bool,
}

/// A partition label: printable ASCII, any other byte the flash holds as `?`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct Label(
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = LABEL_MAX_LEN))
    )]
    pub heapless::String<LABEL_MAX_LEN>,
);

impl Label {
    /// The label stored as `raw`: the bytes before the first NUL, each byte
    /// outside printable ASCII as `?`.
    #[must_use]
    pub fn from_raw(raw: &[u8; LABEL_MAX_LEN]) -> Self {
        let mut label = heapless::String::new();
        for &byte in raw.iter().take_while(|&&byte| byte != 0) {
            let character = if (0x20..=0x7E).contains(&byte) {
                char::from(byte)
            } else {
                '?'
            };
            // At most `LABEL_MAX_LEN` characters of one byte each.
            let _ = label.push(character);
        }
        Self(label)
    }
}

/// The located partition table, the compiled layout, and their comparison.
///
/// The device sends `expected` and `found` as it builds them, an element at a
/// time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Layout {
    /// The table the chain located.
    pub table: Table,
    /// The compiled layout's partitions, in order, each compared with the
    /// located table's partition of its label.
    #[cfg_attr(
        feature = "schema",
        schemars(
            with = "alloc::vec::Vec<ExpectedPartition>",
            length(max = EXPECTED_MAX)
        )
    )]
    pub expected: heapless::Vec<ExpectedPartition, EXPECTED_MAX>,
    /// Every row the bootloader loads from the located table, in table
    /// order: a row's position is its entry's index. Empty unless the
    /// table's status is `valid`.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<FoundRow>", length(max = FOUND_MAX))
    )]
    pub found: heapless::Vec<FoundRow, FOUND_MAX>,
}

impl Layout {
    /// The JSON before the table's.
    pub const START: &'static str = r#"{"table":"#;
    /// The JSON between the table's and the first expected partition's.
    pub const EXPECTED: &'static str = r#","expected":["#;
    /// The JSON between the last expected partition's and the first found
    /// row's.
    pub const FOUND: &'static str = r#"],"found":["#;
    /// The JSON after the last found row's.
    pub const END: &'static str = "]}";
}

/// The partition table the chain located.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Table {
    /// Whether the bootloader accepts it.
    pub status: TableStatus,
    /// The compiled table offset.
    pub expected_offset: u32,
    /// The located table's offset, `null` when none is located.
    pub offset: Option<u32>,
    /// How the table was located, `null` when it wasn't.
    pub source: Option<TableSource>,
    /// What the search found.
    pub search: Search,
    /// The first two offsets the search found when it found several, else
    /// empty.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<u32>", length(max = CANDIDATES_MAX))
    )]
    pub candidates: heapless::Vec<u32, CANDIDATES_MAX>,
    /// Why the bootloader refuses the table, `null` unless the status is
    /// `refused`.
    pub failure: Option<TableFailure>,
    /// What covers the table's bytes, `null` unless the status is `valid`.
    pub coverage: Option<Coverage>,
    /// Whether the bootloader loads the MD5 entry as a partition, `null`
    /// unless the status is `valid`.
    pub md5_read_as_partition: Option<bool>,
}

/// Whether the bootloader accepts the located table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TableStatus {
    /// The bootloader accepts it, and its partitions read.
    Valid,
    /// The bootloader refuses it.
    Refused,
    /// It is located, and the firmware holds no reader for it.
    Unread,
    /// No table is located.
    NotLocated,
}

/// How the table was located.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TableSource {
    /// At the offset the known bootloader reads.
    KnownBootloader,
    /// By the search, which found it alone.
    Searched,
}

/// What the search for the table found. It runs only for a bootloader that
/// isn't known, whose header reads, and a booted offset inside the flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Search {
    /// The search didn't run.
    NotRun,
    /// It found no table.
    None,
    /// It found one table.
    One,
    /// It found several tables, of which none is located.
    Several,
}

/// Why the bootloader refuses a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableFailure {
    /// What is wrong.
    pub code: TableFailureCode,
    /// The index of the entry at fault, `null` for a code that has none.
    pub index: Option<u16>,
}

/// What is wrong with a table the bootloader refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TableFailureCode {
    /// An entry is neither a partition, the MD5 entry nor the end entry.
    Magic,
    /// The table holds no partition.
    Empty,
    /// The table has no end entry.
    Unterminated,
    /// A partition ends past the chip size the bootloader checks against.
    PastChip,
    /// A second MD5 entry.
    SecondMd5,
    /// The MD5 entry's digest isn't the digest of the entries before it.
    Md5,
    /// The flash failed to read.
    Read,
}

/// What covers a valid table's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// The partitions are the compiled layout's, at the compiled offset.
    Layout,
    /// The MD5 entry covers the entries before it.
    Md5,
    /// Nothing does: no byte and no label of it is published.
    Uncovered,
}

/// A partition of the compiled layout, compared with the located table's
/// partition of its label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExpectedPartition {
    /// Its label.
    pub label: Label,
    /// Its type.
    #[serde(rename = "type")]
    pub partition_type: u8,
    /// Its subtype.
    pub subtype: u8,
    /// Its offset in bytes.
    pub offset: u32,
    /// Its size in bytes.
    pub size: u32,
    /// Its flags.
    pub flags: u32,
    /// How the located table's partition of its label compares.
    pub comparison: Comparison,
}

/// How the located table's partition of a label compares with the compiled
/// layout's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Comparison {
    /// Whether it matches.
    pub status: ComparisonStatus,
    /// The fields that differ, empty unless the status is `differs`.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<Field>", length(max = FIELDS_MAX))
    )]
    pub fields: heapless::Vec<Field, FIELDS_MAX>,
}

/// Whether the located table's partition of a label matches the compiled
/// layout's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    /// Every field is equal.
    Matches,
    /// Some fields differ.
    Differs,
    /// The table has no partition of the label.
    Missing,
}

/// A field of a partition entry, other than its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The type.
    Type,
    /// The subtype.
    Subtype,
    /// The offset.
    Offset,
    /// The size.
    Size,
    /// The flags.
    Flags,
}

/// A row the bootloader loads from the located table as a partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FoundRow {
    /// Whether it is a partition entry or the MD5 entry.
    pub kind: RowKind,
    /// The label, `null` for the MD5 entry and when nothing covers it.
    pub label: Option<Label>,
    /// The type, `null` for the MD5 entry.
    #[serde(rename = "type")]
    pub partition_type: Option<u8>,
    /// The subtype, `null` for the MD5 entry.
    pub subtype: Option<u8>,
    /// The offset in bytes, `null` for the MD5 entry.
    pub offset: Option<u32>,
    /// The size in bytes, `null` for the MD5 entry.
    pub size: Option<u32>,
    /// The flags, `null` for the MD5 entry.
    pub flags: Option<u32>,
    /// Whether the compiled layout has no partition of its label, `null`
    /// when the label is `null`. Labels compare as all 16 bytes.
    pub extra: Option<bool>,
}

/// What a found row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// A partition entry.
    Partition,
    /// The MD5 entry, which the bootloader loads as a partition when entries
    /// lie between it and the end entry. Its fields lie in bytes the table's
    /// view sanitizes.
    Md5,
}

/// Every region listed this boot, in offset order.
///
/// The device sends `regions` as it builds it, an element at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Regions {
    /// Each region's document.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<Region>", length(max = REGIONS_MAX))
    )]
    pub regions: heapless::Vec<Region, REGIONS_MAX>,
}

impl Regions {
    /// The JSON before the first region's.
    pub const START: &'static str = r#"{"regions":["#;
    /// The JSON after the last region's.
    pub const END: &'static str = "]}";
}

/// The JSON between two elements of a list.
pub const SEPARATOR: &str = ",";

/// A region of the storage: where it is this boot, whether its bytes may
/// leave the device, and its views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Region {
    /// Its name.
    pub region: RegionName,
    /// The device that holds it.
    pub device: DeviceName,
    /// Whether its bytes may leave the device.
    pub class: Class,
    /// Where it is on the device.
    pub span: Span,
    /// The located table's entry for it, `null` for a region no entry
    /// describes.
    pub partition: Option<Partition>,
    /// The views its format defines, published or not, in order. Empty for a
    /// region that isn't `publishable`.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<View>", length(max = VIEWS_MAX))
    )]
    pub views: heapless::Vec<View, VIEWS_MAX>,
}

/// A region of the storage. Region names are one namespace across devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RegionName {
    /// The bootloader, from 0x0 up to the located or compiled table offset.
    Bootloader,
    /// The located table's sector.
    PartitionTable,
    /// The `factory` partition.
    Factory,
    /// The located otadata.
    Otadata,
    /// The app slot that booted, or the app the chain found.
    AppCurrent,
    /// The app slot that didn't boot.
    AppAlternate,
    /// The current slot's configuration.
    ConfigCurrent,
    /// The alternate slot's configuration.
    ConfigAlternate,
    /// The `diag` partition.
    Diag,
    /// The `reserved` partition.
    Reserved,
}

impl RegionName {
    /// Every region name.
    pub const ALL: [Self; 10] = [
        Self::Bootloader,
        Self::PartitionTable,
        Self::Factory,
        Self::Otadata,
        Self::AppCurrent,
        Self::AppAlternate,
        Self::ConfigCurrent,
        Self::ConfigAlternate,
        Self::Diag,
        Self::Reserved,
    ];
}

impl PathParam for RegionName {}

/// Whether a region's bytes may leave the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Its views may be downloaded once published.
    Publishable,
    /// Its bytes never leave the device, as bytes or as a digest.
    Sensitive,
    /// It holds nothing yet, and the firmware has no handle for it.
    Reserved,
}

/// A span of bytes: from `offset`, `size` bytes, the end excluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Span {
    /// Where it starts, in bytes.
    pub offset: u32,
    /// Its length in bytes.
    pub size: u32,
}

/// The located table's entry for a region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Partition {
    /// Its label, `null` when the label isn't covered.
    pub label: Option<Label>,
    /// Its type.
    #[serde(rename = "type")]
    pub partition_type: u8,
    /// Its subtype.
    pub subtype: u8,
    /// Its flags.
    pub flags: u32,
}

/// A view of a region: bytes a content module verified, in the format's
/// canonical form, the unit of download.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct View {
    /// Its name.
    pub view: ViewName,
    /// Whether it is published, or why not.
    pub status: ViewStatus,
    /// Where it starts within the region's span, `null` unless published.
    pub offset: Option<u32>,
    /// Its length in bytes, `null` unless published.
    pub length: Option<u32>,
    /// The spans within the view served as `0xFF` that aren't erased on the
    /// flash, `null` unless published.
    #[cfg_attr(
        feature = "schema",
        schemars(
            with = "Option<alloc::vec::Vec<Span>>",
            length(max = SANITIZED_MAX)
        )
    )]
    pub sanitized: Option<heapless::Vec<Span, SANITIZED_MAX>>,
    /// The download's strong entity tag, quotes included, `null` unless
    /// published.
    #[cfg_attr(
        feature = "schema",
        schemars(
            with = "Option<alloc::string::String>",
            length(max = ETAG_MAX_LEN)
        )
    )]
    pub etag: Option<heapless::String<ETAG_MAX_LEN>>,
}

/// A view of a region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ViewName {
    /// An app or bootloader image, its digest included.
    Image,
    /// The entries of a partition table its check covers.
    Table,
    /// The otadata's first entry.
    #[serde(rename = "entry_0")]
    Entry0,
    /// The otadata's second entry.
    #[serde(rename = "entry_1")]
    Entry1,
}

impl PathParam for ViewName {}

/// Whether a view is published, or why not. A status other than `published`
/// can be transient. Later versions may add statuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ViewStatus {
    /// It may be downloaded.
    Published,
    /// A write session is open on the region.
    Writing,
    /// The region is stable, and its contents haven't been checked since it
    /// was last written.
    Checking,
    /// The contents don't pass the format's check.
    Unverified,
    /// The flash failed to read.
    Unreadable,
    /// A status this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(text: &str) -> Label {
        Label(text.try_into().unwrap())
    }

    fn region(name: RegionName) -> Region {
        Region {
            region: name,
            device: DeviceName::Internal,
            class: Class::Publishable,
            span: Span {
                offset: 0x1_0000,
                size: 0x30_0000,
            },
            partition: Some(Partition {
                label: Some(label("ota_0")),
                partition_type: 0,
                subtype: 0x10,
                flags: 0,
            }),
            views: heapless::Vec::from_slice(&[View {
                view: ViewName::Image,
                status: ViewStatus::Published,
                offset: Some(0),
                length: Some(1_203_456),
                sanitized: Some(heapless::Vec::new()),
                etag: Some("\"5c1e9a0f3b2d7781-3\"".try_into().unwrap()),
            }])
            .unwrap(),
        }
    }

    #[test]
    fn a_region_reads_as_the_spec_writes_it() {
        let json = serde_json::json!({
            "region": "app_current",
            "device": "internal",
            "class": "publishable",
            "span": { "offset": 65_536, "size": 3_145_728 },
            "partition": { "label": "ota_0", "type": 0, "subtype": 16, "flags": 0 },
            "views": [{
                "view": "image",
                "status": "published",
                "offset": 0,
                "length": 1_203_456,
                "sanitized": [],
                "etag": "\"5c1e9a0f3b2d7781-3\""
            }]
        });
        assert_eq!(
            serde_json::to_value(region(RegionName::AppCurrent)).unwrap(),
            json
        );
        assert_eq!(
            serde_json::from_value::<Region>(json).unwrap(),
            region(RegionName::AppCurrent)
        );
    }

    #[test]
    fn region_and_view_names_are_path_parameters() {
        for (name, expected) in [
            ("partition_table", Some(RegionName::PartitionTable)),
            ("app_alternate", Some(RegionName::AppAlternate)),
            ("config_current", Some(RegionName::ConfigCurrent)),
            ("App_current", None),
            ("app-current", None),
            ("", None),
        ] {
            assert_eq!(
                super::super::endpoint::path_params::<RegionName>("{region}", name),
                expected,
                "{name}"
            );
        }
        for (name, expected) in [
            ("entry_0", Some(ViewName::Entry0)),
            ("entry_1", Some(ViewName::Entry1)),
            ("image", Some(ViewName::Image)),
            ("entry0", None),
            ("entry_2", None),
        ] {
            assert_eq!(
                super::super::endpoint::path_params::<ViewName>("{view}", name),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn names_are_written_as_they_serialize() {
        for name in RegionName::ALL {
            assert_eq!(
                serde_json::to_value(name).unwrap(),
                serde_json::Value::from(name.as_str())
            );
            for view in name.views() {
                assert_eq!(
                    serde_json::to_value(view).unwrap(),
                    serde_json::Value::from(view.as_str())
                );
            }
            assert_eq!(name.is_sensitive(), name.views().is_empty(), "{name:?}");
        }
    }

    #[test]
    fn a_view_path_has_a_region_and_a_view() {
        use super::super::endpoint::path_params;
        assert_eq!(
            path_params::<(RegionName, ViewName)>(
                GetView::PATH,
                "/storage/regions/otadata/views/entry_1"
            ),
            Some((RegionName::Otadata, ViewName::Entry1))
        );
        for path in [
            "/storage/regions/otadata/views/entry_2",
            "/storage/regions/other/views/image",
            "/storage/regions/otadata/views",
            "/storage/regions/otadata/views/entry_1/more",
        ] {
            assert_eq!(
                path_params::<(RegionName, ViewName)>(GetView::PATH, path),
                None,
                "{path}"
            );
        }
    }

    #[test]
    fn every_region_name_is_listed_once() {
        let names: Vec<_> = RegionName::ALL
            .iter()
            .map(|name| serde_json::to_string(name).unwrap())
            .collect();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
    }

    #[test]
    fn labels_keep_printable_ascii_up_to_the_first_nul() {
        let mut raw = [0; LABEL_MAX_LEN];
        raw[..5].copy_from_slice(b"ota_0");
        assert_eq!(Label::from_raw(&raw), label("ota_0"));
        let mut raw = [b'a'; LABEL_MAX_LEN];
        raw[1] = 0x7F;
        raw[2] = 0xC3;
        raw[3] = b'\n';
        assert_eq!(Label::from_raw(&raw), label("a???aaaaaaaaaaaa"));
        raw[4] = 0;
        assert_eq!(Label::from_raw(&raw), label("a???"));
        assert_eq!(Label::from_raw(&[0; LABEL_MAX_LEN]), label(""));
    }

    /// `elements`' JSON, each serialized on its own, separated as a list's.
    fn list<T: Serialize>(elements: &[T]) -> String {
        elements
            .iter()
            .map(|element| serde_json::to_string(element).unwrap())
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }

    #[test]
    fn regions_written_in_parts_are_the_document() {
        for names in [
            &[][..],
            &[RegionName::Bootloader][..],
            &[RegionName::AppCurrent, RegionName::AppAlternate][..],
        ] {
            let regions: Vec<_> = names.iter().map(|&name| region(name)).collect();
            let document = Regions {
                regions: heapless::Vec::from_slice(&regions).unwrap(),
            };
            let parts = [Regions::START, &list(&regions), Regions::END].concat();
            assert_eq!(parts, serde_json::to_string(&document).unwrap());
        }
    }

    #[test]
    fn a_layout_reads_as_the_spec_writes_it() {
        let json = serde_json::json!({
            "table": {
                "status": "valid",
                "expected_offset": 32768,
                "offset": 32768,
                "source": "known_bootloader",
                "search": "not_run",
                "candidates": [],
                "failure": null,
                "coverage": "layout",
                "md5_read_as_partition": false
            },
            "expected": [{
                "label": "config_0", "type": 1, "subtype": 64, "offset": 6_356_992,
                "size": 131_072, "flags": 0,
                "comparison": { "status": "differs", "fields": ["offset", "size"] }
            }],
            "found": [
                {
                    "kind": "partition", "label": "ota_0", "type": 0, "subtype": 16,
                    "offset": 65_536, "size": 3_145_728, "flags": 0, "extra": false
                },
                {
                    "kind": "md5", "label": null, "type": null, "subtype": null,
                    "offset": null, "size": null, "flags": null, "extra": null
                }
            ]
        });
        let layout: Layout = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(layout.found[1].kind, RowKind::Md5);
        assert_eq!(
            layout.expected[0].comparison.fields,
            [Field::Offset, Field::Size]
        );
        assert_eq!(serde_json::to_value(&layout).unwrap(), json);

        let parts = [
            Layout::START,
            &serde_json::to_string(&layout.table).unwrap(),
            Layout::EXPECTED,
            &list(&layout.expected),
            Layout::FOUND,
            &list(&layout.found),
            Layout::END,
        ]
        .concat();
        assert_eq!(parts, serde_json::to_string(&layout).unwrap());
        let empty = Layout {
            table: layout.table.clone(),
            expected: heapless::Vec::new(),
            found: heapless::Vec::new(),
        };
        let parts = [
            Layout::START,
            &serde_json::to_string(&layout.table).unwrap(),
            Layout::EXPECTED,
            Layout::FOUND,
            Layout::END,
        ]
        .concat();
        assert_eq!(parts, serde_json::to_string(&empty).unwrap());
    }

    #[cfg(feature = "schema")]
    #[test]
    fn the_schemas_accept_the_documents() {
        let value = serde_json::to_value(region(RegionName::AppCurrent)).unwrap();
        assert!(crate::schema_accepts::<Region>(&value));
        let regions = serde_json::json!({ "regions": [value] });
        assert!(crate::schema_accepts::<Regions>(&regions));
        let mut bad = regions.clone();
        bad["regions"][0]["region"] = "app".into();
        assert!(!crate::schema_accepts::<Regions>(&bad));
    }
}
