#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets, lengths and counts are small"
)]

use core::future::Future;
use core::pin::{Pin, pin};
use core::task::{Context, Poll, Waker};

use eveningstar_boot::ImageFailure;
use eveningstar_esp_formats::Entry;
use eveningstar_esp_formats::image;
use eveningstar_esp_formats::otadata::{Otadata as Selection, PlanError, Sector, State};
use eveningstar_esp_formats::test_support::{
    APP, BOOTLOADER, ImageBuilder, OTADATA_FIRST_BOOT, OTADATA_LEN, Rng, TABLE, Tear,
    app_descriptor_bytes, boot, data, idf_loads, segment_at,
};
use eveningstar_storage::layout::{FLASH_SIZE, OTA_0, OTA_1, OTADATA, TABLE_OFFSET};
use eveningstar_storage::nor::{NorDevice, Reader};
use eveningstar_storage::roles::{AlternateApp, Otadata as OtadataRole};
use eveningstar_storage::test_support::{Call, RamNor, TestFlash, block_on, boot_device, run};
use eveningstar_storage::{
    DeviceError, SECTOR_SIZE, SlotPartitions as _, SnapshotError, Span, WORD_SIZE,
};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::test_support::{Checks, ota_on_spans, ota_over};
use crate::{OtaError, Unavailable, VerifiedImage};
use Slot::{Ota0, Ota1};

/// The otadata partition's offset.
const OTADATA_AT: u32 = OTADATA.offset;

// Images.

/// `image` with `edit` made to its segment data, and its checksum and digest
/// made again to match.
fn restamp(image: &[u8], edit: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut stamped = image.to_vec();
    edit(&mut stamped);
    let end = stamped.len() - 32;
    // The checksum is the XOR of the segment data, so it changes by the XOR
    // of the bytes changed.
    let change = image[..end]
        .iter()
        .zip(&stamped[..end])
        .fold(0, |change, (old, new)| change ^ old ^ new);
    stamped[end - 1] ^= change;
    let digest = Sha256::digest(&stamped[..end]);
    stamped[end..].copy_from_slice(&digest);
    stamped
}

/// `text`, NUL padded, in the `field` of an app descriptor.
fn set_text(field: &mut [u8], text: &str) {
    field.fill(0);
    field[..text.len()].copy_from_slice(text.as_bytes());
}

/// Offset of the app descriptor in an image: the first segment's data.
const DESCRIPTOR: usize = 24 + 8;

/// The firmware's image with its descriptor naming `version`.
fn app_version(version: &str) -> Vec<u8> {
    restamp(APP, |image| {
        set_text(&mut image[DESCRIPTOR + 16..DESCRIPTOR + 48], version);
    })
}

/// The firmware's image with its descriptor naming `project`.
fn app_project(project: &str) -> Vec<u8> {
    restamp(APP, |image| {
        set_text(&mut image[DESCRIPTOR + 48..DESCRIPTOR + 80], project);
    })
}

/// A small image of `project` and `version`, with `extra` words of data in
/// its first segment.
fn small_image(project: &str, version: &str, extra: usize) -> Vec<u8> {
    let mut rng = Rng::new(0x5A11 + extra as u64);
    let mut first = app_descriptor_bytes(project, version);
    first.extend(data(&mut rng, 0x1F00 + 4 * extra));
    ImageBuilder::new(vec![
        (0x4200_0020, first),
        (0x4080_0000, data(&mut rng, 0x1234)),
        (0x4081_0000, data(&mut rng, 0x10)),
    ])
    .build()
}

/// A small image of the firmware's project.
fn small_app(version: &str, extra: usize) -> Vec<u8> {
    small_image("eveningstar", version, extra)
}

#[test]
fn the_test_images_verify_with_what_they_name() {
    let images = [
        (app_version("0.2.0"), "eveningstar", "0.2.0"),
        (app_project("another"), "another", "0.1.0"),
        (small_app("1.0.0", 0), "eveningstar", "1.0.0"),
        (small_app("2.0.0", 999), "eveningstar", "2.0.0"),
        (small_image("another", "1.0.0", 3), "another", "1.0.0"),
    ];
    for (image, project, version) in images {
        let app = run(image::verify_app(&image[..])).unwrap();
        assert_eq!(app.size() as usize, image.len());
        assert!(idf_loads(&image));
        let descriptor = image::app_descriptor(&image[..]).unwrap();
        assert_eq!(descriptor.project_name().to_str(), Some(project));
        assert_eq!(descriptor.version().to_str(), Some(version));
    }
    assert_ne!(app_version("0.2.0"), APP);
    assert!(idf_loads(APP));
    let mut broken = APP.to_vec();
    broken[0x1000] ^= 1;
    assert!(!idf_loads(&broken));
    assert!(!idf_loads(&APP[..APP.len() - 1]));
    assert!(!idf_loads(&[0xFF; 64]));
}

// Boards.

/// A board `flash-all` wrote with `image` in `ota_0` and blank otadata,
/// booted once, which writes the otadata's first entry.
fn flashed(image: &[u8]) -> RamNor {
    let flash = RamNor::with(
        FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (TABLE_OFFSET, TABLE),
            (OTA_0.offset, image),
        ],
    );
    assert_eq!(boot_device(&flash), Some(Ota0));
    assert_eq!(flash.at(OTADATA_AT, OTADATA_LEN), OTADATA_FIRST_BOOT);
    flash
}

/// The OTA mechanism of `flash` booted from `running`, which reaches the
/// boot tier, and the checks of its slots.
fn ota_on(flash: &RamNor, running: Slot) -> (Ota<'_>, Checks<'_>) {
    ota_over(flash.lend(), running)
}

/// Confirms the image `flash` runs from `running`.
fn confirm_on(flash: &RamNor, running: Slot) {
    let (mut ota, checks) = ota_on(flash, running);
    checks.run(ota.confirm()).unwrap();
}

/// Boots `flash`, which has to start a slot.
fn reboot(flash: &RamNor) -> Slot {
    boot_device(flash).expect("no slot boots")
}

/// The otadata of `flash`.
fn otadata_of(flash: &RamNor) -> Selection {
    otadata::read(&flash.at(OTADATA_AT, OTADATA_LEN)[..])
}

/// The boot state `flash`'s otadata gives the image running from `running`.
fn state_of(flash: &RamNor, running: Slot) -> BootState {
    otadata_of(flash).boot_state(running)
}

/// Whether `slot` of `flash` starts with `image`.
fn holds(flash: &RamNor, slot: Slot, image: &[u8]) -> bool {
    flash.at(slot.partition().offset, image.len()) == image
}

/// The lengths `rng` cuts `image` into: up to `max` bytes, and now and then
/// a few, which leave partial words.
fn parts<'a>(image: &'a [u8], rng: &mut Rng, max: usize) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut rest = image;
    while !rest.is_empty() {
        let len = match rng.below(4) {
            0 => 1 + rng.below(7),
            _ => 1 + rng.below(max),
        };
        let (part, more) = rest.split_at(len.min(rest.len()));
        parts.push(part);
        rest = more;
    }
    parts
}

/// Writes `image` as an update in parts from `rng` of up to `max` bytes, and
/// finishes it.
async fn update<'f>(
    ota: &mut Ota<'f>,
    image: &[u8],
    rng: &mut Rng,
    max: usize,
) -> Result<VerifiedImage<'f>, OtaError> {
    let mut update = ota.begin_update().await?;
    for part in parts(image, rng, max) {
        update.write(part).await?;
    }
    update.finish().await
}

/// Writes `bytes` as an update in one part, and finishes it.
fn finish_with<'f>(
    ota: &mut Ota<'f>,
    checks: Checks<'f>,
    bytes: &[u8],
) -> Result<VerifiedImage<'f>, OtaError> {
    checks.run(async {
        let mut update = ota.begin_update().await?;
        update.write(bytes).await?;
        update.finish().await
    })
}

/// Updates the board running from `running` to `image`, activates it and
/// boots it.
fn update_and_boot(flash: &RamNor, running: Slot, image: &[u8], rng: &mut Rng) {
    let (mut ota, checks) = ota_on(flash, running);
    let verified = checks.run(update(&mut ota, image, rng, 0x8000)).unwrap();
    checks.run(ota.activate(verified)).unwrap();
    assert_eq!(reboot(flash), running.other());
}

/// A board that ran `old` from `ota_0`, then updated to `new` in `ota_1`,
/// confirmed it and booted it again: `old` is the rollback target.
fn updated(old: &[u8], new: &[u8]) -> RamNor {
    let flash = flashed(old);
    update_and_boot(&flash, Ota0, new, &mut Rng::new(0x0DD));
    confirm_on(&flash, Ota1);
    assert_eq!(reboot(&flash), Ota1);
    let state = state_of(&flash, Ota1);
    assert_eq!(state.state, Some(State::Valid));
    assert!(state.rollback_target);
    flash
}

/// The span of an erase or program.
fn op_span(op: Call) -> (u32, u32) {
    match op {
        Call::Erase(address) => (address, address + SECTOR_SIZE),
        Call::Program(address, len) => (address, address + len),
        Call::Read(..) => panic!("{op:?} isn't a write"),
    }
}

/// Every erase and program `flash` logged from the `from`th on lies in the
/// otadata or in the slot that isn't `running`.
fn confined(flash: &RamNor, from: usize, running: Slot) {
    for op in &flash.writes()[from..] {
        let (start, end) = op_span(*op);
        let inside = |entry: Entry| entry.offset <= start && end <= entry.end();
        assert!(
            inside(OTADATA) || inside(running.other().partition()),
            "{op:?} while running from {running:?}"
        );
    }
}

/// The operations of one otadata write to the sector at `at`: the erase,
/// then the label and state, the CRC and the sequence number.
fn entry_ops(at: u32) -> [Call; 4] {
    [
        Call::Erase(at),
        Call::Program(at + 4, 24),
        Call::Program(at + 28, 4),
        Call::Program(at, 4),
    ]
}

// A whole update.

