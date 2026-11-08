#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use sha2::Digest as _;

use crate::hash::Sha256Hasher;
use crate::pace::{Pace, Yield};

use super::*;
use crate::test_support::{
    APP, BOOTLOADER, ESPFLASH_BOOTLOADER, FLASH_SIZE, ImageBuilder, LAYOUT_RECORD_AT, PARTITIONS,
    Rng, TABLE, TABLE_OFFSET, app_descriptor_bytes, app_image, app_image_with_layout, block_on,
    board_layout_record, bootloader_image, data, layout_record_bytes, run, segment_at, with_layout,
};

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

fn app(source: &[u8]) -> Result<App, ImageError> {
    run(verify_app(source))
}

fn bootloader(source: &[u8]) -> Result<Bootloader, ImageError> {
    run(verify_bootloader(source))
}

/// `image` with `len` erased bytes after it, as a partition holds it.
fn in_partition(image: &[u8], len: usize) -> Vec<u8> {
    let mut partition = vec![0xFF; len];
    partition[..image.len()].copy_from_slice(image);
    partition
}

/// Verifying `image` as a bootloader fails with `error`. When it holds an app
/// descriptor, so does verifying it as an app, and inspecting it unless the
/// error is one only verification finds.
fn rejected(image: &[u8], error: ImageError) {
    assert_eq!(bootloader(image).map(|_| ()), Err(error), "bootloader");
    if image.get(32..36) != Some(&APP_DESC_MAGIC.to_le_bytes()[..]) {
        return;
    }
    assert_eq!(app(image).map(|_| ()), Err(error), "app");
    let verified_only = matches!(
        error,
        ImageError::Checksum { .. } | ImageError::NoDigest | ImageError::DigestMismatch
    );
    if !verified_only {
        assert_eq!(inspect_app(image).map(|_| ()), Err(error), "inspect");
    }
}

// The fixtures.

const NIX_BOOTLOADER_DIGEST: &str =
    "6d53be01af0220ee443c48d3692f8c6939afc534c7c35838aa9678e25b562421";
const ESPFLASH_BOOTLOADER_DIGEST: &str =
    "f11dbe3ab3cbdfe22c670a509d1af9ff391f7e2e23deefeed8879f8277f93aa2";
const APP_DIGEST: &str = "ee6c7951f0c52e956abdd6e22b0104c3736412f971bc3c78f7da36e972514c2d";

// `sha256sum` of each image alone: the files, and the first 22016 bytes of
// the espflash bootloader's.
const NIX_BOOTLOADER_FILE_SHA256: &str =
    "e49b7315251ccf47f23fd93ac064b4509e325715b085d2c236d79fa992eab3d2";
const ESPFLASH_BOOTLOADER_FILE_SHA256: &str =
    "b1d7d018cc62f3fb30b901d5bacf95f58ab879f70ebec50d231f092e976c9318";
const APP_FILE_SHA256: &str = "561b57c28e5a430dd0f1659646afa24f68793036f6a45670aec3c41914877d3b";

#[test]
fn the_nix_bootloader_verifies() {
    let verified = bootloader(BOOTLOADER).unwrap();
    assert_eq!(hex(verified.digest()), NIX_BOOTLOADER_DIGEST);
    assert_eq!(hex(verified.file_sha256()), NIX_BOOTLOADER_FILE_SHA256);
    assert_eq!(verified.size(), 22672);
    assert_eq!(verified.size() as usize, BOOTLOADER.len());
    let header = verified.header();
    assert_eq!(header.segment_count, 3);
    assert_eq!(header.chip_id, CHIP_ID);
    assert!(header.hash_appended);
    let descriptor = verified.descriptor().unwrap();
    assert_eq!(descriptor.version(), 1);
    assert_eq!(descriptor.secure_version(), 0);
    assert_eq!(descriptor.idf_version().to_str(), Some("v5.5.2"));
    assert_eq!(descriptor.date_time().to_str(), Some(""));
    assert_eq!(bootloader_descriptor(BOOTLOADER), Ok(Some(*descriptor)));
}

#[test]
fn the_nix_bootloader_verifies_in_its_partition() {
    // Up to the table at 0x8000; the erased bytes after the image don't count.
    let flash = in_partition(BOOTLOADER, 0x8000);
    let verified = bootloader(&flash).unwrap();
    assert_eq!(hex(verified.digest()), NIX_BOOTLOADER_DIGEST);
    assert_eq!(hex(verified.file_sha256()), NIX_BOOTLOADER_FILE_SHA256);
    assert_eq!(verified.size(), 22672);
}

#[test]
fn the_espflash_bootloader_verifies() {
    let verified = bootloader(ESPFLASH_BOOTLOADER).unwrap();
    assert_eq!(hex(verified.digest()), ESPFLASH_BOOTLOADER_DIGEST);
    // Of the image alone, without the erased bytes after it.
    assert_eq!(hex(verified.file_sha256()), ESPFLASH_BOOTLOADER_FILE_SHA256);
    assert_eq!(verified.size(), 22016);
    assert_eq!(verified.header().segment_count, 3);
    let descriptor = verified.descriptor().unwrap();
    assert_eq!(descriptor.version(), 1);
    assert_eq!(
        descriptor.idf_version().to_str(),
        Some("v6.1-beta1-497-g14f663f003e")
    );
    assert_eq!(
        bootloader_descriptor(ESPFLASH_BOOTLOADER),
        Ok(Some(*descriptor))
    );
    assert_ne!(verified.digest(), bootloader(BOOTLOADER).unwrap().digest());
}

