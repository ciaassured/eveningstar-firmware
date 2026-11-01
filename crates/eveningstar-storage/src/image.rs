//! ESP images: the app and bootloader images the ROM and the ESP-IDF
//! bootloader load, checked the way they check them, and the descriptors in
//! them.
//!
//! An image is a 24-byte header, up to [`MAX_SEGMENTS`] segments (an 8-byte
//! header of load address and length, then the data), padding up to a 16-byte
//! boundary whose last byte is the checksum, and, when the header says so, the
//! SHA-256 of everything before it. The checksum is the XOR of every segment
//! data byte, seeded with `0xEF`. The first segment's data starts with the
//! image's descriptor: `esp_app_desc_t` in an app, `esp_bootloader_desc_t` in
//! a bootloader.
//!
//! The source is the partition: offsets are relative to its start, and the
//! image has to lie inside it. The partition is taken to start on a 64 KiB
//! boundary, as app partitions and the bootloader at 0x0 do, for the check of
//! where mapped segments lie.
//!
//! The chip revision range in the header isn't checked against the chip, and
//! neither are the load addresses of segments copied to RAM.

use core::fmt;

use sha2::Sha256;

use crate::hash::Sha256Hasher;
use crate::pace::{Pace, Yield};
use crate::source::{Source, read_array};
use crate::{ReadError, Text};

/// The first byte of an image.
pub const MAGIC: u8 = 0xE9;

/// The chip ID of the ESP32-C6, which every image has to name.
pub const CHIP_ID: u16 = 0x000D;

/// The most segments an image can have.
pub const MAX_SEGMENTS: u8 = 16;

/// Length of the image header in bytes.
pub const HEADER_LEN: u32 = 24;

/// Length of a segment header in bytes.
pub const SEGMENT_HEADER_LEN: u32 = 8;

/// Length of the appended SHA-256 digest in bytes.
pub const DIGEST_LEN: u32 = 32;

/// The number of bytes hashed between two yields to the executor.
pub const YIELD_INTERVAL: u32 = 1024;

/// The flash MMU's page size: a segment mapped from flash has to lie at the
/// same offset within a page in the flash as in the address space.
pub const MMU_PAGE_SIZE: u32 = 0x1_0000;

/// The address range the flash MMU maps (`SOC_IROM_LOW` to `SOC_IROM_HIGH` on
/// the ESP32-C6, 256 pages); segments loaded there are mapped, not copied.
const MAPPED: core::ops::Range<u32> = 0x4200_0000..0x4300_0000;

const CHECKSUM_SEED: u8 = 0xEF;
const CHUNK_LEN: usize = 256;

const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const APP_DESC_LEN: u32 = 256;
/// The part of `esp_app_desc_t` read: up to the ELF's SHA-256.
const APP_DESC_READ: usize = 176;

const BOOTLOADER_DESC_MAGIC: u8 = 0x50;
const BOOTLOADER_DESC_LEN: u32 = 80;
/// The part of `esp_bootloader_desc_t` read: up to the end of the date and
/// time.
const BOOTLOADER_DESC_READ: usize = 64;

/// Why an image doesn't verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Reading the source failed.
    Read(ReadError),
    /// The source ends before the header, the checksum or the digest does.
    PastEnd,
    /// The first byte isn't [`MAGIC`].
    Magic(u8),
    /// The header names a chip other than [`CHIP_ID`].
    Chip(u16),
    /// The header names more than [`MAX_SEGMENTS`] segments.
    TooManySegments(u8),
    /// A segment's length isn't a multiple of 4.
    SegmentLength {
        /// Position of the segment.
        index: u8,
        /// The length its header gives.
        len: u32,
    },
    /// A segment's header or data runs past the end of the source.
    SegmentPastEnd {
        /// Position of the segment.
        index: u8,
    },
    /// A segment mapped from flash lies at another offset within an MMU page
    /// in the flash than at its load address.
    SegmentMapping {
        /// Position of the segment.
        index: u8,
    },
    /// The checksum stored doesn't match the segment data.
    Checksum {
        /// The checksum in the image.
        stored: u8,
        /// The checksum of the segment data.
        computed: u8,
    },
    /// The header says no SHA-256 digest is appended.
    NoDigest,
    /// The appended SHA-256 digest doesn't match the image.
    DigestMismatch,
    /// The first segment doesn't start with an app descriptor.
    MissingDescriptor,
}