#[test]
fn an_update_is_written_activated_booted_and_confirmed_and_then_another() {
    let old = APP;
    let new = app_version("0.2.0");
    let newer = app_version("0.3.0");
    let mut rng = Rng::new(0x07A);
    let flash = flashed(old);

    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state().unwrap(),
        BootState {
            running: Ota0,
            selected: Some(Ota0),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        }
    );
    let image = checks
        .run(update(&mut ota, &new, &mut rng, 20_000))
        .unwrap();
    assert_eq!(image.slot(), Ota1);
    assert_eq!(image.size() as usize, new.len());
    assert_eq!(image.digest()[..], new[new.len() - 32..]);
    assert_eq!(image.version().to_str(), Some("0.2.0"));
    assert!(holds(&flash, Ota1, &new));
    checks.run(ota.activate(image)).unwrap();
    // Selected, while the running image keeps its own state.
    let state = ota.boot_state().unwrap();
    assert_eq!(state.selected, Some(Ota1));
    assert_eq!(state.state, Some(State::Valid));
    confined(&flash, 0, Ota0);
    assert!(holds(&flash, Ota0, old));

    // Started once, with its rollback armed, and confirmed.
    assert_eq!(reboot(&flash), Ota1);
    let ops = flash.writes().len();
    let (mut ota, checks) = ota_on(&flash, Ota1);
    assert_eq!(
        ota.boot_state().unwrap(),
        BootState {
            running: Ota1,
            selected: Some(Ota1),
            state: Some(State::PendingVerify),
            rollback_target: true,
            needs_confirmation: true,
        }
    );
    checks.run(ota.confirm()).unwrap();
    assert_eq!(ota.boot_state().unwrap().state, Some(State::Valid));
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(reboot(&flash), Ota1);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    assert_eq!(
        ota.boot_state().unwrap(),
        BootState {
            running: Ota1,
            selected: Some(Ota1),
            state: Some(State::Valid),
            rollback_target: true,
            needs_confirmation: false,
        }
    );

    // And back the other way, over the old image.
    let image = checks
        .run(update(&mut ota, &newer, &mut rng, 50_000))
        .unwrap();
    assert_eq!(image.slot(), Ota0);
    assert_eq!(image.version().to_str(), Some("0.3.0"));
    checks.run(ota.activate(image)).unwrap();
    confined(&flash, ops, Ota1);
    assert!(holds(&flash, Ota1, &new));
    assert_eq!(reboot(&flash), Ota0);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(ota.boot_state().unwrap().state, Some(State::PendingVerify));
    checks.run(ota.confirm()).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::Valid));
    assert!(holds(&flash, Ota0, &newer));
    assert!(holds(&flash, Ota1, &new));
}

#[test]
fn the_parts_an_image_arrives_in_dont_change_what_is_written() {
    let new = small_app("2.0.0", 1000);
    let flash = flashed(&small_app("1.0.0", 0));
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let patterns: [&[usize]; 8] = [
        &[1],
        &[2],
        &[3],
        &[4],
        &[5, 3, 1, 7],
        &[4095, 1, 4096, 4097],
        &[0x1FFF, 2, 0x2001],
        &[new.len()],
    ];
    for pattern in patterns {
        let mut lens = pattern.iter().copied().cycle();
        let image = checks
            .run(async {
                let mut update = ota.begin_update().await?;
                let mut rest = &new[..];
                while !rest.is_empty() {
                    let (part, more) = rest.split_at(lens.next().unwrap().min(rest.len()));
                    update.write(part).await?;
                    rest = more;
                }
                assert_eq!(update.written() as usize, new.len());
                update.finish().await
            })
            .unwrap();
        assert_eq!(image.size() as usize, new.len(), "{pattern:?}");
        assert!(holds(&flash, Ota1, &new), "{pattern:?}");
    }
    // Empty parts change nothing.
    let image = checks
        .run(async {
            let mut update = ota.begin_update().await?;
            for part in new.chunks(1000) {
                update.write(&[]).await?;
                update.write(part).await?;
            }
            update.finish().await
        })
        .unwrap();
    assert_eq!(image.size() as usize, new.len());
}

#[test]
fn each_sector_is_erased_just_before_the_first_bytes_programmed_into_it() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    // Zeros, which nothing programmed over them could set back.
    flash.poke(OTA_1.offset, &vec![0; OTA_1.size as usize]);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let mut rng = Rng::new(0xE7A5);
    let from = flash.writes().len();
    let image = checks
        .run(async {
            let mut update = ota.begin_update().await?;
            let begun = flash.writes().len();
            assert_eq!(flash.writes()[begun - 1], Call::Erase(OTA_1.offset));
            for part in parts(&new, &mut rng, 10_000) {
                update.write(part).await?;
                // Up to the sector the last whole word programmed lies in.
                let programmed = update.written() - update.written() % WORD_SIZE;
                let sectors = programmed.div_ceil(SECTOR_SIZE).max(1);
                let erased: Vec<_> = flash.writes()[begun - 1..]
                    .iter()
                    .filter_map(|op| match op {
                        Call::Erase(address) => Some(*address),
                        Call::Program(..) | Call::Read(..) => None,
                    })
                    .collect();
                let expected: Vec<_> = (0..sectors)
                    .map(|sector| OTA_1.offset + sector * SECTOR_SIZE)
                    .collect();
                assert_eq!(erased, expected);
            }
            update.finish().await
        })
        .unwrap();
    assert_eq!(image.size() as usize, new.len());
    // Every program follows its sector's erase.
    let mut erased = Vec::new();
    for op in &flash.writes()[from..] {
        match *op {
            Call::Erase(address) => erased.push(address),
            Call::Program(address, len) if OTA_1.offset <= address => {
                let sector = address - address % SECTOR_SIZE;
                assert!(erased.contains(&sector), "{op:?}");
                assert_eq!(sector, (address + len - 1) / SECTOR_SIZE * SECTOR_SIZE);
            }
            Call::Program(..) | Call::Read(..) => {}
        }
    }
    // The sectors past the image are left alone.
    let past = OTA_1.offset + (new.len() as u32).div_ceil(SECTOR_SIZE) * SECTOR_SIZE;
    assert_eq!(flash.at(past, 0x1000), vec![0; 0x1000]);
    assert!(holds(&flash, Ota1, &new));
}

#[test]
fn a_partial_word_is_held_until_it_is_completed_or_the_update_finishes() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let at = OTA_1.offset;
    checks.run(async {
        let mut update = ota.begin_update().await.unwrap();
        let ops = flash.writes().len();
        update.write(&[1, 2, 3]).await.unwrap();
        assert_eq!(flash.writes().len(), ops, "nothing programmed");
        assert_eq!(update.written(), 3);
        update.write(&[4]).await.unwrap();
        assert_eq!(flash.writes()[ops..], [Call::Program(at, 4)]);
        update.write(&[5, 6]).await.unwrap();
        update.write(&[7, 8, 9, 10, 11]).await.unwrap();
        assert_eq!(
            flash.writes()[ops..],
            [Call::Program(at, 4), Call::Program(at + 4, 4)]
        );
        assert_eq!(update.written(), 11);
        // The partial word is completed with 0xFF and programmed; the check
        // stops at the magic.
        assert_eq!(
            update.finish().await,
            Err(OtaError::Image(ImageFailure::Magic))
        );
        assert_eq!(flash.writes()[ops + 2..], [Call::Program(at + 8, 4)]);
    });
    assert_eq!(
        flash.at(at, 16),
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
        ]
    );
}

// What `finish` refuses.

#[test]
fn an_update_with_nothing_or_a_few_bytes_written_is_refused() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        finish_with(&mut ota, checks, &[]),
        Err(OtaError::Image(ImageFailure::Magic))
    );
    // The header, then erased bytes where the first segment's header goes.
    assert_eq!(
        finish_with(&mut ota, checks, &APP[..24]),
        Err(OtaError::Image(ImageFailure::SegmentLength { index: 0 }))
    );
    assert!(matches!(
        finish_with(&mut ota, checks, &APP[..1000]),
        Err(OtaError::Image(_))
    ));
}

#[test]
fn a_truncated_image_is_refused() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    for cut in [
        1,
        2,
        3,
        4,
        5,
        16,
        31,
        32,
        33,
        4096,
        new.len() / 2,
        new.len() - 24,
    ] {
        let result = finish_with(&mut ota, checks, &new[..new.len() - cut]);
        assert!(
            matches!(result, Err(OtaError::Image(_) | OtaError::Length { .. })),
            "{cut}: {result:?}"
        );
    }
    // With the same image left in the slot by an earlier update, an update
    // cut at a sector boundary leaves the image's tail in the sectors it
    // never reached. The image verifies, and only the length is wrong.
    assert!(finish_with(&mut ota, checks, &new).is_ok());
    let last = (new.len() - 1) / 4096 * 4096;
    for end in [last, last - 4096, 0x1_0000] {
        assert_eq!(
            finish_with(&mut ota, checks, &new[..end]),
            Err(OtaError::Length {
                written: end as u32,
                image: new.len() as u32,
            }),
            "{end:#x}"
        );
        assert!(finish_with(&mut ota, checks, &new).is_ok());
    }
}

#[test]
fn a_write_short_of_bytes_the_padding_puts_back_is_refused() {
    // An image whose digest ends in 0xFF: without its last byte, the padding
    // of the partial word puts the byte back and the image verifies, but
    // the byte wasn't written.
    let image = (0..10_000)
        .map(|n| small_app(&format!("1.0.{n}"), 0))
        .find(|image| image.last() == Some(&0xFF))
        .unwrap();
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let len = image.len() as u32;
    assert_eq!(
        finish_with(&mut ota, checks, &image[..image.len() - 1]),
        Err(OtaError::Length {
            written: len - 1,
            image: len,
        })
    );
    assert!(holds(&flash, Ota1, &image));
    assert_eq!(
        finish_with(&mut ota, checks, &image).map(|image| image.size()),
        Ok(len)
    );
}

#[test]
fn bytes_after_the_image_are_refused() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    for extra in [&[0xFF][..], &[0; 3], &[0xFF; 4], &[1; 100], &[0xFF; 4096]] {
        let mut bytes = new.clone();
        bytes.extend_from_slice(extra);
        assert_eq!(
            finish_with(&mut ota, checks, &bytes),
            Err(OtaError::Length {
                written: bytes.len() as u32,
                image: new.len() as u32,
            }),
            "{} more",
            extra.len()
        );
    }
}

#[test]
fn a_changed_byte_anywhere_is_refused() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let mut rng = Rng::new(0xB17);
    let len = new.len();
    // The header, every segment header, the descriptor, the tail and a sample
    // of the rest.
    let mut positions: Vec<usize> = (0..24).chain(len - 64..len).collect();
    for index in 0..4 {
        let at = segment_at(&new, index);
        positions.extend(at..at + 8);
    }
    positions.extend((DESCRIPTOR..DESCRIPTOR + 256).step_by(7));
    positions.extend((0..150).map(|_| rng.below(len)));
    for at in positions {
        let mut bytes = new.clone();
        bytes[at] ^= 1 << rng.below(8);
        let result = finish_with(&mut ota, checks, &bytes);
        assert!(
            matches!(result, Err(OtaError::Image(_))),
            "{at:#x}: {result:?}"
        );
    }
    assert!(finish_with(&mut ota, checks, &new).is_ok());
}

#[test]
fn an_image_for_another_chip_is_refused() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let mut other = APP.to_vec();
    // The ESP32-C3's, with the digest made again.
    other[12..14].copy_from_slice(&0x0005_u16.to_le_bytes());
    let other = restamp(&other, |_| {});
    assert_eq!(
        finish_with(&mut ota, checks, &other),
        Err(OtaError::Image(ImageFailure::Chip))
    );
    assert_eq!(
        checks.run(ota.verify_other()),
        Err(OtaError::Image(ImageFailure::Chip))
    );
}