#[test]
fn the_app_verifies() {
    let verified = app(APP).unwrap();
    assert_eq!(hex(verified.digest()), APP_DIGEST);
    assert_eq!(hex(verified.file_sha256()), APP_FILE_SHA256);
    assert_eq!(verified.size() as usize, APP.len());
    let header = verified.header();
    assert_eq!(header.segment_count, 4);
    assert_eq!(header.entry, 0x4201_0072);
    assert_eq!(header.spi_mode, 2);
    assert_eq!((header.spi_size, header.spi_speed), (3, 0));
    assert_eq!(header.wp_pin, 0xEE);
    assert_eq!(header.spi_pin_drv, [0; 3]);
    assert_eq!((header.min_chip_rev, header.min_chip_rev_full), (0, 0));
    assert_eq!(header.max_chip_rev_full, 99);
    let descriptor = app_descriptor(APP).unwrap();
    assert_eq!(descriptor.project_name().to_str(), Some("eveningstar"));
    assert_eq!(descriptor.version().to_str(), Some("0.1.0"));
    assert_eq!(descriptor.secure_version(), 0);
    assert_eq!(descriptor.idf_version().to_str(), Some("0.0.0"));
    assert_eq!(descriptor.date().to_str(), Some("2026-10-24"));
    assert_eq!(descriptor.time().to_str(), Some("07:37:59"));
    assert_eq!(&descriptor.elf_sha256()[..], &APP[32 + 144..32 + 176]);
    assert!(std::format!("{descriptor:?}").contains(r#"project_name: "eveningstar""#));
}

#[test]
fn the_app_verifies_in_its_slot() {
    let slot = in_partition(APP, 0x30_0000);
    let verified = app(&slot).unwrap();
    assert_eq!(hex(verified.digest()), APP_DIGEST);
    assert_eq!(hex(verified.file_sha256()), APP_FILE_SHA256);
    assert_eq!(verified.size() as usize, APP.len());
}

#[test]
fn the_file_digest_covers_the_whole_image_and_nothing_after() {
    for image in [APP, BOOTLOADER, &ESPFLASH_BOOTLOADER[..22016]] {
        let verified = bootloader(&in_partition(image, image.len() + 4096)).unwrap();
        let whole: [u8; 32] = Sha256::digest(image).into();
        let before: [u8; 32] = Sha256::digest(&image[..image.len() - 32]).into();
        assert_eq!(verified.file_sha256(), &whole);
        assert_eq!(verified.digest(), &before);
    }
}

#[test]
fn the_app_inspects_without_hashing() {
    let info = inspect_app(APP).unwrap();
    let verified = app(APP).unwrap();
    assert_eq!(info.header(), verified.header());
    assert_eq!(info.size(), verified.size());

    // A corrupt checksum or digest goes unnoticed.
    let mut corrupt = APP.to_vec();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    corrupt[last - 32] ^= 1;
    assert_eq!(inspect_app(&corrupt[..]), Ok(info));
}

#[test]
fn a_bootloader_isnt_an_app_but_an_app_passes_as_a_bootloader() {
    assert_eq!(app(BOOTLOADER), Err(ImageError::MissingDescriptor));
    assert_eq!(inspect_app(BOOTLOADER), Err(ImageError::MissingDescriptor));
    assert_eq!(
        app_descriptor(BOOTLOADER),
        Err(ImageError::MissingDescriptor)
    );
    // An app's first segment doesn't start with a bootloader descriptor.
    let verified = bootloader(APP).unwrap();
    assert_eq!(hex(verified.digest()), APP_DIGEST);
    assert_eq!(hex(verified.file_sha256()), APP_FILE_SHA256);
    assert_eq!(verified.descriptor(), None);
}

#[test]
fn the_verification_yields_every_1_kib_hashed() {
    let hashed = APP.len() - 32;
    let verify = block_on(verify_app(APP));
    assert_eq!(hex(verify.output.unwrap().digest()), APP_DIGEST);
    assert_eq!(verify.wakes, hashed / YIELD_INTERVAL as usize);
    assert_eq!(verify.polls, verify.wakes + 1);
    assert_eq!(verify.wakes, 212);

    let verify = block_on(verify_bootloader(BOOTLOADER));
    assert!(verify.output.is_ok());
    assert_eq!(
        verify.wakes,
        (BOOTLOADER.len() - 32) / YIELD_INTERVAL as usize
    );
    assert_eq!(verify.polls, verify.wakes + 1);
}

#[test]
fn a_failed_verification_stops_without_further_yields() {
    let mut image = APP.to_vec();
    image[0] = 0;
    let verify = block_on(verify_app(&image[..]));
    assert_eq!(verify.output, Err(ImageError::Magic(0)));
    assert_eq!(verify.polls, 1);
}

// Other hashers and paces.

/// SHA-256 in software, fed one byte at a time: an implementation that
/// buffers its input differently from one fed whole chunks, as a hardware
/// accelerator does.
#[derive(Clone, Default)]
struct Bytewise(Sha256);

impl Sha256Hasher for Bytewise {
    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            Sha256Hasher::update(&mut self.0, core::slice::from_ref(byte));
        }
    }

    fn finalize(self) -> [u8; 32] {
        Sha256Hasher::finalize(self.0)
    }
}

/// Counts its pauses, and pauses without handing control back.
struct Counting(usize);

impl Pace for Counting {
    fn pause(&mut self) -> impl Future<Output = ()> {
        self.0 += 1;
        core::future::ready(())
    }
}

/// Verifying `image` with [`Bytewise`] finds what [`verify_app`] does.
fn same_with_another_hasher(image: &[u8]) {
    let other = run(verify_app_with(image, Bytewise::default(), &mut Yield));
    assert_eq!(other, app(image));
}

