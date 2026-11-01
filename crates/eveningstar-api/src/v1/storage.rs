//! The storage endpoints: the board's storage devices, the internal flash, its
//! regions and partition table, and the content of its public regions.
//!
//! Every storage endpoint only reads. The content of a region leaves the
//! device only when the region is [`Contents::Public`]; a sensitive region
//! never does, as bytes or as a digest.

#![allow(
    clippy::large_stack_frames,
    reason = "the derived `Clone` and `Deserialize` of the larger responses hold a whole response on the stack; they are for clients, and the firmware serialises the responses by reference"
)]

use core::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::endpoint::errors;
use super::{Bytes, Empty, Endpoint, ErrorCode, Errors, Json, Method, PathParam, Scope};

/// The endpoint answering with the board's [`StorageDevices`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetStorage;

impl Endpoint for GetStorage {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "List the board's storage devices";

    type Params = ();
    type Request = Empty;
    type Response = Json<StorageDevices>;
    type Error = core::convert::Infallible;
}

/// The endpoint answering with the state of the [`InternalFlash`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetInternalFlash;

impl Endpoint for GetInternalFlash {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/internal";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str =
        "Get the internal flash's trust tier, bootloader, boot state and app slots";

    type Params = ();
    type Request = Empty;
    type Response = Json<InternalFlash>;
    type Error = core::convert::Infallible;
}

/// The endpoint answering with the internal flash's [`Regions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetRegions;

impl Endpoint for GetRegions {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/internal/regions";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "List the regions of the internal flash";

    type Params = ();
    type Request = Empty;
    type Response = Json<Regions>;
    type Error = core::convert::Infallible;
}

/// The endpoint answering with the [`BoardTable`]: the partition table the
/// bootloader read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetPartitionTable;

impl Endpoint for GetPartitionTable {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/internal/table";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Get the partition table the bootloader read";

    type Params = ();
    type Request = Empty;
    type Response = Json<BoardTable>;
    type Error = core::convert::Infallible;
}

/// The endpoint answering with the content of a public [`Region`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GetRegionContent;

impl Endpoint for GetRegionContent {
    const METHOD: Method = Method::Get;
    const PATH: &'static str = "/storage/internal/regions/{region}/content";
    const STATUS: u16 = 200;
    const SCOPE: Scope = Scope::Read;
    const SUMMARY: &'static str = "Download a public region of the internal flash";
    const DESCRIPTION: Option<&'static str> = Some(
        "The whole region, erased bytes included, or the part a `Range` header \
        asks for. Offsets are relative to the region. The `ETag` changes \
        whenever the region may have changed, and on every boot, so a download \
        resumed with `If-Range` never mixes two contents. The device serves one \
        download at a time, and closes the connection early when the region is \
        written while it streams.",
    );

    type Params = Region;
    type Request = Empty;
    type Response = Bytes;
    type Error = RegionContentError;
}

errors! {
    /// The errors of downloading a region.
    RegionContentError { Sensitive, Unavailable, Busy }
}

/// Most devices a [`StorageDevices`] lists.
pub const STORAGE_DEVICES_MAX: usize = 4;

/// The board's storage devices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageDevices {
    /// Every storage device on the board, supported or not.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<StorageDevice>", length(max = STORAGE_DEVICES_MAX))
    )]
    pub devices: heapless::Vec<StorageDevice, STORAGE_DEVICES_MAX>,
}

/// A storage device on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageDevice {
    /// Which device it is; also the path segment of its endpoints under
    /// `/storage`.
    pub id: StorageId,
    /// What kind of memory it is.
    pub kind: StorageKind,
    /// Whether this firmware supports it.
    pub status: StorageStatus,
    /// Its size in bytes, when the firmware knows it.
    pub capacity: Option<u64>,
}

/// A storage device. Later versions may add devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StorageId {
    /// The flash inside the ESP32-C6 module, holding the bootloader, the
    /// partition table, the firmware and its data.
    Internal,
    /// The flash chip beside the module, for bulk data.
    External,
    /// A device this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// A kind of memory. Later versions may add kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// NOR flash.
    NorFlash,
    /// NAND flash.
    NandFlash,
    /// A kind this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Whether the firmware supports a storage device. Later versions may add
/// states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StorageStatus {
    /// The firmware supports it, and its endpoints answer.
    Available,
    /// It is on the board, and this firmware has no support for it.
    NotSupported,
    /// A state this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Longest text read from the flash, such as a version, in [`AppDescriptor`]