impl From<ReadError> for ImageError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

/// The image header, with the fields ESP-IDF's `esp_image_header_t` has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The number of segments.
    pub segment_count: u8,
    /// SPI flash mode.
    pub spi_mode: u8,
    /// SPI flash speed: the low four bits of the fourth byte.
    pub spi_speed: u8,
    /// SPI flash size: the high four bits of the fourth byte.
    pub spi_size: u8,
    /// Entry point address.
    pub entry: u32,
    /// The WP pin for quad I/O, or `0xEE` when unused.
    pub wp_pin: u8,
    /// Drive strengths of the SPI flash pins.
    pub spi_pin_drv: [u8; 3],
    /// The chip the image is for.
    pub chip_id: u16,
    /// Minimum chip revision, the major number only (deprecated by ESP-IDF).
    pub min_chip_rev: u8,
    /// Minimum chip revision, as `major * 100 + minor`.
    pub min_chip_rev_full: u16,
    /// Maximum chip revision, as `major * 100 + minor`.
    pub max_chip_rev_full: u16,
    /// Whether a SHA-256 digest follows the checksum.
    pub hash_appended: bool,
}

impl Header {
    /// The header stored in `bytes`, checked as the bootloader checks it: the
    /// magic, the chip and the number of segments.
    ///
    /// # Errors
    ///
    /// [`ImageError::Magic`], [`ImageError::Chip`] or
    /// [`ImageError::TooManySegments`].
    pub fn parse(bytes: &[u8; HEADER_LEN as usize]) -> Result<Self, ImageError> {
        let [
            magic,
            segment_count,
            spi_mode,
            speed_size,
            e0,
            e1,
            e2,
            e3,
            wp_pin,
            d0,
            d1,
            d2,
            c0,
            c1,
            min_chip_rev,
            min0,
            min1,
            max0,
            max1,
            _,
            _,
            _,
            _,
            hash_appended,
        ] = *bytes;
        if magic != MAGIC {
            return Err(ImageError::Magic(magic));
        }
        let chip_id = u16::from_le_bytes([c0, c1]);
        if chip_id != CHIP_ID {
            return Err(ImageError::Chip(chip_id));
        }
        if segment_count > MAX_SEGMENTS {
            return Err(ImageError::TooManySegments(segment_count));
        }
        Ok(Self {
            segment_count,
            spi_mode,
            spi_speed: speed_size & 0x0F,
            spi_size: speed_size >> 4,
            entry: u32::from_le_bytes([e0, e1, e2, e3]),
            wp_pin,
            spi_pin_drv: [d0, d1, d2],
            chip_id,
            min_chip_rev,
            min_chip_rev_full: u16::from_le_bytes([min0, min1]),
            max_chip_rev_full: u16::from_le_bytes([max0, max1]),
            hash_appended: hash_appended != 0,
        })
    }
}

/// An app's descriptor, `esp_app_desc_t`, from the start of its first segment:
/// its first 176 bytes, up to the ELF's SHA-256, as stored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AppDescriptor([u8; APP_DESC_READ]);

impl AppDescriptor {
    /// The version the anti-rollback feature compares.
    #[must_use]
    pub fn secure_version(&self) -> u32 {
        u32::from_le_bytes(field(&self.0, 4))
    }

    /// The app's version.
    #[must_use]
    pub fn version(&self) -> Text<32> {
        Text::from_raw(field(&self.0, 16))
    }

    /// The project's name.
    #[must_use]
    pub fn project_name(&self) -> Text<32> {
        Text::from_raw(field(&self.0, 48))
    }

    /// Build time.
    #[must_use]
    pub fn time(&self) -> Text<16> {
        Text::from_raw(field(&self.0, 80))
    }