#[test]
fn a_verification_hashes_with_the_hasher_and_pauses_with_the_pace_given() {
    let mut pace = Counting(0);
    let verify = block_on(verify_app_with(APP, Bytewise::default(), &mut pace));
    let verified = verify.output.unwrap();
    assert_eq!(hex(verified.digest()), APP_DIGEST);
    assert_eq!(hex(verified.file_sha256()), APP_FILE_SHA256);
    // A pause after every step, and the pace never yielded.
    assert_eq!(pace.0, (APP.len() - 32) / YIELD_INTERVAL as usize);
    assert_eq!((verify.polls, verify.wakes), (1, 0));
}

#[test]
fn a_bootloader_hashes_with_the_hasher_and_pauses_with_the_pace_given() {
    let mut pace = Counting(0);
    let verify = block_on(verify_bootloader_with(
        BOOTLOADER,
        Bytewise::default(),
        &mut pace,
    ));
    let verified = verify.output.unwrap();
    assert_eq!(Ok(verified), run(verify_bootloader(BOOTLOADER)));
    assert_eq!(
        pace.0,
        (verified.size() - DIGEST_LEN) as usize / YIELD_INTERVAL as usize
    );
    assert_eq!((verify.polls, verify.wakes), (1, 0));
}

#[test]
fn every_hasher_finds_the_same() {
    same_with_another_hasher(APP);
    same_with_another_hasher(&in_partition(APP, APP.len() + 4096));
    same_with_another_hasher(BOOTLOADER);
    same_with_another_hasher(&app_image().build());
    for len in (0..64).step_by(4) {
        let mut builder = app_image();
        builder.segments[2].1 = vec![0x5A; len];
        same_with_another_hasher(&builder.build());
    }
    let mut rng = Rng::new(0x5A5);
    let mut image = APP.to_vec();
    for _ in 0..20 {
        let at = rng.below(APP.len());
        image[at] = rng.byte();
        same_with_another_hasher(&image);
        image[at] = APP[at];
    }
}

// Built images.