/// and [`BootloaderInfo`]. Text read from the flash is printable ASCII: any
/// other byte is written `?`.
pub const FLASH_TEXT_MAX_LEN: usize = 32;

/// Longest partition label: the partition table's limit.
pub const LABEL_MAX_LEN: usize = 16;

/// Longest name of a known bootloader.
pub const BOOTLOADER_NAME_MAX_LEN: usize = 48;

/// Text read from the flash: printable ASCII, any other byte written `?`.
pub type FlashText = heapless::String<FLASH_TEXT_MAX_LEN>;

/// A partition label, as text read from the flash.
pub type Label = heapless::String<LABEL_MAX_LEN>;

/// The internal flash: how far the firmware trusts it, and what it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InternalFlash {
    /// Size of the flash chip in bytes, from its JEDEC ID; 0 when the ID
    /// isn't one the firmware knows.
    pub capacity: u32,
    /// Size of the flash the firmware's partition layout is for.
    pub layout_size: u32,
    /// How far the firmware trusts the flash.
    pub tier: Tier,
    /// Why the tier is below [`Tier::Full`]; `null` in the full tier.
    pub limit: Option<Limit>,
    /// Whether the firmware saves its configuration.
    pub persistent_config: bool,
    /// Whether the firmware can install updates and roll them back.
    pub updates: bool,
    /// Offset of the app the bootloader started; `null` when the flash's
    /// memory mapping doesn't say.
    pub booted_offset: Option<u32>,
    /// The partition table.
    pub table: TableSummary,
    /// The bootloader.
    pub bootloader: BootloaderInfo,
    /// What the otadata selects; `null` in the untrusted tier, where it isn't
    /// read.
    pub boot: Option<BootState>,
    /// Both entries of the otadata, in sector order; empty in the untrusted
    /// tier.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<OtadataEntry>", length(max = 2))
    )]
    pub otadata: heapless::Vec<OtadataEntry, 2>,
    /// The app slots the firmware can read: both in the data and full tiers,
    /// only the running app in the untrusted tier.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<AppSlot>", length(max = 2))
    )]
    pub apps: heapless::Vec<AppSlot, 2>,
    /// The background checks of the app slots.
    pub checks: Checks,
}

/// How far the firmware trusts the internal flash. Later versions may add
/// tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// The flash isn't laid out as the firmware expects: it only reads the
    /// boot chain, and runs on a default configuration it doesn't save.
    Untrusted,
    /// The layout is the firmware's: it saves its configuration, and can't
    /// install updates.
    Data,
    /// The bootloader is also known to support rollback: updates are
    /// available.
    Full,
    /// A tier this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The first check that kept the internal flash below the full tier. Later
/// versions may add checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    /// The partition table at the compiled offset isn't the firmware's
    /// layout. Untrusted tier.
    Table,
    /// The flash is smaller than the layout. Untrusted tier.
    FlashSize,
    /// The app didn't boot from one of the layout's app slots. Untrusted
    /// tier.
    NotASlot,
    /// The bootloader isn't known, and another partition table could be the
    /// one it read. Untrusted tier.
    OtherTable,
    /// The bootloader doesn't verify. Data tier.
    BootloaderInvalid,
    /// The bootloader verifies, and isn't one the firmware knows to support
    /// rollback. Data tier.
    BootloaderUnlisted,
    /// The running app's descriptor doesn't read. Data tier.
    AppDescriptor,
    /// A check this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The partition table at the compiled offset, compared with the layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableSummary {
    /// Offset the firmware reads the table at.
    pub offset: u32,
    /// Whether the table there matches the layout, entry by entry. Its MD5
    /// isn't checked.
    pub matches: bool,
    /// Offset of the table the bootloader read, when the firmware knows it.
    pub board_offset: Option<u32>,
}