    /// Build date.
    #[must_use]
    pub fn date(&self) -> Text<16> {
        Text::from_raw(field(&self.0, 96))
    }

    /// The ESP-IDF version, or what the build put in its place.
    #[must_use]
    pub fn idf_version(&self) -> Text<32> {
        Text::from_raw(field(&self.0, 112))
    }

    /// SHA-256 of the ELF the image was made from.
    #[must_use]
    pub fn elf_sha256(&self) -> [u8; 32] {
        field(&self.0, 144)
    }

    /// Whether a first segment `segment_len` bytes long that starts with
    /// `start` holds an app descriptor.
    fn marks(start: [u8; 4], segment_len: u32) -> bool {
        segment_len >= APP_DESC_LEN && u32::from_le_bytes(start) == APP_DESC_MAGIC
    }
}

impl fmt::Debug for AppDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppDescriptor")
            .field("secure_version", &self.secure_version())
            .field("version", &self.version())
            .field("project_name", &self.project_name())
            .field("time", &self.time())
            .field("date", &self.date())
            .field("idf_version", &self.idf_version())
            .field("elf_sha256", &self.elf_sha256())
            .finish()
    }
}

/// A bootloader's descriptor, `esp_bootloader_desc_t`, from the start of its
/// first segment: its first 64 bytes, up to the build date and time, as
/// stored.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BootloaderDescriptor([u8; BOOTLOADER_DESC_READ]);

impl BootloaderDescriptor {
    /// The version the bootloader's anti-rollback feature compares.
    #[must_use]
    pub fn secure_version(&self) -> u8 {
        let [_, _, _, secure_version] = field(&self.0, 0);
        secure_version
    }

    /// The bootloader's version.
    #[must_use]
    pub fn version(&self) -> u32 {
        u32::from_le_bytes(field(&self.0, 4))
    }

    /// The ESP-IDF version it was built with.
    #[must_use]
    pub fn idf_version(&self) -> Text<32> {
        Text::from_raw(field(&self.0, 8))
    }

    /// Build date and time, empty when the build leaves them out.
    #[must_use]
    pub fn date_time(&self) -> Text<24> {
        Text::from_raw(field(&self.0, 40))
    }

    /// The descriptor at the start of `bytes`, the start of a first segment
    /// `segment_len` bytes long, if it holds one.
    fn parse(bytes: &[u8], segment_len: u32) -> Option<Self> {
        let raw: [u8; BOOTLOADER_DESC_READ] = *bytes.first_chunk()?;
        let [magic, ..] = raw;
        (segment_len >= BOOTLOADER_DESC_LEN && magic == BOOTLOADER_DESC_MAGIC).then_some(Self(raw))
    }
}

impl fmt::Debug for BootloaderDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootloaderDescriptor")
            .field("secure_version", &self.secure_version())
            .field("version", &self.version())
            .field("idf_version", &self.idf_version())
            .field("date_time", &self.date_time())
            .finish()
    }
}

/// The `N` bytes at `at` in `bytes`; zeros where `bytes` is too short, which
/// the fixed-size descriptors never are.
fn field<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes
        .get(at..)
        .and_then(<[u8]>::first_chunk)
        .copied()
        .unwrap_or([0; N])
}

/// The length and the two SHA-256 digests of an image that verified, from one
/// pass over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Digests {
    /// The image's length in bytes, the digest included.
    size: u32,
    /// The digest the image stores, of every byte before it.
    stored: [u8; 32],
    /// The digest of the whole image, the stored digest included.
    file: [u8; 32],
}

/// An app image's header and length, read without hashing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppInfo {
    header: Header,
    size: u32,
}

impl AppInfo {
    /// The image header.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// The image's length in bytes: header, segments, padding with the
    /// checksum and, when the header says one is appended, the digest.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.size
    }
}

/// An app image that verified. Its descriptor, which the verification found,
/// is read with [`app_descriptor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    header: Header,
    digests: Digests,
}