#[test]
fn built_images_verify() {
    let image = app_image().build();
    let verified = app(&image).unwrap();
    assert_eq!(verified.size() as usize, image.len());
    assert_eq!(&verified.digest()[..], &image[image.len() - 32..]);
    assert_eq!(verified.file_sha256()[..], Sha256::digest(&image)[..]);
    let descriptor = app_descriptor(&image[..]).unwrap();
    assert_eq!(descriptor.project_name().to_str(), Some("eveningstar"));
    assert_eq!(descriptor.version().to_str(), Some("1.2.3"));
    assert_eq!(descriptor.secure_version(), 7);
    assert_eq!(descriptor.time().to_str(), Some("12:34:56"));
    assert_eq!(descriptor.date().to_str(), Some("2026-11-01"));
    assert_eq!(descriptor.idf_version().to_str(), Some("v5.5.2"));
    assert_eq!(descriptor.elf_sha256()[31], 31);
    assert_eq!((image.len() - 32) % 16, 0);

    let image = bootloader_image().build();
    let verified = bootloader(&image).unwrap();
    let descriptor = verified.descriptor().unwrap();
    assert_eq!((descriptor.secure_version(), descriptor.version()), (2, 3));
    assert!(std::format!("{descriptor:?}").contains(r#"idf_version: "v5.5.2""#));
    assert_eq!(descriptor.date_time().to_str(), Some("Nov  1 2026 12:00:0"));
}

#[test]
fn every_padding_length_verifies() {
    // Segment data from 0 to 60 bytes covers every remainder modulo 16 of
    // where the segments end.
    for len in (0..64).step_by(4) {
        let mut builder = app_image();
        builder.segments[2].1 = vec![0x5A; len];
        let image = builder.build();
        assert!(app(&image).is_ok(), "{len}");
        assert_eq!(
            inspect_app(&image[..]).unwrap().size() as usize,
            image.len()
        );
    }
}

#[test]
fn a_bootloader_without_segments_verifies_without_a_descriptor() {
    let image = ImageBuilder::new(vec![]).build();
    assert_eq!(image.len(), 32 + 32);
    let verified = bootloader(&image).unwrap();
    assert_eq!(verified.descriptor(), None);
    assert_eq!(bootloader_descriptor(&image[..]), Ok(None));
    assert_eq!(app(&image), Err(ImageError::MissingDescriptor));
    assert_eq!(inspect_app(&image[..]), Err(ImageError::MissingDescriptor));
}

#[test]
fn a_bootloader_with_a_short_or_unmarked_first_segment_has_no_descriptor() {
    let mut builder = bootloader_image();
    builder.segments[0].1.truncate(76);
    let image = builder.build();
    assert_eq!(bootloader(&image).unwrap().descriptor(), None);
    assert_eq!(bootloader_descriptor(&image[..]), Ok(None));

    let mut builder = bootloader_image();
    builder.segments[0].1[0] = 0x51;
    let image = builder.build();
    assert_eq!(bootloader(&image).unwrap().descriptor(), None);
    assert_eq!(bootloader_descriptor(&image[..]), Ok(None));
}

// Each corruption, with its error.

#[test]
fn a_wrong_magic_is_rejected() {
    for value in [0x00, 0xE8, 0xEA, 0xFF] {
        let mut image = APP.to_vec();
        image[0] = value;
        rejected(&image, ImageError::Magic(value));
    }
    rejected(&[0xFF; 64], ImageError::Magic(0xFF));
}

#[test]
fn another_chip_is_rejected() {
    for chip in [0x0000, 0x0005, 0x000C, 0x0D00, 0xFFFF] {
        let mut image = BOOTLOADER.to_vec();
        image[12..14].copy_from_slice(&u16::to_le_bytes(chip));
        rejected(&image, ImageError::Chip(chip));
    }
}

#[test]
fn too_many_segments_are_rejected() {
    for count in [17, 0x80, 0xFF] {
        let mut image = APP.to_vec();
        image[1] = count;
        rejected(&image, ImageError::TooManySegments(count));
    }
}

#[test]
fn sixteen_segments_are_accepted() {
    let mut rng = Rng::new(16);
    let mut segments = vec![(0x4200_0020, app_descriptor_bytes("eveningstar", "16"))];
    for i in 0..15 {
        segments.push((0x4080_0000 + i * 0x100, data(&mut rng, 4 * i as usize)));
    }
    let image = ImageBuilder::new(segments).build();
    assert_eq!(app(&image).unwrap().header().segment_count, MAX_SEGMENTS);
}

#[test]
fn a_segment_length_not_a_multiple_of_4_is_rejected() {
    for index in 0..4 {
        for delta in 1..4 {
            let mut image = APP.to_vec();
            let at = segment_at(&image, index) + 4;
            let len = u32::from_le_bytes(image[at..at + 4].try_into().unwrap()) - delta;
            image[at..at + 4].copy_from_slice(&len.to_le_bytes());
            rejected(
                &image,
                ImageError::SegmentLength {
                    index: index as u8,
                    len,
                },
            );
        }
    }
}

#[test]
fn a_segment_past_the_end_is_rejected() {
    let image = APP.to_vec();
    for index in 0..4 {
        // The length runs past the image, or overflows the offset.
        for len in [0x0010_0000, 0xFFFF_FFFC] {
            let mut image = image.clone();
            let at = segment_at(&image, index) + 4;
            image[at..at + 4].copy_from_slice(&u32::to_le_bytes(len));
            rejected(&image, ImageError::SegmentPastEnd { index: index as u8 });
        }
        // The source ends in the segment's header or data.
        let at = segment_at(&image, index);
        for end in [at, at + 7, at + 8 + 4] {
            rejected(
                &image[..end],
                ImageError::SegmentPastEnd { index: index as u8 },
            );
        }
    }
    // More segments than the image holds: the next header lies past the end.
    let mut built = app_image().build();
    built[1] = 4;
    let index = 3;
    let at = segment_at(&built, index);
    // The padding and digest happen to be read as a header: cut them off.
    rejected(
        &built[..at],
        ImageError::SegmentPastEnd { index: index as u8 },
    );
}

#[test]
fn a_mapped_segment_off_its_page_offset_is_rejected() {
    let mut builder = app_image();
    builder.segments[0].0 = 0x4200_0024;
    rejected(&builder.build(), ImageError::SegmentMapping { index: 0 });

    // A later mapped segment, which has to start where its address does
    // within a 64 KiB page.
    let mut builder = app_image();
    builder.segments[2].0 = 0x4201_0000;
    rejected(&builder.build(), ImageError::SegmentMapping { index: 2 });
    let image = builder.build();
    let data_at = (segment_at(&image, 2) + 8) as u32;
    builder.segments[2].0 = 0x4201_0000 + data_at % MMU_PAGE_SIZE;
    assert!(app(&builder.build()).is_ok());

    // Outside the mapped range, the address is RAM and isn't checked.
    let mut builder = app_image();
    builder.segments[2].0 = 0x4300_0000;
    assert!(app(&builder.build()).is_ok());
    builder.segments[2].0 = 0x41FF_FFFC;
    assert!(app(&builder.build()).is_ok());
}

/// An app built by `edit`ed [`app_image`] whose third segment is mapped
/// 32 KiB past where a 64 KiB page would put it: in place for a 32 KiB page,
/// not for a 64 KiB one.
fn app_off_a_64_kib_page(mmu_page_size: u8) -> Vec<u8> {
    let mut builder = app_image();
    builder.segments[0].1[180] = mmu_page_size;
    let data_at = (segment_at(&builder.build(), 2) + 8) as u32;
    builder.segments[2].0 = 0x4201_0000 + data_at % 0x8000 + 0x8000;
    builder.build()
}

#[test]
fn mapped_segments_are_checked_with_the_descriptor_s_page_size() {
    // The firmware's image names 64 KiB pages, as 2^16.
    assert_eq!(app_descriptor(APP).unwrap().mmu_page_size(), 16);
    assert_eq!(mmu_page_size(16), Some(MMU_PAGE_SIZE));

    // A 32 KiB page, as 2^15.
    let image = app_off_a_64_kib_page(15);
    assert_eq!(app(&image).unwrap().size() as usize, image.len());
    assert!(inspect_app(&image[..]).is_ok());
    assert_eq!(app_descriptor(&image[..]).unwrap().mmu_page_size(), 15);
    // A bootloader's segments are checked with 64 KiB pages, whatever its
    // first segment holds.
    assert_eq!(
        bootloader(&image).map(|_| ()),
        Err(ImageError::SegmentMapping { index: 2 })
    );

    // 0 and 16 both select 64 KiB.
    for log2 in [0, 16] {
        let image = app_off_a_64_kib_page(log2);
        let error = ImageError::SegmentMapping { index: 2 };
        assert_eq!(app(&image).map(|_| ()), Err(error), "{log2}");
        assert_eq!(inspect_app(&image[..]).map(|_| ()), Err(error), "{log2}");
    }

    // Smaller pages pass whatever lies at the same offset within them.
    for log2 in [1, 2, 13, 14] {
        let image = app_off_a_64_kib_page(log2);
        assert!(app(&image).is_ok(), "{log2}");
    }
}

#[test]
fn the_first_segment_is_checked_with_its_own_page_size() {
    // Mapped 4 KiB past where a page puts it: in place for 4 KiB pages, not
    // for 8 KiB ones.
    let mut builder = app_image();
    builder.segments[0].0 = 0x4200_1020;
    builder.segments[0].1[180] = 12;
    assert!(app(&builder.build()).is_ok());
    builder.segments[0].1[180] = 13;
    let image = builder.build();
    let error = ImageError::SegmentMapping { index: 0 };
    assert_eq!(app(&image).map(|_| ()), Err(error));
    assert_eq!(inspect_app(&image[..]).map(|_| ()), Err(error));
    assert_eq!(app_descriptor(&image[..]).map(|_| ()), Err(error));
}

#[test]
fn a_page_size_past_32_bits_is_rejected() {
    for log2 in [32, 33, 0x80, 0xFF] {
        assert_eq!(mmu_page_size(log2), None);
        let mut builder = app_image();
        builder.segments[0].1[180] = log2;
        let image = builder.build();
        let error = ImageError::MmuPageSize(log2);
        assert_eq!(app(&image).map(|_| ()), Err(error), "{log2}");
        assert_eq!(inspect_app(&image[..]).map(|_| ()), Err(error), "{log2}");
        assert_eq!(app_descriptor(&image[..]).map(|_| ()), Err(error), "{log2}");
    }
    assert_eq!(mmu_page_size(31), Some(0x8000_0000));
    assert_eq!(mmu_page_size(1), Some(2));
}

#[test]
fn the_flash_size_code_gives_the_chip_size() {
    let mib = 0x10_0000;
    let sizes: Vec<u32> = (0..=7).map(flash_size).collect();
    assert_eq!(
        sizes,
        [
            mib,
            2 * mib,
            4 * mib,
            8 * mib,
            16 * mib,
            32 * mib,
            64 * mib,
            128 * mib
        ]
    );
    // An unknown code is 2 MiB.
    for code in 8..=0xFF {
        assert_eq!(flash_size(code), 2 * mib, "{code}");
    }
    // The fixtures name 8 MiB, as `nix/board.nix` sets.
    for image in [BOOTLOADER, ESPFLASH_BOOTLOADER, APP] {
        let header: [u8; 24] = image[..24].try_into().unwrap();
        assert_eq!(flash_size_code(&header), 3);
        assert_eq!(Header::parse(&header).unwrap().spi_size, 3);
    }
    // The code is read from the byte whatever the rest of the header holds.
    let mut header = [0; 24];
    header[3] = 0x5F;
    assert_eq!(flash_size_code(&header), 5);
    assert!(Header::parse(&header).is_err());
}

#[test]
fn a_wrong_checksum_is_rejected() {
    for (image, digest) in [(APP, true), (BOOTLOADER, true)] {
        let mut image = image.to_vec();
        let at = image.len() - 1 - if digest { 32 } else { 0 };
        let computed = image[at];
        image[at] ^= 0x10;
        let stored = image[at];
        rejected(&image, ImageError::Checksum { stored, computed });
    }
    // Segment data that changes the checksum.
    let mut image = app_image().build();
    let at = segment_at(&image, 1) + 8 + 5;
    let computed = image[image.len() - 33] ^ 0x01;
    image[at] ^= 0x01;
    rejected(
        &image,
        ImageError::Checksum {
            stored: image[image.len() - 33],
            computed,
        },
    );
}

#[test]
fn a_missing_digest_is_rejected() {
    let mut builder = app_image();
    builder.header[23] = 0;
    let image = builder.build();
    rejected(&image, ImageError::NoDigest);
    // Inspecting it works, and its length has no digest.
    assert_eq!(
        inspect_app(&image[..]).unwrap().size() as usize,
        image.len()
    );

    // The real app with the flag cleared, digest left in place.
    let mut image = APP.to_vec();
    image[23] = 0;
    rejected(&image, ImageError::NoDigest);
}

#[test]
fn a_wrong_digest_is_rejected() {
    for fixture in [APP, BOOTLOADER] {
        for at in [fixture.len() - 32, fixture.len() - 1] {
            let mut image = fixture.to_vec();
            image[at] ^= 0x01;
            rejected(&image, ImageError::DigestMismatch);
        }
    }
}

#[test]
fn changes_the_checksum_misses_are_caught_by_the_digest() {
    // The header and the padding are hashed but not in the checksum.
    let mut image = APP.to_vec();
    image[2] ^= 0x01;
    rejected(&image, ImageError::DigestMismatch);
    let mut image = app_image().build();
    let padding = image.len() - 34;
    image[padding] ^= 0x01;
    rejected(&image, ImageError::DigestMismatch);
    // Two flips of the same bit leave the XOR unchanged.
    let mut image = APP.to_vec();
    image[1000] ^= 0x04;
    image[2000] ^= 0x04;
    rejected(&image, ImageError::DigestMismatch);
    // A segment's load address is hashed but not in the checksum.
    let mut image = APP.to_vec();
    let at = segment_at(&image, 1);
    image[at] ^= 0x04;
    rejected(&image, ImageError::DigestMismatch);
}

#[test]
fn an_image_longer_than_its_source_is_rejected() {
    for fixture in [APP, BOOTLOADER] {
        let len = fixture.len();
        // Without the digest, part of it, or the checksum.
        for end in [len - 32, len - 1, len - 33] {
            rejected(&fixture[..end], ImageError::PastEnd);
        }
    }
    for end in [0, 1, 23] {
        rejected(&APP[..end], ImageError::PastEnd);
    }
    assert_eq!(
        Header::parse(APP[..24].try_into().unwrap()),
        Ok(*app(APP).unwrap().header())
    );
}

#[test]
fn a_missing_app_descriptor_is_rejected() {
    let missing = |image: &[u8]| {
        assert_eq!(app(image), Err(ImageError::MissingDescriptor));
        assert_eq!(inspect_app(image), Err(ImageError::MissingDescriptor));
        assert_eq!(app_descriptor(image), Err(ImageError::MissingDescriptor));
    };
    // Another magic.
    let mut builder = app_image();
    builder.segments[0].1[0] ^= 0x01;
    missing(&builder.build());
    assert!(bootloader(&builder.build()).is_ok());
    // A first segment shorter than the descriptor.
    let mut builder = app_image();
    builder.segments[0].1.truncate(252);
    missing(&builder.build());
    builder.segments[0].1.clear();
    missing(&builder.build());
    // Just long enough.
    let mut builder = app_image();
    builder.segments[0].1.truncate(256);
    assert!(app(&builder.build()).is_ok());
    assert!(app_descriptor(&builder.build()[..]).is_ok());
    // No segments.
    missing(&ImageBuilder::new(vec![]).build());
    // The real app with its descriptor's magic changed.
    let mut image = APP.to_vec();
    image[32] ^= 0x80;
    missing(&image);
}

#[test]
fn a_short_first_segment_at_the_end_of_the_source_has_no_descriptor() {
    // The source ends with the segment, so reading a descriptor's worth of
    // bytes would run past it.
    let mut builder = app_image();
    builder.segments[0].1.clear();
    let image = builder.build();
    assert_eq!(
        inspect_app(&image[..32]),
        Err(ImageError::MissingDescriptor)
    );
    assert_eq!(
        app_descriptor(&image[..32]),
        Err(ImageError::MissingDescriptor)
    );
    let mut builder = app_image();
    builder.segments[0].1.truncate(100);
    let image = builder.build();
    assert_eq!(
        inspect_app(&image[..132]),
        Err(ImageError::MissingDescriptor)
    );
    assert_eq!(
        app_descriptor(&image[..132]),
        Err(ImageError::MissingDescriptor)
    );
}

#[test]
fn the_descriptor_read_checks_only_the_start_of_the_image() {
    // Corruption after the first segment's header doesn't matter to it.
    let mut image = APP.to_vec();
    let at = segment_at(&image, 1) + 4;
    image[at] ^= 0x01;
    image[1000] ^= 0x01;
    assert!(app(&image).is_err());
    assert_eq!(app_descriptor(&image[..]), app_descriptor(APP));
    // The header and the first segment's header do.
    let mut image = APP.to_vec();
    image[0] = 0;
    assert_eq!(app_descriptor(&image[..]), Err(ImageError::Magic(0)));
    let mut image = APP.to_vec();
    image[28] = 1;
    assert!(matches!(
        app_descriptor(&image[..]),
        Err(ImageError::SegmentLength { index: 0, .. })
    ));
    assert_eq!(
        app_descriptor(&APP[..100]),
        Err(ImageError::SegmentPastEnd { index: 0 })
    );
    assert_eq!(app_descriptor(&APP[..10]), Err(ImageError::PastEnd));
}

/// A source whose reads fail from `from` on.
struct Failing<'a> {
    bytes: &'a [u8],
    from: u32,
}

impl Source for Failing<'_> {
    fn size(&self) -> u32 {
        self.bytes.size()
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), ReadError> {
        if offset + buf.len() as u32 > self.from {
            return Err(ReadError::Device);
        }
        self.bytes.read(offset, buf)
    }
}