#[test]
fn an_image_of_another_project_is_refused() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let images = [
        app_project("another"),
        app_project("eveningstar2"),
        app_project("evening"),
        app_project(""),
        small_image("another", "1.0.0", 0),
    ];
    for image in images {
        assert_eq!(
            finish_with(&mut ota, checks, &image),
            Err(OtaError::Project)
        );
        assert_eq!(checks.run(ota.verify_other()), Err(OtaError::Project));
    }
    // Bytes after the name's NUL don't count, as they aren't part of it.
    let padded = restamp(APP, |image| {
        image[DESCRIPTOR + 48 + 30] = b'x';
    });
    assert_eq!(
        finish_with(&mut ota, checks, &padded).map(|image| image.size()),
        Ok(APP.len() as u32)
    );
}

#[test]
fn the_project_is_the_running_app_s() {
    // A board running a small image of another project updates only to that
    // project.
    let theirs = small_image("another", "1.0.0", 0);
    let flash = flashed(&theirs);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(finish_with(&mut ota, checks, APP), Err(OtaError::Project));
    let next = small_image("another", "2.0.0", 5);
    assert_eq!(
        finish_with(&mut ota, checks, &next).map(|image| image.size()),
        Ok(next.len() as u32)
    );
}

#[test]
fn writing_past_the_end_of_the_slot_is_refused_before_anything_is_written() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let slot = OTA_1.size as usize;
    checks.run(async {
        let mut update = ota.begin_update().await.unwrap();
        let ops = flash.writes().len();
        assert_eq!(
            update.write(&vec![0; slot + 1]).await,
            Err(OtaError::TooLarge)
        );
        assert_eq!(flash.writes().len(), ops);
        assert_eq!(update.written(), 0);

        let zeros = vec![0; 0x1_0000];
        for _ in 0..slot / zeros.len() - 1 {
            update.write(&zeros).await.unwrap();
        }
        update.write(&zeros[..zeros.len() - 3]).await.unwrap();
        let ops = flash.writes().len();
        assert_eq!(update.write(&[0; 4]).await, Err(OtaError::TooLarge));
        assert_eq!(flash.writes().len(), ops, "nothing of it written");
        // The update goes on.
        update.write(&[0; 3]).await.unwrap();
        assert_eq!(update.written() as usize, slot);
        assert_eq!(update.write(&[0]).await, Err(OtaError::TooLarge));
        update.write(&[]).await.unwrap();
        assert_eq!(
            update.finish().await,
            Err(OtaError::Image(ImageFailure::Magic))
        );
    });
    confined(&flash, 0, Ota0);
    let last = flash.writes().last().copied().unwrap();
    assert_eq!(op_span(last).1, OTA_1.end());
}

#[test]
fn a_failed_write_ends_the_update() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    checks.run(async {
        let mut update = ota.begin_update().await.unwrap();
        update.write(&new[..0x2000]).await.unwrap();
        // The third sector's erase goes through, its program doesn't.
        flash.fail_after(1);
        assert_eq!(
            update.write(&new[0x2000..0x3000]).await,
            Err(OtaError::Access(AccessError::Device))
        );
        flash.heal();
        assert_eq!(
            update.write(&new[0x3000..]).await,
            Err(OtaError::Interrupted)
        );
        assert_eq!(update.finish().await, Err(OtaError::Interrupted));
    });
    // A new update starts over.
    let image = finish_with(&mut ota, checks, &new).unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert!(holds(&flash, Ota1, &new));
}

// Stale and changed images.

#[test]
fn an_update_begun_since_makes_a_verified_image_stale() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let finished = finish_with(&mut ota, checks, &new).unwrap();
    let verified = checks.run(ota.verify_other()).unwrap();
    assert_eq!(finished, verified);
    // An update begun and abandoned.
    assert!(checks.run(ota.begin_update()).is_ok());
    let ops = flash.writes().len();
    assert_eq!(checks.run(ota.activate(finished)), Err(OtaError::Stale));
    assert_eq!(checks.run(ota.activate(verified)), Err(OtaError::Stale));
    assert_eq!(flash.writes().len(), ops, "nothing written");
    assert_eq!(ota.boot_state().unwrap().selected, Some(Ota0));
    // The abandoned update erased the slot's first sector.
    assert_eq!(
        checks.run(ota.verify_other()),
        Err(OtaError::Image(ImageFailure::Magic))
    );
    assert_eq!(reboot(&flash), Ota0);
}

#[test]
fn a_refused_update_still_makes_a_verified_image_stale() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &new, &mut Rng::new(1));
    let (mut ota, checks) = ota_on(&flash, Ota1);
    // The old image, which verifies, while the new one isn't confirmed.
    let old = checks.run(ota.verify_other()).unwrap();
    let ops = flash.writes().len();
    assert_eq!(
        checks.run(ota.begin_update()).err(),
        Some(OtaError::Plan(PlanError::Unconfirmed))
    );
    assert_eq!(checks.run(ota.activate(old)), Err(OtaError::Stale));
    assert_eq!(flash.writes().len(), ops, "nothing written");
}

#[test]
fn an_image_changed_without_a_session_is_activated_and_the_bootloader_falls_back() {
    // Bytes that change on the flash outside a write session, as decay
    // changes them, leave the slot's epoch, so activating doesn't read the
    // slot again; the bootloader, which verifies the image, starts the
    // running slot instead.
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &new).unwrap();
    flash.poke(OTA_1.offset + 0x5000, &[new[0x5000] ^ 0x10]);
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    let state = state_of(&flash, Ota0);
    assert_eq!(state.selected, Some(Ota1));
    assert!(state.needs_confirmation);
    confirm_on(&flash, Ota0);
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::Valid));
}

#[test]
fn verify_other_takes_the_check_of_what_the_other_slot_holds() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        checks.run(ota.verify_other()),
        Err(OtaError::Image(ImageFailure::Magic))
    );
    let mut rng = Rng::new(0x6A7);
    for bytes in [data(&mut rng, 0x4000), vec![0xE9]] {
        let flash = flashed(APP);
        flash.poke(OTA_1.offset, &bytes);
        let (mut ota, checks) = ota_on(&flash, Ota0);
        assert!(matches!(
            checks.run(ota.verify_other()),
            Err(OtaError::Image(_))
        ));
    }
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    flash.poke(OTA_1.offset, &new);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = checks.run(ota.verify_other()).unwrap();
    assert_eq!(image.slot(), Ota1);
    assert_eq!(image.size() as usize, new.len());
    assert_eq!(image.version().to_str(), Some("0.2.0"));
    assert_eq!(flash.writes(), [], "verifying writes nothing");
}

#[test]
fn the_debug_output_names_the_slot_and_the_progress() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &app_version("0.2.0")).unwrap();
    let debug = format!("{image:?}");
    assert!(
        debug.contains("Ota1") && debug.contains("\"0.2.0\""),
        "{debug}"
    );
    let ota_debug = format!("{ota:?}");
    assert!(ota_debug.contains("generation: 1"), "{ota_debug}");
    checks.run(async {
        let update = ota.begin_update().await.unwrap();
        let debug = format!("{update:?}");
        assert!(
            debug.contains("written: 0") && debug.contains("Ota1"),
            "{debug}"
        );
    });
}

// Activation.

#[test]
fn an_image_is_activated_once() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &app_version("0.2.0")).unwrap();
    let again = checks.run(ota.verify_other()).unwrap();
    let ops = flash.writes().len();
    let inactive = otadata_of(&flash).active().unwrap().other();
    checks.run(ota.activate(image)).unwrap();
    // The entry selecting the slot, written to the sector that wasn't active.
    let otadata = otadata_of(&flash);
    assert_eq!(otadata.active(), Some(inactive));
    let entry = otadata.active_entry().unwrap();
    assert_eq!((entry.slot(), entry.state), (Ota1, State::New));
    let at = OTADATA_AT + inactive.offset();
    assert_eq!(flash.writes()[ops..], entry_ops(at));
    // Selecting it again isn't selecting the running slot's image away.
    assert_eq!(
        checks.run(ota.activate(again)),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert_eq!(flash.writes().len(), ops + 4);
}

#[test]
fn an_image_isnt_activated_while_the_running_one_is_unconfirmed() {
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(2));
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let old = checks.run(ota.verify_other()).unwrap();
    assert_eq!(
        checks.run(ota.activate(old)),
        Err(OtaError::Plan(PlanError::Unconfirmed))
    );
    // Rolling back is the way back.
    checks.run(ota.roll_back()).unwrap();
    assert_eq!(reboot(&flash), Ota0);
}

#[test]
fn the_image_in_the_other_slot_is_activated_again() {
    let new = app_version("0.2.0");
    let flash = updated(APP, &new);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let old = checks.run(ota.verify_other()).unwrap();
    assert_eq!(old.version().to_str(), Some("0.1.0"));
    checks.run(ota.activate(old)).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::PendingVerify));
    // Unconfirmed, it returns to the new image.
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::Valid));
}

// Beginning an update from each boot state.