impl App {
    /// The image header.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// The image's length in bytes, the digest included.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.digests.size
    }

    /// The image's SHA-256 digest, which the image stores and which matches
    /// it: the digest of every byte before it.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digests.stored
    }

    /// The SHA-256 of the whole image, the stored digest included: what
    /// `sha256sum` prints for a file holding the image alone.
    #[must_use]
    pub const fn file_sha256(&self) -> &[u8; 32] {
        &self.digests.file
    }
}

/// A bootloader image that verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bootloader {
    header: Header,
    digests: Digests,
    descriptor: Option<BootloaderDescriptor>,
}

impl Bootloader {
    /// The image header.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// The image's length in bytes, the digest included.
    #[must_use]
    pub const fn size(&self) -> u32 {
        self.digests.size
    }

    /// The image's SHA-256 digest, which the image stores and which matches
    /// it: the digest of every byte before it.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digests.stored
    }

    /// The SHA-256 of the whole image, the stored digest included: what
    /// `sha256sum` prints for a file holding the image alone.
    #[must_use]
    pub const fn file_sha256(&self) -> &[u8; 32] {
        &self.digests.file
    }

    /// The bootloader descriptor, or `None` for a bootloader without one, as
    /// ESP-IDF releases before 5.3 build them.
    #[must_use]
    pub const fn descriptor(&self) -> Option<&BootloaderDescriptor> {
        self.descriptor.as_ref()
    }
}

/// Reads an app image's header and length, checking the header, the segment
/// headers, that the first segment starts with an app descriptor and that the
/// image fits the source, but neither the checksum nor the digest.
///
/// # Errors
///
/// [`ImageError::Read`] when reading fails, and the errors of
/// [`verify_app`] other than those of the checksum and the digest.
pub fn inspect_app<S: Source + ?Sized>(source: &S) -> Result<AppInfo, ImageError> {
    let (header, _) = read_header(source)?;
    let mut at = HEADER_LEN;
    for index in 0..header.segment_count {
        let (segment, _) = read_segment(source, at, index)?;
        if index == 0 {
            if segment.len < APP_DESC_LEN {
                return Err(ImageError::MissingDescriptor);
            }
            let start: [u8; 4] = read_array(source, segment.data)?;
            if !AppDescriptor::marks(start, segment.len) {
                return Err(ImageError::MissingDescriptor);
            }
        }
        at = segment.end;
    }
    if header.segment_count == 0 {
        return Err(ImageError::MissingDescriptor);
    }
    let size = tail(source, at, header.hash_appended)?.size;
    Ok(AppInfo { header, size })
}

/// Reads the descriptor of the app image in `source`, checking the header and
/// the first segment's header, but nothing after it.
///
/// # Errors
///
/// [`ImageError::Read`] when reading fails, the errors of the header and the
/// first segment's header, and [`ImageError::MissingDescriptor`].
pub fn app_descriptor<S: Source + ?Sized>(source: &S) -> Result<AppDescriptor, ImageError> {
    let segment = first_segment(source)?;
    if segment.len < APP_DESC_LEN {
        return Err(ImageError::MissingDescriptor);
    }
    // Read in place: the descriptor is the largest value on the stack.
    let mut descriptor = AppDescriptor([0; APP_DESC_READ]);
    source.read(segment.data, &mut descriptor.0)?;
    if !AppDescriptor::marks(field(&descriptor.0, 0), segment.len) {
        return Err(ImageError::MissingDescriptor);
    }
    Ok(descriptor)
}

/// Reads the descriptor of the bootloader image in `source`, checking the
/// header and the first segment's header, but nothing after it: `None` for a
/// bootloader without one, as ESP-IDF releases before 5.3 build them, as
/// [`Bootloader::descriptor`] gives it.
///
/// # Errors
///
/// [`ImageError::Read`] when reading fails, and the errors of the header and
/// the first segment's header.
pub fn bootloader_descriptor<S: Source + ?Sized>(
    source: &S,
) -> Result<Option<BootloaderDescriptor>, ImageError> {
    let (header, _) = read_header(source)?;
    if header.segment_count == 0 {
        return Ok(None);
    }
    let (segment, _) = read_segment(source, HEADER_LEN, 0)?;
    if segment.len < BOOTLOADER_DESC_LEN {
        return Ok(None);
    }
    let bytes: [u8; BOOTLOADER_DESC_READ] = read_array(source, segment.data)?;
    Ok(BootloaderDescriptor::parse(&bytes, segment.len))
}