#[test]
fn a_failing_read_is_reported() {
    let device = ImageError::Read(ReadError::Device);
    for from in [0, 23, 30, 40, 0x5000, BOOTLOADER.len() as u32 - 1] {
        let source = Failing {
            bytes: BOOTLOADER,
            from,
        };
        assert_eq!(
            run(verify_bootloader(&source)).map(|_| ()),
            Err(device),
            "{from}"
        );
    }
    let source = Failing {
        bytes: APP,
        from: 40,
    };
    assert_eq!(inspect_app(&source), Err(device));
    assert_eq!(app_descriptor(&source), Err(device));
    assert_eq!(run(verify_app(&source)), Err(device));
}

// The layout record.

/// The record of the board whose table is the fixture's.
fn board_record() -> LayoutRecord<{ PARTITIONS.len() }> {
    LayoutRecord::new(TABLE_OFFSET, FLASH_SIZE, &PARTITIONS).unwrap()
}

/// The bytes of `record` as it lies in memory, which is how the firmware's
/// image holds it.
fn record_bytes<const N: usize>(record: &LayoutRecord<N>) -> &[u8] {
    // SAFETY: every field of the `repr(C)` record is a byte array, so it has
    // no padding and every byte is initialized, and the slice borrows it.
    unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(record).cast::<u8>(),
            size_of::<LayoutRecord<N>>(),
        )
    }
}

