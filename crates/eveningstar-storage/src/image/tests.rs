#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use super::*;
use crate::test_support::{
    APP, BOOTLOADER, ESPFLASH_BOOTLOADER, ImageBuilder, Rng, app_descriptor_bytes, app_image,
    block_on, bootloader_image, data, run, segment_at,
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

#[test]
fn the_nix_bootloader_verifies() {
    let verified = bootloader(BOOTLOADER).unwrap();
    assert_eq!(hex(verified.digest()), NIX_BOOTLOADER_DIGEST);
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
}

#[test]
fn the_nix_bootloader_verifies_in_its_partition() {
    // Up to the table at 0x8000; the erased bytes after the image don't count.
    let flash = in_partition(BOOTLOADER, 0x8000);
    let verified = bootloader(&flash).unwrap();
    assert_eq!(hex(verified.digest()), NIX_BOOTLOADER_DIGEST);
    assert_eq!(verified.size(), 22672);
}

#[test]
fn the_espflash_bootloader_verifies() {
    let verified = bootloader(ESPFLASH_BOOTLOADER).unwrap();
    assert_eq!(hex(verified.digest()), ESPFLASH_BOOTLOADER_DIGEST);
    assert_eq!(verified.size(), 22016);
    assert_eq!(verified.header().segment_count, 3);
    let descriptor = verified.descriptor().unwrap();
    assert_eq!(descriptor.version(), 1);
    assert_eq!(
        descriptor.idf_version().to_str(),
        Some("v6.1-beta1-497-g14f663f003e")
    );
    assert_ne!(verified.digest(), bootloader(BOOTLOADER).unwrap().digest());
}

#[test]
fn the_app_verifies() {
    let verified = app(APP).unwrap();
    assert_eq!(hex(verified.digest()), APP_DIGEST);
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
    assert_eq!(verified.size() as usize, APP.len());
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
    assert_eq!(verified.descriptor(), None);
}

#[test]
fn the_verification_yields_every_4_kib_hashed() {
    let hashed = APP.len() - 32;
    let verify = block_on(verify_app(APP));
    assert_eq!(hex(verify.output.unwrap().digest()), APP_DIGEST);
    assert_eq!(verify.wakes, hashed / YIELD_INTERVAL as usize);
    assert_eq!(verify.polls, verify.wakes + 1);
    assert_eq!(verify.wakes, 53);

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

// Built images.

#[test]
fn built_images_verify() {
    let image = app_image().build();
    let verified = app(&image).unwrap();
    assert_eq!(verified.size() as usize, image.len());
    assert_eq!(&verified.digest()[..], &image[image.len() - 32..]);
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
    assert_eq!(app(&image), Err(ImageError::MissingDescriptor));
    assert_eq!(inspect_app(&image[..]), Err(ImageError::MissingDescriptor));
}

#[test]
fn a_bootloader_with_a_short_or_unmarked_first_segment_has_no_descriptor() {
    let mut builder = bootloader_image();
    builder.segments[0].1.truncate(76);
    assert_eq!(bootloader(&builder.build()).unwrap().descriptor(), None);

    let mut builder = bootloader_image();
    builder.segments[0].1[0] = 0x51;
    assert_eq!(bootloader(&builder.build()).unwrap().descriptor(), None);
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