/// The first segment of the image in `source`, after checking the header and
/// the segment's header.
fn first_segment<S: Source + ?Sized>(source: &S) -> Result<Segment, ImageError> {
    let (header, _) = read_header(source)?;
    if header.segment_count == 0 {
        return Err(ImageError::MissingDescriptor);
    }
    Ok(read_segment(source, HEADER_LEN, 0)?.0)
}

/// Verifies the app image in `source`: the header, every segment, the
/// checksum, the appended SHA-256 digest (which has to be there) and the app
/// descriptor. Hashes in software, and yields to the executor every
/// [`YIELD_INTERVAL`] bytes hashed.
///
/// # Errors
///
/// The first check that fails, as an [`ImageError`].
pub async fn verify_app<S: Source + ?Sized>(source: &S) -> Result<App, ImageError> {
    verify_app_with(source, Sha256::default(), &mut Yield).await
}

/// Verifies the app image in `source` as [`verify_app`] does, hashing with
/// `sha`, a hash not yet fed, and running `pace` after every
/// [`YIELD_INTERVAL`] bytes hashed.
///
/// # Errors
///
/// The first check that fails, as an [`ImageError`].
pub async fn verify_app_with<S, H, P>(source: &S, sha: H, pace: &mut P) -> Result<App, ImageError>
where
    S: Source + ?Sized,
    H: Sha256Hasher,
    P: Pace,
{
    let mut verifier = Verifier::new(source, Kind::App, sha);
    while !verifier.step()? {
        pace.pause().await;
    }
    verifier.app()
}

/// Verifies the bootloader image in `source`: the header, every segment, the
/// checksum and the appended SHA-256 digest (which has to be there), and reads
/// its descriptor when it has one. Hashes in software, and yields to the
/// executor every [`YIELD_INTERVAL`] bytes hashed.
///
/// # Errors
///
/// The first check that fails, as an [`ImageError`].
pub async fn verify_bootloader<S: Source + ?Sized>(source: &S) -> Result<Bootloader, ImageError> {
    let mut verifier = Verifier::new(source, Kind::Bootloader, Sha256::default());
    while !verifier.step()? {
        Yield.pause().await;
    }
    verifier.bootloader()
}

/// A segment, from its header.
struct Segment {
    /// Offset of the data in the source.
    data: u32,
    len: u32,
    /// Offset of the first byte after the data.
    end: u32,
}

/// Where an image ends, from the end of its last segment.
struct Tail {
    /// Length of the image up to the digest.
    unhashed_end: u32,
    /// Length of the image, the digest included when there is one.
    size: u32,
}

fn read_header<S: Source + ?Sized>(
    source: &S,
) -> Result<(Header, [u8; HEADER_LEN as usize]), ImageError> {
    if source.size() < HEADER_LEN {
        return Err(ImageError::PastEnd);
    }
    let bytes = read_array(source, 0)?;
    Ok((Header::parse(&bytes)?, bytes))
}

/// Reads and checks the header of segment `index` at `at`, and returns it with
/// its bytes.
fn read_segment<S: Source + ?Sized>(
    source: &S,
    at: u32,
    index: u8,
) -> Result<(Segment, [u8; SEGMENT_HEADER_LEN as usize]), ImageError> {
    let past_end = ImageError::SegmentPastEnd { index };
    let data = at.checked_add(SEGMENT_HEADER_LEN).ok_or(past_end)?;
    if data > source.size() {
        return Err(past_end);
    }
    let bytes = read_array(source, at)?;
    let [a0, a1, a2, a3, l0, l1, l2, l3] = bytes;
    let load_addr = u32::from_le_bytes([a0, a1, a2, a3]);
    let len = u32::from_le_bytes([l0, l1, l2, l3]);
    if !len.is_multiple_of(4) {
        return Err(ImageError::SegmentLength { index, len });
    }
    let end = data.checked_add(len).ok_or(past_end)?;
    if end > source.size() {
        return Err(past_end);
    }
    if MAPPED.contains(&load_addr) && data % MMU_PAGE_SIZE != load_addr % MMU_PAGE_SIZE {
        return Err(ImageError::SegmentMapping { index });
    }
    Ok((Segment { data, len, end }, bytes))
}