/// The layout record `image` embeds, read and compared with the board's.
fn layout_of(image: &[u8]) -> Option<(AppLayout, bool)> {
    let layout = app_layout(image).unwrap()?;
    Some((layout, layout.matches(image, &board_record()).unwrap()))
}

#[test]
fn a_layout_record_is_its_format_s_bytes() {
    let record = board_record();
    assert_eq!(size_of::<LayoutRecord<8>>(), 32 + 8 * 32);
    assert_eq!(align_of::<LayoutRecord<8>>(), 1);
    assert_eq!(record_bytes(&record), &board_layout_record()[..]);
    assert_eq!(&record_bytes(&record)[..16], b"EVENINGSTAR-LYT1");
    assert_eq!(record.table_offset(), TABLE_OFFSET);
    assert_eq!(record.flash_size(), FLASH_SIZE);
    for (i, entry) in record.entries().iter().enumerate() {
        assert_eq!(&entry[..], &TABLE[i * 32..i * 32 + 32]);
    }
    let empty = LayoutRecord::<0>::new(TABLE_OFFSET, FLASH_SIZE, &[]).unwrap();
    assert_eq!(
        record_bytes(&empty),
        &layout_record_bytes(TABLE_OFFSET, FLASH_SIZE, &[])[..]
    );
}