#[test]
fn beginning_an_update_cancels_an_activation_not_yet_booted() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &new).unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(ota.boot_state().unwrap().selected, Some(Ota1));
    let ops = flash.writes().len();
    let activation = otadata_of(&flash).active().unwrap();
    let at = OTADATA_AT + activation.offset();
    assert!(checks.run(ota.begin_update()).is_ok());
    // The activation rewritten in place as Invalid, which leaves the other
    // entry, for the running slot, active; then a second entry for the
    // running slot in the sector that isn't active, the same one; then the
    // slot's first sector.
    assert_eq!(
        flash.writes()[ops..],
        [
            &entry_ops(at)[..],
            &entry_ops(at),
            &[Call::Erase(OTA_1.offset)]
        ]
        .concat()
    );
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota0));
    }
    assert_eq!(ota.boot_state().unwrap().selected, Some(Ota0));
    // The slot is rewritten and activated.
    let newer = app_version("0.3.0");
    let image = finish_with(&mut ota, checks, &newer).unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert!(holds(&flash, Ota1, &newer));
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn after_a_fallback_an_update_follows_a_confirmation() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &new).unwrap();
    checks.run(ota.activate(image)).unwrap();
    // The image breaks before the boot, so the bootloader falls back.
    flash.poke(OTA_1.offset + 0x100, &[!new[0x100]]);
    assert_eq!(reboot(&flash), Ota0);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state().unwrap(),
        BootState {
            running: Ota0,
            selected: Some(Ota1),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: true,
        }
    );
    let ops = flash.writes().len();
    assert_eq!(
        checks.run(ota.begin_update()).err(),
        Some(OtaError::Plan(PlanError::Unconfirmed))
    );
    assert_eq!(
        checks.run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert!(matches!(
        checks.run(ota.verify_other()),
        Err(OtaError::Image(_))
    ));
    assert_eq!(flash.writes().len(), ops, "nothing written");

    // Confirmed beside the active entry, which stays until replaced.
    let beside = otadata_of(&flash).active().unwrap().other();
    checks.run(ota.confirm()).unwrap();
    assert_eq!(
        flash.writes()[ops..],
        entry_ops(OTADATA_AT + beside.offset())
    );
    let state = ota.boot_state().unwrap();
    assert_eq!(
        (state.selected, state.state),
        (Some(Ota0), Some(State::Valid))
    );
    assert!(!state.needs_confirmation);
    assert!(checks.run(ota.begin_update()).is_ok());
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota0));
    }
    let image = finish_with(&mut ota, checks, &new).unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn a_decayed_selected_image_leaves_the_other_running_until_it_is_confirmed() {
    // An update booted once; then the old image, its rollback target,
    // decays. Unconfirmed, the reboot aborts the update, the bootloader
    // can't load the old image and falls back to the update, now aborted.
    let new = app_version("0.2.0");
    let newer = app_version("0.3.0");
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &new, &mut Rng::new(11));
    flash.poke(OTA_0.offset + 0x2000, &[!APP[0x2000]]);
    assert_eq!(reboot(&flash), Ota1);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let state = ota.boot_state().unwrap();
    assert_eq!(state.selected, Some(Ota0));
    assert_eq!(state.state, Some(State::Aborted));
    assert!(state.needs_confirmation);
    assert_eq!(
        checks
            .run(update(&mut ota, &newer, &mut Rng::new(12), 9000))
            .err(),
        Some(OtaError::Plan(PlanError::Unconfirmed))
    );
    // Unconfirmed, every boot falls back the same way.
    assert_eq!(reboot(&flash), Ota1);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    checks.run(ota.confirm()).unwrap();
    assert!(!ota.boot_state().unwrap().needs_confirmation);
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::Valid));
    // The update goes ahead, over the decayed image.
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let image = checks
        .run(update(&mut ota, &newer, &mut Rng::new(13), 9000))
        .unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::PendingVerify));
    assert!(holds(&flash, Ota0, &newer));
}

#[test]
fn beginning_an_update_is_refused_while_the_running_image_is_unconfirmed() {
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(3));
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let ops = flash.writes().len();
    assert_eq!(
        checks.run(ota.begin_update()).err(),
        Some(OtaError::Plan(PlanError::Unconfirmed))
    );
    assert_eq!(flash.writes().len(), ops, "nothing written");
    assert!(holds(&flash, Ota0, APP));
    checks.run(ota.confirm()).unwrap();
    assert!(checks.run(ota.begin_update()).is_ok());
}

#[test]
fn without_a_valid_entry_an_update_follows_a_confirmation() {
    // An aborted entry and a corrupt one; ota_0 doesn't load, so the
    // bootloader started ota_1.
    let mut otadata = vec![0xFF; OTADATA_LEN];
    otadata[..32].copy_from_slice(&otadata::SelectEntry::new(3, State::Aborted).to_bytes());
    let mut corrupt = otadata::SelectEntry::new(4, State::Valid);
    corrupt.crc ^= 1;
    otadata[4096..4096 + 32].copy_from_slice(&corrupt.to_bytes());
    let flash = RamNor::with(
        FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (TABLE_OFFSET, TABLE),
            (OTADATA_AT, &otadata),
            (OTA_1.offset, APP),
        ],
    );
    assert_eq!(reboot(&flash), Ota1);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    assert_eq!(ota.boot_state().unwrap().selected, None);
    assert!(ota.boot_state().unwrap().needs_confirmation);
    let new = app_version("0.2.0");
    assert_eq!(
        finish_with(&mut ota, checks, &new).err(),
        Some(OtaError::Plan(PlanError::Unconfirmed))
    );
    // The entry the bootloader writes on blank otadata, in the first
    // sector.
    let ops = flash.writes().len();
    checks.run(ota.confirm()).unwrap();
    assert_eq!(flash.writes()[ops..], entry_ops(OTADATA_AT));
    assert_eq!(otadata_of(&flash).selected(), Some(Ota1));
    let image = finish_with(&mut ota, checks, &new).unwrap();
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota1));
    }
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::PendingVerify));
    assert_eq!(reboot(&flash), Ota1);
}

/// A flash whose programs of the otadata report success and change nothing.
struct Forgetful<'a>(&'a RamNor);

impl NorDevice for Forgetful<'_> {
    fn capacity(&self) -> u32 {
        self.0.capacity()
    }

    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        self.0.read(address, buf)
    }

    fn erase_sector(&self, address: u32) -> Result<(), DeviceError> {
        self.0.erase_sector(address)
    }

    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError> {
        if (OTADATA.offset..OTADATA.end()).contains(&address) {
            Ok(())
        } else {
            self.0.program(address, bytes)
        }
    }
}

#[test]
fn beginning_an_update_stops_when_the_otadata_doesnt_keep_what_is_written() {
    let flash = flashed(APP);
    let mut forgetful = Forgetful(&flash);
    let (mut ota, checks) = ota_over(&mut forgetful, Ota0);
    assert_eq!(
        checks.run(ota.begin_update()).err(),
        Some(OtaError::Unsettled)
    );
    // Two writes of the second sector, and nothing of the slot.
    let second = OTADATA_AT + SECTOR_SIZE;
    assert_eq!(flash.writes(), [Call::Erase(second), Call::Erase(second)]);
}

// Confirming.

#[test]
fn confirming_rewrites_a_pending_image_s_entry_once() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    checks.run(ota.confirm()).unwrap();
    assert_eq!(flash.writes(), [], "already valid");

    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(4));
    let (mut ota, checks) = ota_on(&flash, Ota1);
    let ops = flash.writes().len();
    let active = otadata_of(&flash).active().unwrap();
    checks.run(ota.confirm()).unwrap();
    // The active entry, rewritten in place: the rollback target stays.
    let at = OTADATA_AT + active.offset();
    assert_eq!(flash.writes()[ops..], entry_ops(at));
    assert_eq!(otadata_of(&flash).active(), Some(active));
    assert!(ota.boot_state().unwrap().rollback_target);
    checks.run(ota.confirm()).unwrap();
    assert_eq!(flash.writes().len(), ops + 4);
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::Valid));
}

#[test]
fn confirming_leaves_an_activation_not_yet_booted() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, checks, &app_version("0.2.0")).unwrap();
    checks.run(ota.activate(image)).unwrap();
    let ops = flash.writes().len();
    assert!(!ota.boot_state().unwrap().needs_confirmation);
    checks.run(ota.confirm()).unwrap();
    assert_eq!(flash.writes().len(), ops);
    assert_eq!(ota.boot_state().unwrap().selected, Some(Ota1));
    assert_eq!(reboot(&flash), Ota1);
}

/// A board booted in a state that needs a confirmation, running from the
/// returned slot.
struct Unconfirmed {
    name: &'static str,
    flash: RamNor,
    running: Slot,
    /// The image the running slot holds.
    image: Vec<u8>,
}

/// A board in each state a booted image needs a confirmation in.
fn unconfirmed_boards() -> Vec<Unconfirmed> {
    let old = small_app("1.0.0", 0);
    let new = small_app("2.0.0", 500);
    let otadata_with = |entries: &[(u32, State)]| {
        let mut otadata = vec![0xFF; OTADATA_LEN];
        for (at, &(seq, state)) in [0, 4096].into_iter().zip(entries) {
            otadata[at..at + 32].copy_from_slice(&otadata::SelectEntry::new(seq, state).to_bytes());
        }
        otadata
    };
    let board = |otadata: &[u8], images: [Option<&[u8]>; 2]| {
        let flash = RamNor::with(
            FLASH_SIZE,
            &[
                (0, BOOTLOADER),
                (TABLE_OFFSET, TABLE),
                (OTADATA_AT, otadata),
            ],
        );
        for (slot, image) in Slot::ALL.into_iter().zip(images) {
            if let Some(image) = image {
                flash.poke(slot.partition().offset, image);
            }
        }
        flash
    };
    let decay = |flash: &RamNor, slot: Slot| {
        let at = slot.partition().offset + 0x40;
        flash.poke(at, &[!flash.at(at, 1)[0]]);
    };

    // Booted once after an update.
    let pending = flashed(&old);
    update_and_boot(&pending, Ota0, &new, &mut Rng::new(20));
    // The same, after its rollback target decayed: the bootloader aborts it,
    // can't load the target and falls back to it.
    let aborted = pending.copy();
    decay(&aborted, Ota0);
    assert_eq!(reboot(&aborted), Ota1);
    // A fallback to a slot no entry names.
    let unnamed = board(
        &otadata_with(&[(1, State::Valid)]),
        [Some(&old), Some(&new)],
    );
    decay(&unnamed, Ota0);
    assert_eq!(reboot(&unnamed), Ota1);
    // A fallback from an activation that didn't load, beside a valid entry.
    let tried = flashed(&old);
    let (mut ota, checks) = ota_on(&tried, Ota0);
    let image = finish_with(&mut ota, checks, &new).unwrap();
    checks.run(ota.activate(image)).unwrap();
    decay(&tried, Ota1);
    assert_eq!(reboot(&tried), Ota0);
    // No state.
    let undefined = board(&otadata_with(&[(1, State::Undefined)]), [Some(&old), None]);
    assert_eq!(reboot(&undefined), Ota0);
    // No valid entry: ota_0 doesn't load.
    let none = board(
        &otadata_with(&[(3, State::Aborted), (4, State::Invalid)]),
        [None, Some(&new)],
    );
    assert_eq!(reboot(&none), Ota1);
    vec![
        Unconfirmed {
            name: "pending",
            flash: pending,
            running: Ota1,
            image: new.clone(),
        },
        Unconfirmed {
            name: "fallback, own entry aborted",
            flash: aborted,
            running: Ota1,
            image: new.clone(),
        },
        Unconfirmed {
            name: "fallback, no own entry",
            flash: unnamed,
            running: Ota1,
            image: new.clone(),
        },
        Unconfirmed {
            name: "fallback, own entry valid",
            flash: tried,
            running: Ota0,
            image: old.clone(),
        },
        Unconfirmed {
            name: "undefined",
            flash: undefined,
            running: Ota0,
            image: old.clone(),
        },
        Unconfirmed {
            name: "no valid entry",
            flash: none,
            running: Ota1,
            image: new,
        },
    ]
}