/// The bootloader at the start of the flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BootloaderInfo {
    /// Whether it verifies, and whether the firmware knows it.
    pub status: BootloaderStatus,
    /// Its name, when the firmware knows it.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<alloc::string::String>", length(max = BOOTLOADER_NAME_MAX_LEN))
    )]
    pub name: Option<heapless::String<BOOTLOADER_NAME_MAX_LEN>>,
    /// Why it doesn't verify, when it doesn't.
    pub error: Option<ImageFault>,
    /// Length of its image in bytes, when it verifies.
    pub size: Option<u32>,
    /// The SHA-256 its image stores, over every byte before it (the
    /// "Validation Hash" of `esptool image_info`), when it verifies.
    pub digest: Option<Sha256>,
    /// The SHA-256 of its whole image, the stored digest included (what
    /// `sha256sum` prints for the image alone), when it verifies.
    pub file_sha256: Option<Sha256>,
    /// The version its descriptor gives; `null` without a descriptor, as
    /// ESP-IDF releases before 5.3 build them.
    pub version: Option<u32>,
    /// The ESP-IDF version it was built with, from its descriptor.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<alloc::string::String>", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub idf_version: Option<FlashText>,
    /// When it was built, from its descriptor.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<alloc::string::String>", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub built: Option<FlashText>,
}

/// Whether the bootloader verifies, and whether the firmware knows it. Later
/// versions may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BootloaderStatus {
    /// It verifies, and the firmware knows it to support rollback.
    Known,
    /// It verifies, and isn't in the firmware's list of known bootloaders.
    Unlisted,
    /// It doesn't verify.
    Invalid,
    /// A state this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Why an image doesn't verify. Later versions may add faults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ImageFault {
    /// Reading the flash failed.
    ReadFailed,
    /// The region ends before the image does.
    PastEnd,
    /// The image doesn't start with the image magic byte.
    Magic,
    /// The image is for another chip.
    Chip,
    /// The image has more segments than an image can.
    TooManySegments,
    /// A segment's length isn't a multiple of 4.
    SegmentLength,
    /// A segment runs past the end of the region.
    SegmentPastEnd,
    /// A segment can't be mapped from the flash where it lies.
    SegmentMapping,
    /// The checksum doesn't match the segments.
    Checksum,
    /// The image has no appended SHA-256 digest.
    NoDigest,
    /// The appended SHA-256 digest doesn't match the image.
    DigestMismatch,
    /// The image has no app descriptor.
    MissingDescriptor,
    /// A fault this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// What the otadata selects for booting, and the running app's state in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BootState {
    /// The slot the app booted from.
    pub running: SlotName,
    /// The slot the otadata selects; `null` when no entry is valid, as on a
    /// first boot. A selected slot other than the running one means the
    /// bootloader fell back, as the selected one failed to load.
    pub selected: Option<SlotName>,
    /// The running app's state; `null` when no entry selects it.
    pub state: Option<OtaState>,
    /// Whether the other slot holds an image the bootloader would roll back
    /// to.
    pub rollback_target: bool,
    /// Whether the running app has to be confirmed, or the bootloader rolls
    /// it back at the next boot.
    pub needs_confirmation: bool,
}

/// An app slot. Later versions may add slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SlotName {
    /// The partition `ota_0`.
    #[serde(rename = "ota_0")]
    Ota0,
    /// The partition `ota_1`.
    #[serde(rename = "ota_1")]
    Ota1,
    /// A slot this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The state of an image in the otadata, as ESP-IDF defines it. Later
/// versions may add states.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OtaState {
    /// Selected, and not booted yet.
    New,
    /// Booted once, and waiting to be confirmed.
    PendingVerify,
    /// Confirmed.
    Valid,
    /// Marked as not working.
    Invalid,
    /// Rolled back from, unconfirmed.
    Aborted,
    /// ESP-IDF's state for an entry written without one.
    Undefined,
    /// A value ESP-IDF doesn't define.
    Other,
    /// A state this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// One entry of the otadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OtadataEntry {
    /// The entry's sector: 0 or 1.
    pub sector: u8,
    /// Whether the bootloader would take the entry.
    pub validity: EntryValidity,
    /// Its sequence number; `null` when blank or unreadable.
    pub seq: Option<u32>,
    /// The state it stores; `null` when blank or unreadable.
    pub state: Option<OtaState>,
    /// The slot its sequence number selects; `null` when blank or
    /// unreadable.
    pub selects: Option<SlotName>,
}

/// Whether the bootloader would take an otadata entry. Later versions may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EntryValidity {
    /// It would.
    Valid,
    /// The entry is erased.
    Blank,
    /// Its CRC doesn't match.
    Corrupt,
    /// Its state is `invalid`.
    Invalid,
    /// Its state is `aborted`.
    Aborted,
    /// Reading it failed.
    Unreadable,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// An app slot the firmware can read, and the image in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppSlot {
    /// The region: [`Region::AppCurrent`] or [`Region::AppAlternate`].
    pub region: Region,
    /// The slot it is; `null` when the app didn't boot from one.
    pub slot: Option<SlotName>,
    /// Offset of the region in the flash.
    pub offset: u32,
    /// Size of the region in bytes.
    pub size: u32,
    /// The image in it.
    pub image: AppImage,
}