#[test]
fn a_layout_record_needs_labels_that_fit_and_a_table_s_count() {
    let mut entries = PARTITIONS;
    entries[3].label = "seventeen-bytes-a";
    assert_eq!(LayoutRecord::new(TABLE_OFFSET, FLASH_SIZE, &entries), None);
    let many = [PARTITIONS[0]; table::MAX_ENTRIES + 1];
    assert_eq!(LayoutRecord::new(TABLE_OFFSET, FLASH_SIZE, &many), None);
    let most = [PARTITIONS[0]; table::MAX_ENTRIES];
    assert!(LayoutRecord::new(TABLE_OFFSET, FLASH_SIZE, &most).is_some());
}

#[test]
fn a_layout_record_round_trips_through_an_image() {
    let record = board_record();
    for image in [
        app_image_with_layout().build(),
        with_layout(APP, record_bytes(&record)),
    ] {
        assert!(app(&image).is_ok());
        let (layout, matches) = layout_of(&image).unwrap();
        assert!(matches);
        assert_eq!(layout.table_offset(), TABLE_OFFSET);
        assert_eq!(layout.flash_size(), FLASH_SIZE);
        assert_eq!(layout.entry_count(), PARTITIONS.len() as u32);
        for (index, entry) in (0..).zip(record.entries()) {
            assert_eq!(layout.entry(&image[..], index), Ok(Some(*entry)));
        }
        assert_eq!(layout.entry(&image[..], PARTITIONS.len() as u32), Ok(None));
        assert_eq!(layout.entry(&image[..], u32::MAX), Ok(None));
    }
}

#[test]
fn an_app_without_a_record_has_no_layout() {
    // The fixture predates the record, and the builder's app has data there.
    assert_eq!(app_layout(APP), Ok(None));
    assert_eq!(app_layout(&app_image().build()[..]), Ok(None));
    // A bootloader has no app descriptor.
    assert_eq!(app_layout(BOOTLOADER), Err(ImageError::MissingDescriptor));
}

/// A change to a layout: to its entries, giving the table offset and flash
/// size.
type Change = fn(&mut Vec<Entry>) -> (u32, u32);

#[test]
fn a_record_of_another_layout_reads_but_doesnt_match() {
    let changes: [Change; 6] = [
        |entries| {
            entries[3].size -= 0x1000;
            (TABLE_OFFSET, FLASH_SIZE)
        },
        |entries| {
            entries[0].label = "factorz";
            (TABLE_OFFSET, FLASH_SIZE)
        },
        |entries| {
            entries[6].flags = 1;
            (TABLE_OFFSET, FLASH_SIZE)
        },
        |entries| {
            entries.pop();
            (TABLE_OFFSET, FLASH_SIZE)
        },
        |_| (0x9000, FLASH_SIZE),
        |_| (TABLE_OFFSET, 2 * FLASH_SIZE),
    ];
    for (i, change) in changes.into_iter().enumerate() {
        let mut entries = PARTITIONS.to_vec();
        let (table_offset, flash_size) = change(&mut entries);
        let record = layout_record_bytes(table_offset, flash_size, &entries);
        let image = with_layout(APP, &record);
        let (layout, matches) = layout_of(&image).unwrap();
        assert!(!matches, "change {i}");
        assert_eq!(layout.entry_count() as usize, entries.len(), "change {i}");
    }
}

#[test]
fn a_record_is_read_only_right_after_the_descriptor() {
    let record = board_layout_record();
    // Four bytes further on, the bytes after the descriptor aren't a record.
    let mut shifted = vec![0; 4];
    shifted.extend_from_slice(&record);
    assert_eq!(layout_of(&with_layout(APP, &shifted)), None);
    // A second record after the first changes nothing: the first is the
    // image's, whatever follows it.
    let mut other = PARTITIONS.to_vec();
    other[2].offset += 0x1_0000;
    let mut two = record.clone();
    two.extend_from_slice(&layout_record_bytes(TABLE_OFFSET, FLASH_SIZE, &other));
    assert!(layout_of(&with_layout(APP, &two)).unwrap().1);
    let mut two = layout_record_bytes(TABLE_OFFSET, FLASH_SIZE, &other);
    two.extend_from_slice(&record);
    assert!(!layout_of(&with_layout(APP, &two)).unwrap().1);
}

#[test]
fn a_malformed_record_is_no_record() {
    let record = board_layout_record();
    let edits: [(usize, u8); 5] = [(0, b'e'), (15, b'2'), (28, 1), (31, 0x80), (27, 1)];
    for (at, value) in edits {
        let mut bad = record.clone();
        bad[at] = value;
        assert_eq!(layout_of(&with_layout(APP, &bad)), None, "byte {at}");
    }
    // More entries than a table holds.
    let mut bad = record.clone();
    bad[24..28].copy_from_slice(&97_u32.to_le_bytes());
    assert_eq!(layout_of(&with_layout(APP, &bad)), None);
    let mut most = record.clone();
    most[24..28].copy_from_slice(&96_u32.to_le_bytes());
    assert!(!layout_of(&with_layout(APP, &most)).unwrap().1);
}

#[test]
fn a_record_past_the_first_segment_is_no_record() {
    let record = board_layout_record();
    let len_of = |entries: usize| 256 + 32 + 32 * entries;
    let n = PARTITIONS.len();
    for first_len in [
        256,
        256 + 28,
        256 + 32,
        len_of(n - 1),
        len_of(n) - 4,
        len_of(n),
    ] {
        let mut builder = app_image();
        let first = &mut builder.segments[0].1;
        first.truncate(256);
        first.extend_from_slice(&record);
        first.truncate(first_len);
        let image = builder.build();
        assert!(app(&image).is_ok(), "{first_len}");
        let found = layout_of(&image);
        assert_eq!(found.is_some(), first_len == len_of(n), "{first_len}");
    }
}