#[test]
fn confirming_makes_any_booted_image_valid_and_lets_an_update_begin() {
    let next = small_app("3.0.0", 900);
    for board in unconfirmed_boards() {
        let Unconfirmed {
            name,
            flash,
            running,
            ..
        } = board;
        let (mut ota, checks) = ota_on(&flash, running);
        assert!(ota.boot_state().unwrap().needs_confirmation, "{name}");
        let ops = flash.writes().len();
        assert_eq!(
            checks.run(ota.begin_update()).err(),
            Some(OtaError::Plan(PlanError::Unconfirmed)),
            "{name}"
        );
        assert_eq!(flash.writes().len(), ops, "{name}: nothing written");

        checks.run(ota.confirm()).unwrap();
        let state = ota.boot_state().unwrap();
        assert_eq!(state.selected, Some(running), "{name}");
        assert_eq!(state.state, Some(State::Valid), "{name}");
        assert!(!state.needs_confirmation, "{name}");
        confined(&flash, ops, running);
        assert_eq!(reboot(&flash), running, "{name}");
        assert_eq!(
            state_of(&flash, running).state,
            Some(State::Valid),
            "{name}"
        );

        recovers(&flash, running, &next, name);
    }
}

#[test]
fn a_cut_confirmation_boots_as_before() {
    let next = small_app("3.0.0", 900);
    let mut total = 0;
    for board in unconfirmed_boards() {
        let start = board.flash.copy();
        let boots_of = |flash: &RamNor| {
            let copy = flash.copy();
            [reboot(&copy), reboot(&copy)]
        };
        let before = boots_of(&start.copy());
        let mut whole = None;
        total += sweep(
            &start,
            board.running,
            |ota, checks| checks.run(ota.confirm()),
            |flash, case| {
                let case = format!("{}, {case}", board.name);
                confined(flash, 0, board.running);
                let boots = boots_of(flash);
                if whole.is_none() {
                    // The first check is the whole write's.
                    assert_eq!(boots, [board.running; 2], "{case}");
                    whole = Some(boots);
                } else {
                    assert_eq!(boots, before, "{case}");
                }
                assert!(holds(flash, board.running, &board.image), "{case}");
                // The next attempt confirms what boots and updates.
                let running = reboot(flash);
                let (mut ota, checks) = ota_on(flash, running);
                checks.run(ota.confirm()).unwrap();
                recovers(flash, running, &next, &case);
            },
        );
    }
    assert!(total > 200, "{total}");
}

// Rolling back.