/// The image in an app slot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppImage {
    /// What the background check found.
    pub status: ImageStatus,
    /// Seconds after boot the check finished; `null` while checking.
    pub checked_uptime_s: Option<u64>,
    /// Why the image doesn't verify, when [`ImageStatus::Invalid`].
    pub error: Option<ImageFault>,
    /// Length of the image in bytes, read from its headers without hashing,
    /// so known while checking; `null` when the headers don't read.
    pub size: Option<u32>,
    /// The SHA-256 the image stores, over every byte before it (the
    /// "Validation Hash" of `esptool image_info`), when
    /// [`ImageStatus::Valid`].
    pub digest: Option<Sha256>,
    /// The SHA-256 of the whole image, the stored digest included (what
    /// `sha256sum` prints for the image alone), when [`ImageStatus::Valid`].
    pub file_sha256: Option<Sha256>,
    /// The image's descriptor, read without hashing, so known while
    /// checking; `null` when it doesn't read.
    pub descriptor: Option<AppDescriptor>,
}

/// What the background check found in an app slot. Later versions may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ImageStatus {
    /// Not checked yet, or written since it was.
    Checking,
    /// The image verifies: its checksum and stored digest match.
    Valid,
    /// The slot holds no image: its header is erased.
    Empty,
    /// The image doesn't verify.
    Invalid,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// An app's descriptor: what the build wrote into the image about itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppDescriptor {
    /// The project's name.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub project_name: FlashText,
    /// The app's version.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub version: FlashText,
    /// The date it was built.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub date: FlashText,
    /// The time it was built.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub time: FlashText,
    /// The ESP-IDF version, or what stands for it, it was built with.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = FLASH_TEXT_MAX_LEN))
    )]
    pub idf_version: FlashText,
    /// Its secure version, for anti-rollback.
    pub secure_version: u32,
    /// The SHA-256 of the ELF file it was built from, as the build wrote it.
    pub elf_sha256: Sha256,
}

/// The background checks of the app slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Checks {
    /// Whether every slot has a current result.
    pub status: CheckStatus,
    /// Seconds after boot the last check finished, when done.
    pub finished_uptime_s: Option<u64>,
}

/// Whether the background checks have finished. Later versions may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// A slot hasn't been checked, or was written since.
    Running,
    /// Every slot has a current result.
    Done,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Most regions a [`Regions`] lists.
pub const REGIONS_MAX: usize = 12;

/// The regions of the internal flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Regions {
    /// Every region, in offset order: the bootloader, the partition table,
    /// then the layout's partitions. Space between them is unallocated.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<RegionInfo>", length(max = REGIONS_MAX))
    )]
    pub regions: heapless::Vec<RegionInfo, REGIONS_MAX>,
}

/// A region of the internal flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RegionInfo {
    /// The region's name, as its content's path names it.
    pub region: Region,
    /// The layout's label for it; `null` for the bootloader and the table.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<alloc::string::String>", length(max = LABEL_MAX_LEN))
    )]
    pub label: Option<Label>,
    /// The layout's entry for it; `null` for the bootloader and the table.
    pub partition: Option<PartitionKind>,
    /// Offset in the flash.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
    /// Whether its content may leave the device.
    pub contents: Contents,
    /// Whether the firmware can read it this boot. A region is downloadable
    /// when it is [`Contents::Public`] and available.
    pub available: bool,
    /// How the table at the compiled offset compares with the layout's entry
    /// for it; `null` for the bootloader and the table.
    pub layout: Option<LayoutMatch>,
    /// What it is to the boot.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<Role>", length(max = ROLES_MAX))
    )]
    pub roles: heapless::Vec<Role, ROLES_MAX>,
}