#[test]
fn the_layout_read_checks_only_the_start_of_the_image() {
    let image = with_layout(APP, &board_layout_record());
    let mut corrupt = image.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    corrupt[1000] ^= 1;
    assert!(app(&corrupt).is_err());
    assert!(layout_of(&corrupt).unwrap().1);
    let mut image_bytes = image.clone();
    image_bytes[0] = 0;
    assert_eq!(app_layout(&image_bytes[..]), Err(ImageError::Magic(0)));
    assert_eq!(
        app_layout(&image[..100]),
        Err(ImageError::SegmentPastEnd { index: 0 })
    );
    let device = ImageError::Read(ReadError::Device);
    let source = Failing {
        bytes: &image,
        from: LAYOUT_RECORD_AT as u32 + 8,
    };
    assert_eq!(app_layout(&source), Err(device));
    let source = Failing {
        bytes: &image,
        from: LAYOUT_RECORD_AT as u32 + 40,
    };
    let layout = app_layout(&source).unwrap().unwrap();
    assert_eq!(layout.matches(&source, &board_record()), Err(device));
    assert_eq!(layout.entry(&source, 0), Err(device));
}

#[test]
fn mutated_records_never_panic() {
    let mut rng = Rng::new(0x1A7);
    let image = with_layout(APP, &board_layout_record());
    let mut mutated = image.clone();
    for at in 0..LAYOUT_RECORD_AT + 32 * 9 {
        for value in [0x00, 0xFF, image[at] ^ 0x80, rng.byte()] {
            mutated[at] = value;
            let _ = std::format!("{:?}", app_layout(&mutated[..]));
            if let Ok(Some(layout)) = app_layout(&mutated[..]) {
                let _ = layout.matches(&mutated[..], &board_record());
            }
        }
        mutated[at] = image[at];
    }
}

#[test]
fn errors_convert_from_read_errors() {
    assert_eq!(
        ImageError::from(ReadError::OutOfRange),
        ImageError::Read(ReadError::OutOfRange)
    );
}

// Never panicking.

/// Runs every image function over `source`.
fn exercise(source: &[u8]) {
    let _ = std::format!("{:?}", app(source));
    let _ = std::format!("{:?}", bootloader(source));
    let _ = std::format!("{:?}", inspect_app(source));
    let _ = std::format!("{:?}", app_descriptor(source));
    if let Ok(Some(layout)) = app_layout(source) {
        let _ = layout.matches(source, &board_record());
        for index in 0..=layout.entry_count().min(100) {
            let _ = layout.entry(source, index);
        }
    }
}

#[test]
fn random_images_never_panic() {
    let mut rng = Rng::new(0x1A6E);
    for round in 0..3000 {
        let count = rng.below(18) as u8;
        let mut segments = Vec::new();
        for i in 0..count {
            let len = 4 * rng.below(80);
            let addr = match rng.below(3) {
                0 => 0x4200_0020,
                1 => 0x4080_0000,
                _ => rng.next_u32(),
            };
            let mut data = data(&mut rng, len);
            if i == 0 && rng.below(2) == 0 {
                data = app_descriptor_bytes("random", "0");
            }
            segments.push((addr, data));
        }
        let mut builder = ImageBuilder::new(segments);
        builder.header[1] = count;
        if rng.below(4) == 0 {
            builder.header[23] = 0;
        }
        let mut image = builder.build();
        // Corrupt some of them: a byte, a length, or the end.
        match round % 4 {
            0 => {}
            1 => {
                let at = rng.below(image.len());
                image[at] = rng.byte();
            }
            2 if count > 0 => {
                let at = segment_at(&image, rng.below(count as usize)) + 4;
                if at + 4 <= image.len() {
                    image[at..at + 4].copy_from_slice(&rng.next_u32().to_le_bytes());
                }
            }
            _ => image.truncate(rng.below(image.len() + 1)),
        }
        exercise(&image);
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng::new(0xBAD);
    for _ in 0..3000 {
        let len = rng.below(512);
        let mut image = data(&mut rng, len);
        if image.len() >= 24 {
            image[0] = MAGIC;
            image[1] %= 18;
            image[12..14].copy_from_slice(&CHIP_ID.to_le_bytes());
        }
        exercise(&image);
    }
}

#[test]
fn mutated_bootloaders_never_panic() {
    let mut rng = Rng::new(0xB00);
    for fixture in [BOOTLOADER, &ESPFLASH_BOOTLOADER[..22016]] {
        let mut positions: Vec<usize> = (0..64).collect();
        for index in 0..3 {
            let at = segment_at(fixture, index);
            positions.extend(at..at + 8);
        }
        positions.extend(fixture.len() - 48..fixture.len());
        positions.extend((0..300).map(|_| rng.below(fixture.len())));
        for at in positions {
            for value in [0x00, 0xFF, fixture[at] ^ 0x01, rng.byte()] {
                let mut image = fixture.to_vec();
                image[at] = value;
                let _ = bootloader(&image);
            }
        }
    }
}

#[test]
fn mutated_apps_never_panic() {
    let mut rng = Rng::new(0xA99);
    // Inspecting is cheap: every byte of the headers and the descriptor, and
    // random bytes elsewhere.
    let mut positions: Vec<usize> = (0..32 + 256).collect();
    for index in 0..4 {
        let at = segment_at(APP, index);
        positions.extend(at..at + 8);
    }
    positions.extend((0..2000).map(|_| rng.below(APP.len())));
    let mut image = APP.to_vec();
    for &at in &positions {
        for value in [0x00, 0xFF, APP[at] ^ 0x80, rng.byte()] {
            image[at] = value;
            let _ = inspect_app(&image[..]);
            let _ = app_descriptor(&image[..]);
        }
        image[at] = APP[at];
    }
    // Verifying hashes the whole image, so fewer of them.
    for _ in 0..40 {
        let at = positions[rng.below(positions.len())];
        image[at] = rng.byte();
        let _ = app(&image);
        image[at] = APP[at];
    }
}