/// Where the image whose segments end at `at` ends, checked to fit the source.
fn tail<S: Source + ?Sized>(source: &S, at: u32, hash_appended: bool) -> Result<Tail, ImageError> {
    // The padding ends where the image's length plus one is a multiple of 16.
    let checksum = at | 0x0F;
    let unhashed_end = checksum.checked_add(1).ok_or(ImageError::PastEnd)?;
    let size = if hash_appended {
        unhashed_end
            .checked_add(DIGEST_LEN)
            .ok_or(ImageError::PastEnd)?
    } else {
        unhashed_end
    };
    if size > source.size() {
        return Err(ImageError::PastEnd);
    }
    Ok(Tail { unhashed_end, size })
}

/// Which descriptor the first segment starts with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    App,
    Bootloader,
}

/// The verification of an image, run in steps of [`YIELD_INTERVAL`] bytes
/// hashed: the header, then each segment's header and data, then the tail.
struct Verifier<'s, S: ?Sized, H> {
    source: &'s S,
    kind: Kind,
    header: Header,
    sha: H,
    hashed: u32,
    yields: u32,
    checksum: u8,
    /// Whether the header has been read.
    started: bool,
    /// Segments read so far.
    segments: u8,
    /// The next byte of segment data to read, and the end of the segment.
    offset: u32,
    end: u32,
    /// The first bytes of the first segment, and the segment's length.
    descriptor: [u8; BOOTLOADER_DESC_READ],
    descriptor_len: u32,
}

impl<'s, S: Source + ?Sized, H: Sha256Hasher> Verifier<'s, S, H> {
    fn new(source: &'s S, kind: Kind, sha: H) -> Self {
        Self {
            source,
            kind,
            header: Header {
                segment_count: 0,
                spi_mode: 0,
                spi_speed: 0,
                spi_size: 0,
                entry: 0,
                wp_pin: 0,
                spi_pin_drv: [0; 3],
                chip_id: 0,
                min_chip_rev: 0,
                min_chip_rev_full: 0,
                max_chip_rev_full: 0,
                hash_appended: false,
            },
            sha,
            hashed: 0,
            yields: 0,
            checksum: CHECKSUM_SEED,
            started: false,
            segments: 0,
            offset: HEADER_LEN,
            end: HEADER_LEN,
            descriptor: [0; BOOTLOADER_DESC_READ],
            descriptor_len: 0,
        }
    }

    /// Hashes `bytes`, and says whether the count of bytes hashed has passed
    /// another multiple of [`YIELD_INTERVAL`].
    fn hash(&mut self, bytes: &[u8]) -> bool {
        self.sha.update(bytes);
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.hashed = self.hashed.saturating_add(len);
        let due = self.hashed / YIELD_INTERVAL;
        let yield_due = due > self.yields;
        self.yields = due;
        yield_due
    }

    /// Reads and hashes until a yield is due (`false`) or every segment has
    /// been read (`true`).
    fn step(&mut self) -> Result<bool, ImageError> {
        if !self.started {
            let (header, bytes) = read_header(self.source)?;
            self.header = header;
            self.started = true;
            if self.hash(&bytes) {
                return Ok(false);
            }
        }
        loop {
            if self.offset == self.end {
                if self.segments == self.header.segment_count {
                    return Ok(true);
                }
                if self.next_segment()? {
                    return Ok(false);
                }
                continue;
            }
            let mut chunk = [0; CHUNK_LEN];
            // At most CHUNK_LEN, so the conversions can't fail.
            let len = usize::try_from(self.end - self.offset)
                .unwrap_or(CHUNK_LEN)
                .min(CHUNK_LEN);
            let chunk = chunk.get_mut(..len).unwrap_or_default();
            self.source.read(self.offset, chunk)?;
            if self.segments == 1 && self.offset == HEADER_LEN + SEGMENT_HEADER_LEN {
                self.first_chunk(chunk)?;
            }
            self.checksum = chunk.iter().fold(self.checksum, |sum, b| sum ^ b);
            self.offset += u32::try_from(len).unwrap_or(u32::MAX);
            if self.hash(chunk) {
                return Ok(false);
            }
        }
    }