#[test]
fn an_update_that_isnt_confirmed_rolls_back_at_the_next_boot() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &new, &mut Rng::new(5));
    assert_eq!(reboot(&flash), Ota0);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state().unwrap(),
        BootState {
            running: Ota0,
            selected: Some(Ota0),
            state: Some(State::Valid),
            rollback_target: false,
            needs_confirmation: false,
        }
    );
    let aborted = otadata_of(&flash)
        .entries
        .into_iter()
        .flatten()
        .find(|entry| entry.slot() == Ota1)
        .unwrap();
    assert_eq!(aborted.state, State::Aborted);
    assert_eq!(
        checks.run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    // The image is intact and can be activated again.
    let image = checks.run(ota.verify_other()).unwrap();
    checks.run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn rolling_back_starts_the_other_slot_at_the_next_boot() {
    let new = app_version("0.2.0");
    for confirmed in [false, true] {
        let flash = flashed(APP);
        update_and_boot(&flash, Ota0, &new, &mut Rng::new(6));
        let (mut ota, checks) = ota_on(&flash, Ota1);
        if confirmed {
            checks.run(ota.confirm()).unwrap();
        }
        let ops = flash.writes().len();
        let active = otadata_of(&flash).active().unwrap();
        checks.run(ota.roll_back()).unwrap();
        let at = OTADATA_AT + active.offset();
        assert_eq!(flash.writes()[ops..], entry_ops(at));
        assert_eq!(
            otadata_of(&flash).entry(active).unwrap().state,
            State::Invalid
        );
        assert_eq!(reboot(&flash), Ota0, "confirmed: {confirmed}");
        let (mut ota, checks) = ota_on(&flash, Ota0);
        assert_eq!(
            ota.boot_state().unwrap(),
            BootState {
                running: Ota0,
                selected: Some(Ota0),
                state: Some(State::Valid),
                rollback_target: false,
                needs_confirmation: false,
            }
        );
        assert_eq!(
            checks.run(ota.roll_back()),
            Err(OtaError::Plan(PlanError::NoRollbackTarget))
        );
        assert_eq!(reboot(&flash), Ota0);
    }
}

#[test]
fn rolling_back_needs_a_target() {
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    assert_eq!(
        checks.run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    // An image in the other slot that no entry selects: the refusal doesn't
    // wait for the slot's check.
    flash.poke(OTA_1.offset, &app_version("0.2.0"));
    let (mut ota, _) = ota_on(&flash, Ota0);
    let done = block_on(ota.roll_back());
    assert_eq!(
        done.output,
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    assert_eq!(done.wakes, 0);
    assert_eq!(flash.writes(), []);
}

#[test]
fn rolling_back_needs_an_image_that_verifies() {
    let flash = updated(APP, &app_version("0.2.0"));
    let ops = flash.writes().len();
    // A byte of the old image decays.
    flash.poke(OTA_0.offset + 0x9000, &[!APP[0x9000]]);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    assert!(matches!(
        checks.run(ota.roll_back()),
        Err(OtaError::Image(_))
    ));
    // Another project's image.
    flash.poke(OTA_0.offset, &app_project("another"));
    let (mut ota, checks) = ota_on(&flash, Ota1);
    assert_eq!(checks.run(ota.roll_back()), Err(OtaError::Project));
    assert_eq!(flash.writes().len(), ops, "nothing written");
    assert_eq!(reboot(&flash), Ota1);
    // Restored, it rolls back.
    flash.poke(OTA_0.offset, APP);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    checks.run(ota.roll_back()).unwrap();
    assert_eq!(reboot(&flash), Ota0);
}

// Protection.

#[test]
fn no_operation_writes_outside_the_other_slot_and_the_otadata() {
    let new = app_version("0.2.0");
    let newer = app_version("0.3.0");
    let ota_0_board = flashed(APP);
    let ota_1_board = updated(APP, &new);
    for (flash, running, running_image) in
        [(&ota_0_board, Ota0, APP), (&ota_1_board, Ota1, &new[..])]
    {
        let from = flash.writes().len();
        let mut rng = Rng::new(7);
        let (mut ota, checks) = ota_on(flash, running);
        // Refusals.
        assert!(finish_with(&mut ota, checks, &app_project("another")).is_err());
        assert!(finish_with(&mut ota, checks, &newer[..1000]).is_err());
        checks.run(async {
            let mut update = ota.begin_update().await.unwrap();
            let slot = OTA_0.size as usize;
            assert!(update.write(&vec![0; slot + 1]).await.is_err());
        });
        let _ = checks.run(ota.roll_back());
        // An update, cancelled, and another, activated.
        let image = checks
            .run(update(&mut ota, &newer, &mut rng, 30_000))
            .unwrap();
        checks.run(ota.activate(image)).unwrap();
        let image = checks
            .run(update(&mut ota, &newer, &mut rng, 30_000))
            .unwrap();
        checks.run(ota.activate(image)).unwrap();
        let _ = checks.run(ota.confirm());
        confined(flash, from, running);
        assert!(holds(flash, running, running_image));
        // Booted, confirmed and rolled back from the new slot.
        assert_eq!(reboot(flash), running.other());
        let from = flash.writes().len();
        let (mut ota, checks) = ota_on(flash, running.other());
        checks.run(ota.confirm()).unwrap();
        checks.run(ota.roll_back()).unwrap();
        confined(flash, from, running.other());
        assert!(holds(flash, running.other(), &newer));
        assert_eq!(reboot(flash), running);
    }
}

#[test]
fn the_guard_refuses_what_a_wrong_span_would_write() {
    let flash = TestFlash::new(flashed(APP));
    let device = flash.device();
    // The running slot in place of the other one.
    let (mut ota, checks) = ota_on_spans(&flash, Span::from(OTA_0), Span::from(OTADATA));
    assert_eq!(
        checks.run(ota.begin_update()).err(),
        Some(OtaError::Access(AccessError::Protected))
    );
    // The boot chain in place of the otadata, holding no valid entry: a
    // confirmation would write its first sector.
    let (mut ota, checks) = ota_on_spans(&flash, Span::from(OTA_1), Span::new(0, OTADATA.size));
    assert_eq!(
        checks.run(ota.confirm()),
        Err(OtaError::Access(AccessError::Protected))
    );
    assert!(holds(device, Ota0, APP));
    assert_eq!(device.at(0, BOOTLOADER.len()), BOOTLOADER);
    // Only the real otadata was written.
    confined(device, 0, Ota0);
    assert!(
        device
            .writes()
            .iter()
            .all(|op| op_span(*op).0 >= OTADATA.offset && op_span(*op).1 <= OTADATA.end())
    );
}

// Sessions and epochs.

/// The OTA mechanism of a board flashed with the firmware's image in `ota_0`,
/// booted from it, over writers made without `split`, with the checks of its
/// slots and readers of the alternate slot and the otadata over the same
/// state table.
fn watched() -> (
    TestFlash,
    Ota<'static>,
    Checks<'static>,
    Reader<'static, AlternateApp>,
    Reader<'static, OtadataRole>,
) {
    let flash = TestFlash::new(flashed(APP));
    let (ota, checks) = ota_on_spans(&flash, Span::from(OTA_1), Span::from(OTADATA));
    let alternate = flash.reader(Span::from(OTA_1));
    let otadata = flash.reader(Span::from(OTADATA));
    (flash, ota, checks, alternate, otadata)
}

/// Polls `future` once with a waker that does nothing.
fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn an_update_holds_the_slot_s_session_for_its_life() {
    let (_flash, mut ota, checks, alternate, otadata) = watched();
    let new = small_app("0.2.0", 0);
    let before = alternate.write_state();
    checks.run(async {
        let mut update = ota.begin_update().await.unwrap();
        // The slot is written to from the start of the update: no snapshot
        // of it can be taken, and its state is the one it started from.
        assert_eq!(
            alternate.snapshot(Span::new(0, 4)).err(),
            Some(SnapshotError::Busy)
        );
        assert_eq!(alternate.write_state(), WriteState::Writing(before.epoch()));
        // The otadata isn't: its writes are sessions of their own.
        assert!(otadata.write_state().is_stable());
        update.write(&new).await.unwrap();
        assert!(!alternate.write_state().is_stable());
        let image = update.finish().await.unwrap();
        // Finished, the slot is stable at a new epoch, which the image
        // records.
        let after = alternate.write_state();
        assert_eq!(after, WriteState::Stable(image.epoch()));
        assert_ne!(after.epoch(), before.epoch());
        assert!(alternate.snapshot(Span::new(0, 4)).is_ok());
    });
}

#[test]
fn an_abandoned_update_ends_its_session() {
    let (_flash, mut ota, checks, alternate, _) = watched();
    let before = alternate.write_state();
    checks.run(async {
        let mut update = ota.begin_update().await.unwrap();
        update.write(&[0; 100]).await.unwrap();
        drop(update);
    });
    let after = alternate.write_state();
    assert!(after.is_stable());
    assert_ne!(after, before);
    // A failed finish ends it too.
    let image = finish_with(&mut ota, checks, &[0; 64]);
    assert!(image.is_err());
    assert!(alternate.write_state().is_stable());
}

#[test]
fn each_otadata_write_is_its_own_session() {
    let (_flash, mut ota, checks, alternate, otadata) = watched();
    let image = finish_with(&mut ota, checks, &small_app("0.2.0", 0)).unwrap();
    let before = otadata.write_state();
    let slot = alternate.write_state();
    checks.run(ota.activate(image)).unwrap();
    // One write, one session: a new epoch, and nothing open after it.
    let after = otadata.write_state();
    assert!(after.is_stable());
    assert_ne!(after, before);
    assert_eq!(alternate.write_state(), slot);
}

#[test]
fn finishing_waits_for_the_slot_s_check_at_the_new_epoch() {
    let (flash, mut ota, checks, alternate, _) = watched();
    let new = small_app("0.2.0", 1);
    checks.settle();
    let before = alternate.write_state();
    let mut update = checks.run(ota.begin_update()).unwrap();
    checks.run(update.write(&new)).unwrap();
    let calls = flash.device().calls().len();
    let mut finish = pin!(update.finish());
    // With no check running, the session ends at a new epoch, and the update
    // waits.
    for _ in 0..100 {
        assert!(poll_once(finish.as_mut()).is_pending());
    }
    let WriteState::Stable(epoch) = alternate.write_state() else {
        panic!("the session is open");
    };
    assert_ne!(epoch, before.epoch());
    // The update itself reads nothing: it hashes nothing.
    assert_eq!(flash.device().calls()[calls..], []);
    // Once the check keeps its result for the epoch, the update takes it.
    checks.settle();
    let Poll::Ready(image) = poll_once(finish.as_mut()) else {
        panic!("no verdict");
    };
    let image = image.unwrap();
    let model = checks.alternate().model().unwrap();
    assert_eq!(image.epoch(), epoch);
    assert_eq!(model.epoch(), epoch);
    assert_eq!(image.digest(), model.image().unwrap().digest());
    assert_eq!(image.size() as usize, new.len());
}

#[test]
fn a_slot_written_before_its_check_keeps_a_result_fails_the_update() {
    let (flash, mut ota, checks, alternate, _) = watched();
    let new = small_app("0.2.0", 0);
    let mut update = checks.run(ota.begin_update()).unwrap();
    checks.run(update.write(&new)).unwrap();
    let mut finish = pin!(update.finish());
    while !alternate.write_state().is_stable() {
        assert!(poll_once(finish.as_mut()).is_pending());
    }
    assert!(poll_once(finish.as_mut()).is_pending());
    // Another writer of the slot, which no boot has, writes it before the
    // check runs: the epoch the update waits for is gone.
    let mut other = flash.writer::<AlternateApp>(Span::from(OTA_1), Ota0);
    run(other.begin().erase(OTA_1.size - SECTOR_SIZE, OTA_1.size)).unwrap();
    assert_eq!(
        poll_once(finish.as_mut()),
        Poll::Ready(Err(OtaError::Changed))
    );
}

#[test]
fn an_image_whose_slot_was_written_since_isnt_activated() {
    let (flash, mut ota, checks, alternate, _) = watched();
    let image = finish_with(&mut ota, checks, &small_app("0.2.0", 0)).unwrap();
    assert_eq!(alternate.write_state(), WriteState::Stable(image.epoch()));
    // Another writer of the slot, which no boot has, writes it: the slot's
    // epoch moves, though it still holds the image.
    let mut other = flash.writer::<AlternateApp>(Span::from(OTA_1), Ota0);
    run(other.begin().erase(OTA_1.size - SECTOR_SIZE, OTA_1.size)).unwrap();
    assert_ne!(alternate.write_state(), WriteState::Stable(image.epoch()));
    let writes = flash.device().writes().len();
    assert_eq!(checks.run(ota.activate(image)), Err(OtaError::Changed));
    assert_eq!(flash.device().writes().len(), writes, "nothing written");
    // Verified again at the new epoch, it is.
    let again = checks.run(ota.verify_other()).unwrap();
    assert_eq!(alternate.write_state(), WriteState::Stable(again.epoch()));
    checks.run(ota.activate(again)).unwrap();
}

#[test]
fn activating_reads_nothing_of_the_slot() {
    let (flash, mut ota, checks, _, _) = watched();
    let image = finish_with(&mut ota, checks, &small_app("0.2.0", 0)).unwrap();
    let calls = flash.device().calls().len();
    // Without the checks: activating waits for nothing.
    assert_eq!(block_on(ota.activate(image)).output, Ok(()));
    let calls = &flash.device().calls()[calls..];
    // The otadata is read for the plan, then written; the slot isn't
    // touched.
    let otadata = OTADATA.offset..OTADATA.end();
    assert!(
        calls.iter().all(|call| match *call {
            Call::Read(at, len) | Call::Program(at, len) => {
                otadata.contains(&at) && otadata.contains(&(at + len - 1))
            }
            Call::Erase(at) => otadata.contains(&at),
        }),
        "{calls:?}"
    );
    assert!(calls.iter().any(|call| matches!(call, Call::Read(..))));
}

// Readiness.

#[test]
fn ota_is_unavailable_until_the_running_app_s_descriptor_reads() {
    let mut broken = APP.to_vec();
    broken[0x1000] ^= 1;
    for current in [None, Some(&broken[..])] {
        let device = RamNor::with(
            FLASH_SIZE,
            &[(OTADATA_AT, OTADATA_FIRST_BOOT), (OTA_1.offset, APP)],
        );
        if let Some(image) = current {
            device.poke(OTA_0.offset, image);
        }
        let flash = TestFlash::new(device);
        let (mut ota, checks) = ota_on_spans(&flash, Span::from(OTA_1), Span::from(OTADATA));
        let reason = ota.readiness().unwrap_err();
        assert!(matches!(reason, Unavailable::Descriptor(_)), "{reason:?}");
        if current.is_none() {
            assert_eq!(reason, Unavailable::Descriptor(ImageFailure::Magic));
        }
        assert_eq!(ota.project(), Err(reason));
        // Every write is refused, before anything is read or written.
        let refused = OtaError::Unavailable(reason);
        assert_eq!(checks.run(ota.begin_update()).err(), Some(refused));
        assert_eq!(checks.run(ota.verify_other()).err(), Some(refused));
        assert_eq!(checks.run(ota.confirm()).err(), Some(refused));
        assert_eq!(checks.run(ota.roll_back()).err(), Some(refused));
        assert_eq!(flash.device().writes(), []);
        // The boot state still reads.
        assert_eq!(ota.boot_state().unwrap().selected, Some(Ota0));
    }
    // With the image there, OTA is ready, and its project is the one updates
    // match.
    let flash = flashed(APP);
    let (ota, _) = ota_on(&flash, Ota0);
    assert_eq!(ota.readiness(), Ok(()));
    assert_eq!(
        ota.project().map(|project| project.to_str()),
        Ok(Some("eveningstar"))
    );
}

// Yields.

#[test]
fn long_operations_yield_to_the_executor() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let (mut ota, checks) = ota_on(&flash, Ota0);
    let image = checks
        .run(async {
            // One otadata write, an erase and three programs, and the slot's
            // first sector.
            let begin = block_on(ota.begin_update());
            assert_eq!(begin.wakes, 5);
            let mut update = begin.output.unwrap();
            // 16 sectors: 15 erased, as the first is, and 16 programmed.
            assert_eq!(block_on(update.write(&new[..0x1_0000])).wakes, 31);
            // Two words into the next sector, which is erased first; two
            // bytes held.
            assert_eq!(block_on(update.write(&new[0x1_0000..0x1_000A])).wakes, 2);
            // Only held.
            assert_eq!(block_on(update.write(&new[0x1_000A..0x1_000B])).wakes, 0);
            update.write(&new[0x1_000B..]).await.unwrap();
            update.finish().await
        })
        .unwrap();
    // One otadata write: the slot isn't hashed again.
    let activate = block_on(ota.activate(image));
    assert_eq!(activate.output, Ok(()));
    assert_eq!(activate.wakes, 4);
    // The check's result is kept: nothing to wait for or hash.
    assert_eq!(block_on(ota.verify_other()).wakes, 0);

    assert_eq!(reboot(&flash), Ota1);
    let (mut ota, checks) = ota_on(&flash, Ota1);
    checks.settle();
    assert_eq!(block_on(ota.confirm()).wakes, 4);
    // One otadata write, and the wake its session's end gives the task the
    // slot's verdict registered to wait for the slot's change.
    let roll_back = block_on(ota.roll_back());
    assert_eq!(roll_back.output, Ok(()));
    assert_eq!(roll_back.wakes, 4 + 1);
}

// Power cuts.

/// The ways a cut leaves an erase or program. An otadata program: every
/// prefix (every word of the label and state), a few bits short of whole and
/// random subsets of its bits. A slot's program: nothing, half, one bit short
/// and a random subset. An erase lands whole or not at all.
fn tears(op: Call) -> Vec<Tear> {
    let bits = |seeds: u64| (0..seeds).map(|seed| Tear::Bits(0xB175 + seed));
    match op {
        Call::Erase(_) => vec![Tear::Prefix(0)],
        Call::Program(address, len) if address < OTA_0.offset => {
            let step = if len > 4 { 4 } else { 1 };
            (0..len as usize)
                .step_by(step)
                .map(Tear::Prefix)
                .chain([0, 7, 30].map(Tear::AllBut))
                .chain(bits(6))
                .collect()
        }
        Call::Program(_, len) => [Tear::Prefix(0), Tear::Prefix(len as usize / 2 / 4 * 4)]
            .into_iter()
            .chain([Tear::AllBut(0)])
            .chain(bits(1))
            .collect(),
        Call::Read(..) => panic!("{op:?} isn't a write"),
    }
}

/// Runs `flow` on a copy of the board `start`, running from `running`,
/// whole, then cut at each of its erases and programs in each way [`tears`]
/// gives, the power coming back after the cut. Hands `check` each board,
/// with a description of the cut, and returns the number of cuts.
fn sweep(
    start: &RamNor,
    running: Slot,
    flow: impl Fn(&mut Ota<'_>, Checks<'_>) -> Result<(), OtaError>,
    mut check: impl FnMut(&RamNor, &str),
) -> usize {
    let whole = start.copy();
    let (mut ota, checks) = ota_on(&whole, running);
    flow(&mut ota, checks).unwrap();
    let ops = whole.writes();
    check(&whole, "whole");
    let mut cuts = 0;
    for (index, op) in ops.into_iter().enumerate() {
        for tear in tears(op) {
            let flash = start.copy();
            flash.cut_after(index, tear);
            let case = format!("cut at {index}, {op:?}, {tear:?}");
            let (mut ota, checks) = ota_on(&flash, running);
            assert_eq!(
                flow(&mut ota, checks),
                Err(OtaError::Access(AccessError::Device)),
                "{case}"
            );
            flash.heal();
            check(&flash, &case);
            cuts += 1;
        }
    }
    cuts
}

/// Boots the board twice, confirming nothing. The first boot starts `old`
/// from `running`, or a whole image of `pending` from the other slot as
/// `PendingVerify`; the second starts `old`. Returns the first boot's slot.
fn boots_back(flash: &RamNor, running: Slot, old: &[u8], pending: &[&[u8]], case: &str) -> Slot {
    let first = boot_device(flash).unwrap_or_else(|| panic!("{case}: nothing boots"));
    if first == running {
        assert!(holds(flash, running, old), "{case}");
    } else {
        assert!(
            pending.iter().any(|image| holds(flash, first, image)),
            "{case}: a broken image boots"
        );
        assert_eq!(
            state_of(flash, first).state,
            Some(State::PendingVerify),
            "{case}"
        );
    }
    assert_eq!(boot_device(flash), Some(running), "{case}");
    assert!(holds(flash, running, old), "{case}");
    first
}

/// The next attempt on the board running from `running`: an update to
/// `image`, activated, booted once and confirmed.
fn recovers(flash: &RamNor, running: Slot, image: &[u8], case: &str) {
    let (mut ota, checks) = ota_on(flash, running);
    let verified = checks
        .run(update(&mut ota, image, &mut Rng::new(9), 3000))
        .unwrap_or_else(|error| panic!("{case}: {error:?}"));
    checks.run(ota.activate(verified)).unwrap();
    assert_eq!(boot_device(flash), Some(running.other()), "{case}");
    let (mut ota, checks) = ota_on(flash, running.other());
    assert_eq!(
        ota.boot_state().unwrap().state,
        Some(State::PendingVerify),
        "{case}"
    );
    checks.run(ota.confirm()).unwrap();
    assert_eq!(boot_device(flash), Some(running.other()), "{case}");
    assert!(holds(flash, running.other(), image), "{case}");
}

/// An update to `image` in parts of up to `max` bytes, from a fixed seed,
/// activated.
fn update_flow(
    image: &[u8],
    max: usize,
) -> impl Fn(&mut Ota<'_>, Checks<'_>) -> Result<(), OtaError> + '_ {
    move |ota, checks| {
        checks.run(async {
            let verified = update(ota, image, &mut Rng::new(0xC07), max).await?;
            ota.activate(verified).await
        })
    }
}

/// A board an update starts from.
struct Start {
    name: &'static str,
    flash: RamNor,
    /// The slot it runs `old` from.
    running: Slot,
    /// An image that may boot pending before the update takes the other
    /// slot out of the selection.
    activated: Option<Vec<u8>>,
}

/// The boards an update starts from, running `old`, with `other` in the
/// other slot in some of them.
fn update_starts(old: &[u8], other: &[u8]) -> [Start; 4] {
    // An activation of `other` not yet booted.
    let activated = flashed(old);
    let (mut ota, checks) = ota_on(&activated, Ota0);
    let image = finish_with(&mut ota, checks, other).unwrap();
    checks.run(ota.activate(image)).unwrap();
    // A fallback from `other`, which broke after its activation, confirmed.
    let fallback = activated.copy();
    fallback.poke(OTA_1.offset + 0x40, &[!other[0x40]]);
    assert_eq!(reboot(&fallback), Ota0);
    confirm_on(&fallback, Ota0);
    [
        Start {
            name: "first boot",
            flash: flashed(old),
            running: Ota0,
            activated: None,
        },
        Start {
            name: "rollback target",
            flash: updated(other, old),
            running: Ota1,
            activated: None,
        },
        Start {
            name: "activated",
            flash: activated.copy(),
            running: Ota0,
            activated: Some(other.to_vec()),
        },
        Start {
            name: "confirmed fallback",
            flash: fallback,
            running: Ota0,
            activated: None,
        },
    ]
}

#[test]
fn a_cut_update_boots_the_old_image_or_the_new_one_once() {
    let old = small_app("1.0.0", 0);
    let other = small_app("1.5.0", 700);
    let new = small_app("2.0.0", 1500);
    let next = small_app("3.0.0", 100);
    let mut cuts = 0;
    let mut booted_new = 0;
    let mut booted_other = 0;
    for start in update_starts(&old, &other) {
        let running = start.running;
        let mut pending: Vec<&[u8]> = start.activated.iter().map(Vec::as_slice).collect();
        pending.push(&new);
        cuts += sweep(
            &start.flash,
            running,
            update_flow(&new, 3000),
            |flash, case| {
                let case = format!("{}, {case}", start.name);
                confined(flash, 0, running);
                let first = boots_back(flash, running, &old, &pending, &case);
                if first != running && holds(flash, first, &new) {
                    booted_new += 1;
                }
                if first != running && holds(flash, first, &other) {
                    booted_other += 1;
                }
                recovers(flash, running, &next, &case);
            },
        );
    }
    assert!(cuts > 400, "{cuts}");
    // Only the whole flow, once per start, boots the new image.
    assert_eq!(booted_new, 4);
    // The activation not yet booted boots only when nothing of its
    // cancellation lands: the cut at its erase.
    assert_eq!(booted_other, 1);
}

#[test]
fn a_cut_update_of_the_firmware_image_boots_the_old_image_or_the_new_one_once() {
    let new = app_version("0.2.0");
    let next = small_app("3.0.0", 0);
    let start = flashed(APP);
    let cuts = sweep(&start, Ota0, update_flow(&new, 0x1_0000), |flash, case| {
        confined(flash, 0, Ota0);
        boots_back(flash, Ota0, APP, &[&new], case);
        recovers(flash, Ota0, &next, case);
    });
    assert!(cuts > 200, "{cuts}");
}

#[test]
fn a_cut_confirmation_boots_the_new_image_confirmed_or_the_old_one() {
    let old = small_app("1.0.0", 0);
    let new = small_app("2.0.0", 900);
    let next = small_app("3.0.0", 100);
    let start = flashed(&old);
    update_and_boot(&start, Ota0, &new, &mut Rng::new(10));
    let mut outcomes = [0; 2];
    let cuts = sweep(
        &start,
        Ota1,
        |ota, checks| checks.run(ota.confirm()),
        |flash, case| {
            confined(flash, 0, Ota1);
            let first = reboot(flash);
            let image = if first == Ota1 { &new } else { &old };
            assert!(holds(flash, first, image), "{case}");
            assert_eq!(state_of(flash, first).state, Some(State::Valid), "{case}");
            assert_eq!(reboot(flash), first, "{case}");
            outcomes[first.number() as usize] += 1;
            recovers(flash, first, &next, case);
        },
    );
    assert!(cuts > 30, "{cuts}");
    // Only the whole entry confirms.
    assert_eq!(outcomes, [cuts, 1]);
}

#[test]
fn a_cut_roll_back_boots_one_of_the_two_images() {
    let old = small_app("1.0.0", 0);
    let new = small_app("2.0.0", 900);
    let next = small_app("3.0.0", 100);
    let start = updated(&old, &new);
    let mut outcomes = [0; 2];
    let cuts = sweep(
        &start,
        Ota1,
        |ota, checks| checks.run(ota.roll_back()),
        |flash, case| {
            confined(flash, 0, Ota1);
            let first = reboot(flash);
            let image = if first == Ota1 { &new } else { &old };
            assert!(holds(flash, first, image), "{case}");
            assert_eq!(state_of(flash, first).state, Some(State::Valid), "{case}");
            assert_eq!(reboot(flash), first, "{case}");
            outcomes[first.number() as usize] += 1;
            recovers(flash, first, &next, case);
        },
    );
    assert!(cuts > 30, "{cuts}");
    // The erase alone already rolls back; only a cut before it doesn't.
    assert_eq!(outcomes, [cuts, 1]);
}

// A long run of random operations.

/// A board running the simulated bootloader, with the operations of an OTA
/// service on top of [`Ota`], and power cuts.
struct Board<'a> {
    flash: &'a RamNor,
    images: &'a [Vec<u8>],
    ota: Ota<'a>,
    checks: Checks<'a>,
    running: Slot,
    /// The image the running slot holds, by index in `images`.
    running_image: usize,
    /// Whether each slot's image was confirmed since it was written: the
    /// image `flash-all` wrote, or one `confirm` confirmed, and nothing else.
    confirmed: [bool; 2],
    /// A finished image not yet activated.
    held: Option<VerifiedImage<'a>>,
    /// An update began after `held` was made.
    stale: bool,
    /// A byte of the other slot changed outside a write session since the
    /// last boot or write session on the slot, so the result its check kept
    /// may be of the bytes before.
    decayed: bool,
    /// The erases and programs checked so far.
    checked: usize,
    rng: Rng,
    stats: Stats,
    log: Vec<String>,
}