/// A region of the internal flash, named by what it is to the running
/// firmware. Later versions may add regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// From the start of the flash up to the partition table.
    Bootloader,
    /// The partition table the bootloader read.
    PartitionTable,
    /// The otadata, which selects the slot to boot.
    Otadata,
    /// The running app: its slot, or its image when it didn't boot from one.
    AppCurrent,
    /// The app slot that isn't running.
    AppAlternate,
    /// The running slot's configuration.
    ConfigCurrent,
    /// The other slot's configuration.
    ConfigAlternate,
    /// Diagnostics.
    Diag,
    /// Data written when the board was made.
    Factory,
    /// Space kept for later firmware.
    Reserved,
    /// A region this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

impl PathParam for Region {
    fn is_known(&self) -> bool {
        *self != Self::Unknown
    }
}

/// The layout's entry for a partition, as the partition table stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PartitionKind {
    /// Its type: 0 for an app, 1 for data.
    #[serde(rename = "type")]
    pub kind: u8,
    /// Its subtype, which depends on the type.
    pub subtype: u8,
    /// Its flag bits; bit 1 is read-only.
    pub flags: u32,
}

/// Whether a region's content may leave the device. Later versions may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Contents {
    /// It may: the boot chain, the app slots and the otadata.
    Public,
    /// It never leaves the device, as bytes or as a digest: the
    /// configurations, diagnostics and factory data.
    Sensitive,
    /// It holds nothing yet, and the firmware doesn't read it.
    Reserved,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// How the table at the compiled offset compares with the layout's entry for
/// a partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LayoutMatch {
    /// Whether the table has the entry as the layout does.
    pub status: MatchStatus,
    /// The fields that differ, when [`MatchStatus::Differs`].
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<EntryField>", length(max = ENTRY_FIELDS_MAX))
    )]
    pub fields: heapless::Vec<EntryField, ENTRY_FIELDS_MAX>,
}

/// Number of fields of a partition table entry.
pub const ENTRY_FIELDS_MAX: usize = 6;

/// Most roles a region has.
pub const ROLES_MAX: usize = 3;

/// Whether the table has a layout's entry as the layout does. Later versions
/// may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    /// It does.
    Matches,
    /// The entry in its place differs in some fields.
    Differs,
    /// The table ends before it.
    Missing,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// A field of a partition table entry. Later versions may add fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum EntryField {
    /// The label.
    Label,
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
    /// A field this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// What a region is to the boot. Later versions may add roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The app slot running.
    Running,
    /// The app slot the otadata selects for the next boot.
    Selected,
    /// The app slot the bootloader would roll back to.
    RollbackTarget,
    /// A role this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// Most entries a [`TableRead`] lists.
pub const TABLE_ENTRIES_MAX: usize = 16;

/// The partition table the bootloader read, and how the firmware found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BoardTable {
    /// How the firmware found the table.
    pub search: TableSearch,
    /// Offsets of the tables found; two when the search found several, the
    /// first two of them.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<u32>", length(max = 2))
    )]
    pub offsets: heapless::Vec<u32, 2>,
    /// The table's entries; `null` when the firmware doesn't know which table
    /// the bootloader read.
    pub table: Option<TableRead>,
}

/// How the firmware found the table the bootloader read. Later versions may
/// add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TableSearch {
    /// The table at the compiled offset matches the layout, and the
    /// bootloader is known to read it: nothing was searched.
    Compiled,
    /// One table below the running app has an app entry where it booted.
    Found,
    /// Several do, so which one the bootloader read is unknown.
    Several,
    /// None does.
    None,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// The entries of a partition table, as the flash holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableRead {
    /// Offset of the table.
    pub offset: u32,
    /// Its partition entries, in order, up to [`TABLE_ENTRIES_MAX`].
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::vec::Vec<TableEntry>", length(max = TABLE_ENTRIES_MAX))
    )]
    pub entries: heapless::Vec<TableEntry, TABLE_ENTRIES_MAX>,
    /// Whether it has more partition entries than are listed.
    pub truncated: bool,
    /// How its entries end.
    pub end: TableEnd,
}

/// A partition entry of a table on the flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TableEntry {
    /// The label, as text read from the flash.
    #[cfg_attr(
        feature = "schema",
        schemars(with = "alloc::string::String", length(max = LABEL_MAX_LEN))
    )]
    pub label: Label,
    /// The type: 0 for an app, 1 for data.
    #[serde(rename = "type")]
    pub kind: u8,
    /// The subtype, which depends on the type.
    pub subtype: u8,
    /// Offset in the flash.
    pub offset: u32,
    /// Size in bytes.
    pub size: u32,
    /// The flag bits; bit 1 is read-only.
    pub flags: u32,
}