    /// Reads and hashes the next segment's header, and says whether a yield is
    /// due.
    fn next_segment(&mut self) -> Result<bool, ImageError> {
        let index = self.segments;
        let (segment, bytes) = read_segment(self.source, self.end, index)?;
        if index == 0 {
            self.descriptor_len = segment.len;
            if self.kind == Kind::App && segment.len < APP_DESC_LEN {
                return Err(ImageError::MissingDescriptor);
            }
        }
        self.segments += 1;
        self.offset = segment.data;
        self.end = segment.end;
        Ok(self.hash(&bytes))
    }

    /// Keeps the start of the first segment, which holds the descriptor, and
    /// checks it for an app.
    fn first_chunk(&mut self, chunk: &[u8]) -> Result<(), ImageError> {
        let kept = self.descriptor.len().min(chunk.len());
        if let (Some(to), Some(from)) = (self.descriptor.get_mut(..kept), chunk.get(..kept)) {
            to.copy_from_slice(from);
        }
        let start = chunk.first_chunk().copied().unwrap_or_default();
        if self.kind == Kind::App && !AppDescriptor::marks(start, self.descriptor_len) {
            return Err(ImageError::MissingDescriptor);
        }
        Ok(())
    }

    /// Finishes the verification of an app.
    fn app(&mut self) -> Result<App, ImageError> {
        let digests = self.finish()?;
        // A first segment's descriptor was checked as it was read.
        if self.segments == 0 {
            return Err(ImageError::MissingDescriptor);
        }
        Ok(App {
            header: self.header,
            digests,
        })
    }

    /// Finishes the verification of a bootloader.
    fn bootloader(&mut self) -> Result<Bootloader, ImageError> {
        let digests = self.finish()?;
        Ok(Bootloader {
            header: self.header,
            digests,
            descriptor: BootloaderDescriptor::parse(&self.descriptor, self.descriptor_len),
        })
    }

    /// Checks the padding, the checksum and the digest after the segments, and
    /// returns the image's size and digests.
    fn finish(&mut self) -> Result<Digests, ImageError> {
        let at = self.end;
        let tail = tail(self.source, at, self.header.hash_appended)?;
        let mut padding = [0; 16];
        // At most 16 bytes: up to and including the checksum.
        let padding_len = usize::try_from(tail.unhashed_end - at).unwrap_or(16);
        let padding = padding.get_mut(..padding_len).unwrap_or_default();
        self.source.read(at, padding)?;
        self.hash(padding);
        let stored = padding.last().copied().ok_or(ImageError::PastEnd)?;
        if stored != self.checksum {
            return Err(ImageError::Checksum {
                stored,
                computed: self.checksum,
            });
        }

        if !self.header.hash_appended {
            return Err(ImageError::NoDigest);
        }
        let stored: [u8; 32] = read_array(self.source, tail.unhashed_end)?;
        let file = finish_digests(&self.sha, &stored).ok_or(ImageError::DigestMismatch)?;
        Ok(Digests {
            size: tail.size,
            stored,
            file,
        })
    }
}

/// Finishes a copy of `sha`, the hash of every byte of an image before its
/// digest, and compares it with `stored`, the digest the image stores.
/// Returns the digest of the whole image when they match: another copy of
/// `sha`, fed `stored`. Never inlined, so the copies stay out of the
/// verifier's frame.
#[inline(never)]
fn finish_digests<H: Sha256Hasher>(sha: &H, stored: &[u8; 32]) -> Option<[u8; 32]> {
    if sha.clone().finalize() != *stored {
        return None;
    }
    let mut file = sha.clone();
    file.update(stored);
    Some(file.finalize())
}

#[cfg(test)]
mod tests;