#[derive(Debug, Default)]
struct Stats {
    boots: usize,
    cuts: usize,
    activated: usize,
    updates_booted: usize,
    confirmed: usize,
    rolled_back: usize,
    aborted: usize,
    fallbacks: usize,
    stale: usize,
    refused: usize,
    unconfirmed: usize,
}

impl<'a> Board<'a> {
    fn new(flash: &'a RamNor, images: &'a [Vec<u8>], seed: u64) -> Self {
        let (ota, checks) = ota_on(flash, Ota0);
        Self {
            flash,
            images,
            ota,
            checks,
            running: Ota0,
            running_image: 0,
            confirmed: [true, false],
            held: None,
            stale: false,
            decayed: false,
            checked: flash.writes().len(),
            rng: Rng::new(seed),
            stats: Stats::default(),
            log: Vec::new(),
        }
    }

    fn other(&self) -> usize {
        self.running.other().number() as usize
    }

    /// The last lines of the log, for a failure.
    fn trail(&self) -> String {
        self.log[self.log.len().saturating_sub(30)..].join("\n")
    }

    /// The image `slot` holds whole, by index.
    fn image_in(&self, slot: Slot) -> Option<usize> {
        self.images
            .iter()
            .position(|image| holds(self.flash, slot, image))
    }

    /// Arms a power cut somewhere in the next operation, now and then.
    fn maybe_cut(&mut self) {
        if self.rng.below(4) == 0 {
            let at = self.rng.below(24);
            let tear = match self.rng.below(3) {
                // Short of the 4 bytes of the shortest program, so the cut
                // program never lands whole.
                0 => Tear::Prefix(self.rng.below(4)),
                1 => Tear::Bits(self.rng.next_u64()),
                _ => Tear::AllBut(self.rng.below(256)),
            };
            self.flash.cut_after(at, tear);
        }
    }