/// How the entries of a partition table end. Later versions may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TableEnd {
    /// At an MD5 entry, as ESP-IDF writes tables. The MD5 isn't checked.
    Md5,
    /// At an erased entry.
    Erased,
    /// At an entry that is neither a partition, the MD5 entry nor erased.
    Malformed,
    /// Not within the table's longest length.
    Unterminated,
    /// At an entry that didn't read.
    ReadFailed,
    /// A value this version of the crate does not know.
    #[serde(other)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    Unknown,
}

/// A SHA-256 digest, written as 64 lowercase hexadecimal digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sha256(pub [u8; 32]);

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0; 64];
        for (pair, byte) in text.as_chunks_mut::<2>().0.iter_mut().zip(self.0) {
            pair[0] = DIGITS[usize::from(byte >> 4)];
            pair[1] = DIGITS[usize::from(byte & 0xF)];
        }
        // Every byte written is an ASCII digit.
        let text = core::str::from_utf8(&text).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(text)
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Sha256Visitor;

        impl Visitor<'_> for Sha256Visitor {
            type Value = Sha256;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a SHA-256 digest as 64 lowercase hexadecimal digits")
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Sha256, E> {
                let digit = |c: u8| match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    _ => None,
                };
                let bytes = s.as_bytes();
                if bytes.len() != 64 {
                    return Err(E::invalid_length(bytes.len(), &self));
                }
                let mut digest = [0; 32];
                for (byte, pair) in digest.iter_mut().zip(bytes.as_chunks::<2>().0) {
                    let (Some(high), Some(low)) = (digit(pair[0]), digit(pair[1])) else {
                        return Err(E::invalid_value(de::Unexpected::Str(s), &self));
                    };
                    *byte = high << 4 | low;
                }
                Ok(Sha256(digest))
            }
        }

        deserializer.deserialize_str(Sha256Visitor)
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for Sha256 {
    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "Sha256".into()
    }

    fn schema_id() -> alloc::borrow::Cow<'static, str> {
        concat!(module_path!(), "::Sha256").into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "A SHA-256 digest, as 64 lowercase hexadecimal digits.",
            "pattern": "^[0-9a-f]{64}$",
        })
    }
}