    /// Checks the erases and programs since the last check, and that the
    /// running image is intact and the board boots.
    fn check(&mut self) {
        let ops = self.flash.writes();
        for op in &ops[self.checked..] {
            if *op == Call::Erase(self.running.other().partition().offset) {
                let other = self.other();
                self.confirmed[other] = false;
            }
        }
        confined(self.flash, self.checked, self.running);
        self.checked = ops.len();
        // Only `confirm` makes an image confirmed.
        if !self.ota.boot_state().unwrap().needs_confirmation {
            assert!(
                self.confirmed[self.running.number() as usize],
                "an image became valid without a confirmation\n{}",
                self.trail()
            );
        }
        assert_eq!(
            self.image_in(self.running),
            Some(self.running_image),
            "{}",
            self.trail()
        );
        let loadable = Slot::ALL.map(|slot| self.image_in(slot).is_some());
        let mut otadata = self.flash.at(OTADATA_AT, OTADATA_LEN);
        assert!(boot(&mut otadata, loadable).is_some(), "{}", self.trail());
    }

    /// Ends an operation: a cut one loses the power, and the board boots
    /// again.
    fn end(&mut self, result: Result<(), OtaError>) -> Result<(), OtaError> {
        self.flash.heal();
        self.log.push(format!("  {result:?}"));
        if result == Err(OtaError::Access(AccessError::Device)) {
            self.stats.cuts += 1;
            self.reboot();
        }
        result
    }

    fn reboot(&mut self) {
        self.check();
        let before = self.running;
        let pending = state_of(self.flash, before).state == Some(State::PendingVerify);
        let running = boot_device(self.flash).unwrap_or_else(|| panic!("{}", self.trail()));
        self.stats.boots += 1;
        let image = self
            .image_in(running)
            .unwrap_or_else(|| panic!("a broken image boots\n{}", self.trail()));
        let state = state_of(self.flash, running);
        self.log.push(format!(
            "  boot {running:?} image {image} {:?} confirmed {:?}",
            otadata_of(self.flash)
                .entries
                .map(|entry| entry.map(|entry| (entry.seq, entry.state))),
            self.confirmed
        ));
        assert_ne!(state.state, Some(State::New));
        match state.selected {
            Some(selected) if selected != running => {
                // A fallback, only from a slot that doesn't hold a whole
                // image: here, one whose image decayed. It needs a
                // confirmation before anything is updated.
                assert_eq!(self.image_in(selected), None, "{}", self.trail());
                assert!(state.needs_confirmation, "{}", self.trail());
                self.stats.fallbacks += 1;
            }
            _ if !self.confirmed[running.number() as usize] => {
                // An image nobody confirmed runs only with its rollback
                // armed.
                assert_eq!(state.state, Some(State::PendingVerify), "{}", self.trail());
                self.stats.updates_booted += 1;
            }
            _ => {}
        }
        if pending && running != before {
            self.stats.aborted += 1;
        }
        self.running = running;
        self.running_image = image;
        (self.ota, self.checks) = ota_on(self.flash, running);
        self.held = None;
        self.decayed = false;
        self.checked = self.flash.writes().len();
    }

    /// Writes image `index` as an update, `corrupt` with a byte changed, and
    /// finishes it; or abandons it after `stop` bytes.
    fn write(
        &mut self,
        index: usize,
        corrupt: bool,
        stop: Option<usize>,
    ) -> Result<Option<VerifiedImage<'a>>, OtaError> {
        let mut image = self.images[index].clone();
        if corrupt {
            let at = self.rng.below(image.len());
            image[at] ^= 1 << self.rng.below(8);
        }
        let parts = parts(&image, &mut self.rng, 5000);
        let ota = &mut self.ota;
        let result = self.checks.run(async {
            let mut update = ota.begin_update().await?;
            for part in parts {
                if stop.is_some_and(|stop| update.written() as usize >= stop) {
                    return Ok(None);
                }
                update.write(part).await?;
            }
            update.finish().await.map(Some)
        });
        if !matches!(result, Err(OtaError::Plan(_))) {
            self.stale = true;
            self.decayed = false;
        }
        result
    }

    fn update(&mut self) {
        let index = self.rng.below(self.images.len());
        self.maybe_cut();
        let unconfirmed = self.ota.boot_state().unwrap().needs_confirmation;
        let result = match self.write(index, false, None) {
            Ok(image) => {
                let image = image.unwrap();
                assert_eq!(image.size() as usize, self.images[index].len());
                self.checks.run(self.ota.activate(image))
            }
            Err(error) => Err(error),
        };
        if result.is_ok() {
            self.stats.activated += 1;
            assert_eq!(
                self.ota.boot_state().unwrap().selected,
                Some(self.running.other())
            );
        }
        assert_eq!(
            result == Err(OtaError::Plan(PlanError::Unconfirmed)),
            unconfirmed,
            "{}",
            self.trail()
        );
        if unconfirmed {
            self.stats.unconfirmed += 1;
        }
        let _ = self.end(result);
    }

    fn abandon(&mut self) {
        let index = self.rng.below(self.images.len());
        let stop = self.rng.below(self.images[index].len());
        self.maybe_cut();
        let result = self.write(index, false, Some(stop)).map(|_| ());
        let _ = self.end(result);
    }

    fn corrupt(&mut self) {
        let index = self.rng.below(self.images.len());
        self.maybe_cut();
        let result = self.write(index, true, None);
        assert!(
            result.is_err(),
            "a changed image verifies\n{}",
            self.trail()
        );
        if matches!(result, Err(OtaError::Image(_) | OtaError::Length { .. })) {
            self.stats.refused += 1;
        }
        let _ = self.end(result.map(|_| ()));
    }

    fn finish(&mut self) {
        let index = self.rng.below(self.images.len());
        self.maybe_cut();
        let result = self.write(index, false, None);
        let ended = result.as_ref().map(|_| ()).map_err(|error| *error);
        if let Ok(image) = result {
            self.held = image;
            self.stale = false;
        }
        let _ = self.end(ended);
    }

    fn activate_held(&mut self) {
        let Some(image) = self.held.take() else {
            return;
        };
        self.maybe_cut();
        let result = self.checks.run(self.ota.activate(image));
        match result {
            Err(OtaError::Stale) => {
                assert!(self.stale, "{}", self.trail());
                self.stats.stale += 1;
            }
            // Only a write session moves the slot's epoch, and every one
            // begins an update, which makes the image stale first.
            Err(OtaError::Changed) => panic!("the slot's epoch moved\n{}", self.trail()),
            // A decayed image is activated too: the bootloader falls back.
            Ok(()) => {
                assert!(!self.stale, "{}", self.trail());
                self.stats.activated += 1;
            }
            _ => {}
        }
        let _ = self.end(result);
    }

    fn switch_back(&mut self) {
        self.maybe_cut();
        let result = match self.checks.run(self.ota.verify_other()) {
            Ok(image) => {
                assert!(
                    self.image_in(self.running.other()).is_some() || self.decayed,
                    "{}",
                    self.trail()
                );
                self.checks.run(self.ota.activate(image))
            }
            Err(error) => Err(error),
        };
        let _ = self.end(result);
    }

    fn confirm(&mut self) {
        let state = self.ota.boot_state().unwrap();
        self.maybe_cut();
        let result = self.checks.run(self.ota.confirm());
        if result.is_ok() {
            let after = self.ota.boot_state().unwrap();
            assert!(!after.needs_confirmation, "{}", self.trail());
            if state.needs_confirmation {
                assert_eq!(after.selected, Some(self.running), "{}", self.trail());
                self.confirmed[self.running.number() as usize] = true;
                self.stats.confirmed += 1;
            }
        }
        let _ = self.end(result);
    }

    fn roll_back(&mut self) {
        let state = self.ota.boot_state().unwrap();
        let target = self.image_in(self.running.other());
        self.maybe_cut();
        let result = self.checks.run(self.ota.roll_back());
        if result.is_ok() {
            assert!(
                state.rollback_target && (target.is_some() || self.decayed),
                "{}",
                self.trail()
            );
        }
        if result == Err(OtaError::Plan(PlanError::NoRollbackTarget)) {
            assert!(!state.rollback_target);
        }
        if self.end(result).is_ok() {
            self.reboot();
            // A decayed target doesn't load, and the bootloader falls back.
            if let Some(target) = target {
                assert_eq!(self.running_image, target, "{}", self.trail());
                self.stats.rolled_back += 1;
            }
        }
    }

    /// A byte of the other slot's image changes on the flash.
    fn decay(&mut self) {
        if let Some(index) = self.image_in(self.running.other()) {
            let at = self.rng.below(self.images[index].len());
            let address = self.running.other().partition().offset + at as u32;
            let byte = self.flash.at(address, 1)[0];
            self.flash.poke(address, &[byte ^ 0x01]);
            self.decayed = true;
        }
    }

    fn step(&mut self) {
        let op = self.rng.below(16);
        self.log.push(format!(
            "op {op} running {:?} image {} {:?}",
            self.running,
            self.running_image,
            self.ota.boot_state().unwrap()
        ));
        match op {
            0..=3 => self.update(),
            4 => self.abandon(),
            5 => self.corrupt(),
            6 => self.finish(),
            7 => self.activate_held(),
            8 => self.switch_back(),
            9..=11 => self.confirm(),
            12 => self.roll_back(),
            13 if self.rng.below(8) == 0 => self.decay(),
            _ => self.reboot(),
        }
        self.check();
    }
}

#[test]
fn random_updates_confirmations_rollbacks_and_power_cuts_keep_the_board_booting() {
    let images: Vec<Vec<u8>> = [(0, "1.0.0"), (300, "1.1.0"), (1100, "1.2.0"), (40, "1.3.0")]
        .into_iter()
        .map(|(extra, version)| small_app(version, extra))
        .collect();
    // Odd seeds: the generator sets the lowest bit.
    let mut fallbacks = 0;
    for seed in (1..40).step_by(2) {
        let flash = flashed(&images[0]);
        let mut board = Board::new(&flash, &images, seed);
        for _ in 0..3000 {
            board.step();
        }
        let stats = &board.stats;
        assert!(stats.cuts > 150, "{seed}: {stats:?}");
        assert!(stats.updates_booted > 100, "{seed}: {stats:?}");
        assert!(stats.confirmed > 30, "{seed}: {stats:?}");
        assert!(stats.rolled_back > 10, "{seed}: {stats:?}");
        assert!(stats.aborted > 30, "{seed}: {stats:?}");
        assert!(stats.stale > 1, "{seed}: {stats:?}");
        assert!(stats.refused > 80, "{seed}: {stats:?}");
        assert!(stats.unconfirmed > 50, "{seed}: {stats:?}");
        fallbacks += stats.fallbacks;
    }
    // Rare: a decayed image selected at a reboot.
    assert!(fallbacks > 20, "{fallbacks}");
}