// Every error a download answers with has a status.
const _: () = {
    let mut i = 0;
    while i < RegionContentError::CODES.len() {
        assert!(RegionContentError::CODES[i].status().is_some());
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::super::endpoint::path_params;
    use super::*;

    #[test]
    fn regions_are_path_parameters() {
        let parse = |path| path_params::<Region>(GetRegionContent::PATH, path);
        assert_eq!(
            parse("/storage/internal/regions/app_current/content"),
            Some(Region::AppCurrent)
        );
        assert_eq!(
            parse("/storage/internal/regions/partition_table/content"),
            Some(Region::PartitionTable)
        );
        for path in [
            "/storage/internal/regions/unknown/content",
            "/storage/internal/regions/ota_0/content",
            "/storage/internal/regions/AppCurrent/content",
            "/storage/internal/regions//content",
            "/storage/internal/regions/app_current",
        ] {
            assert_eq!(parse(path), None, "{path}");
        }
    }

    #[test]
    fn download_errors_have_their_statuses() {
        assert_eq!(RegionContentError::Sensitive.code().status(), Some(403));
        assert_eq!(RegionContentError::Unavailable.code().status(), Some(409));
        assert_eq!(RegionContentError::Busy.code().status(), Some(503));
    }

    #[test]
    fn digests_are_lowercase_hex() {
        let mut bytes = [0; 32];
        bytes[0] = 0xAB;
        bytes[31] = 0x0F;
        let digest = Sha256(bytes);
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, format!("\"ab{}0f\"", "0".repeat(60)));
        assert_eq!(digest.to_string(), json.trim_matches('"'));
        assert_eq!(serde_json::from_str::<Sha256>(&json).unwrap(), digest);
        for bad in [
            format!("\"AB{}0f\"", "0".repeat(60)),
            format!("\"{}\"", "0".repeat(63)),
            format!("\"{}\"", "0".repeat(65)),
            format!("\"{}g\"", "0".repeat(63)),
        ] {
            assert!(serde_json::from_str::<Sha256>(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn names_are_snake_case_and_slots_keep_their_labels() {
        assert_eq!(
            serde_json::to_string(&Region::ConfigAlternate).unwrap(),
            r#""config_alternate""#
        );
        assert_eq!(
            serde_json::to_string(&SlotName::Ota1).unwrap(),
            r#""ota_1""#
        );
        assert_eq!(
            serde_json::to_string(&Limit::BootloaderUnlisted).unwrap(),
            r#""bootloader_unlisted""#
        );
        assert_eq!(
            serde_json::to_string(&StorageStatus::NotSupported).unwrap(),
            r#""not_supported""#
        );
    }

    #[test]
    fn unknown_values_parse() {
        let region: Region = serde_json::from_str(r#""nand_logs""#).unwrap();
        assert_eq!(region, Region::Unknown);
        let slot: SlotName = serde_json::from_str(r#""ota_2""#).unwrap();
        assert_eq!(slot, SlotName::Unknown);
        let tier: Tier = serde_json::from_str(r#""sealed""#).unwrap();
        assert_eq!(tier, Tier::Unknown);
    }

    #[test]
    fn partition_kinds_name_their_type_field() {
        let kind = PartitionKind {
            kind: 0,
            subtype: 0x10,
            flags: 0,
        };
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#"{"type":0,"subtype":16,"flags":0}"#);
        assert_eq!(serde_json::from_str::<PartitionKind>(&json).unwrap(), kind);
    }

    #[cfg(feature = "schema")]
    mod schema {
        use serde_json::json;

        use super::*;
        use crate::schema_accepts;

        fn devices() -> StorageDevices {
            let mut devices = heapless::Vec::new();
            for device in [
                StorageDevice {
                    id: StorageId::Internal,
                    kind: StorageKind::NorFlash,
                    status: StorageStatus::Available,
                    capacity: Some(8 << 20),
                },
                StorageDevice {
                    id: StorageId::External,
                    kind: StorageKind::NandFlash,
                    status: StorageStatus::NotSupported,
                    capacity: None,
                },
            ] {
                devices.push(device).unwrap();
            }
            StorageDevices { devices }
        }

        fn text(s: &str) -> FlashText {
            s.try_into().unwrap()
        }

        fn internal_flash() -> InternalFlash {
            let mut otadata = heapless::Vec::new();
            otadata
                .push(OtadataEntry {
                    sector: 0,
                    validity: EntryValidity::Valid,
                    seq: Some(3),
                    state: Some(OtaState::Valid),
                    selects: Some(SlotName::Ota0),
                })
                .unwrap();
            otadata
                .push(OtadataEntry {
                    sector: 1,
                    validity: EntryValidity::Blank,
                    seq: None,
                    state: None,
                    selects: None,
                })
                .unwrap();
            let mut apps = heapless::Vec::new();
            apps.push(AppSlot {
                region: Region::AppCurrent,
                slot: Some(SlotName::Ota0),
                offset: 0x1_0000,
                size: 0x30_0000,
                image: AppImage {
                    status: ImageStatus::Valid,
                    checked_uptime_s: Some(0),
                    error: None,
                    size: Some(260_592),
                    digest: Some(Sha256([1; 32])),
                    file_sha256: Some(Sha256([2; 32])),
                    descriptor: Some(AppDescriptor {
                        project_name: text("eveningstar"),
                        version: text("0.1.0"),
                        date: text("Nov  2 2026"),
                        time: text("10:12:44"),
                        idf_version: text("esp-hal"),
                        secure_version: 0,
                        elf_sha256: Sha256([3; 32]),
                    }),
                },
            })
            .unwrap();
            apps.push(AppSlot {
                region: Region::AppAlternate,
                slot: Some(SlotName::Ota1),
                offset: 0x31_0000,
                size: 0x30_0000,
                image: AppImage {
                    status: ImageStatus::Empty,
                    checked_uptime_s: Some(0),
                    error: None,
                    size: None,
                    digest: None,
                    file_sha256: None,
                    descriptor: None,
                },
            })
            .unwrap();
            InternalFlash {
                capacity: 8 << 20,
                layout_size: 8 << 20,
                tier: Tier::Full,
                limit: None,
                persistent_config: true,
                updates: true,
                booted_offset: Some(0x1_0000),
                table: TableSummary {
                    offset: 0x8000,
                    matches: true,
                    board_offset: Some(0x8000),
                },
                bootloader: BootloaderInfo {
                    status: BootloaderStatus::Known,
                    name: Some("esp-idf-v5.5.1-rollback".try_into().unwrap()),
                    error: None,
                    size: Some(22_528),
                    digest: Some(Sha256([4; 32])),
                    file_sha256: Some(Sha256([5; 32])),
                    version: Some(1),
                    idf_version: Some(text("v5.5.1")),
                    built: Some(text("Oct 30 2026 12:00:00")),
                },
                boot: Some(BootState {
                    running: SlotName::Ota0,
                    selected: Some(SlotName::Ota0),
                    state: Some(OtaState::Valid),
                    rollback_target: false,
                    needs_confirmation: false,
                }),
                otadata,
                apps,
                checks: Checks {
                    status: CheckStatus::Done,
                    finished_uptime_s: Some(0),
                },
            }
        }

        fn regions() -> Regions {
            let mut regions = heapless::Vec::new();
            regions
                .push(RegionInfo {
                    region: Region::Bootloader,
                    label: None,
                    partition: None,
                    offset: 0,
                    size: 0x8000,
                    contents: Contents::Public,
                    available: true,
                    layout: None,
                    roles: heapless::Vec::new(),
                })
                .unwrap();
            let mut roles = heapless::Vec::new();
            roles.push(Role::Running).unwrap();
            roles.push(Role::Selected).unwrap();
            let mut fields = heapless::Vec::new();
            fields.push(EntryField::Offset).unwrap();
            regions
                .push(RegionInfo {
                    region: Region::AppCurrent,
                    label: Some("ota_0".try_into().unwrap()),
                    partition: Some(PartitionKind {
                        kind: 0,
                        subtype: 0x10,
                        flags: 0,
                    }),
                    offset: 0x1_0000,
                    size: 0x30_0000,
                    contents: Contents::Public,
                    available: true,
                    layout: Some(LayoutMatch {
                        status: MatchStatus::Differs,
                        fields,
                    }),
                    roles,
                })
                .unwrap();
            Regions { regions }
        }

        fn board_table() -> BoardTable {
            let mut entries = heapless::Vec::new();
            entries
                .push(TableEntry {
                    label: "factory".try_into().unwrap(),
                    kind: 1,
                    subtype: 6,
                    offset: 0x9000,
                    size: 0x1000,
                    flags: 2,
                })
                .unwrap();
            let mut offsets = heapless::Vec::new();
            offsets.push(0x8000).unwrap();
            BoardTable {
                search: TableSearch::Compiled,
                offsets,
                table: Some(TableRead {
                    offset: 0x8000,
                    entries,
                    truncated: false,
                    end: TableEnd::Md5,
                }),
            }
        }

        #[test]
        fn serialised_values_match_their_schemas() {
            assert!(schema_accepts::<StorageDevices>(
                &serde_json::to_value(devices()).unwrap()
            ));
            assert!(schema_accepts::<InternalFlash>(
                &serde_json::to_value(internal_flash()).unwrap()
            ));
            assert!(schema_accepts::<Regions>(
                &serde_json::to_value(regions()).unwrap()
            ));
            assert!(schema_accepts::<BoardTable>(
                &serde_json::to_value(board_table()).unwrap()
            ));
        }

        #[test]
        fn values_round_trip() {
            let flash = internal_flash();
            let json = serde_json::to_string(&flash).unwrap();
            assert_eq!(serde_json::from_str::<InternalFlash>(&json).unwrap(), flash);
            let regions = regions();
            let json = serde_json::to_string(&regions).unwrap();
            assert_eq!(serde_json::from_str::<Regions>(&json).unwrap(), regions);
            let table = board_table();
            let json = serde_json::to_string(&table).unwrap();
            assert_eq!(serde_json::from_str::<BoardTable>(&json).unwrap(), table);
        }

        #[test]
        fn schemas_reject_invalid_values() {
            let mut value = serde_json::to_value(internal_flash()).unwrap();
            value["bootloader"]["digest"] = json!("AB".repeat(32));
            assert!(!schema_accepts::<InternalFlash>(&value));
            let mut value = serde_json::to_value(regions()).unwrap();
            value["regions"][0]["region"] = json!("unknown");
            assert!(!schema_accepts::<Regions>(&value));
            value["regions"][0]["region"] = json!("bootloader");
            value["regions"][0]["label"] = json!("a".repeat(LABEL_MAX_LEN + 1));
            assert!(!schema_accepts::<Regions>(&value));
        }
    }
}
