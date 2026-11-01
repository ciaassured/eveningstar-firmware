#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets, lengths and counts are small"
)]

use sha2::{Digest as _, Sha256};

use super::*;
use crate::Slot::{Ota0, Ota1};
use crate::layout::{OTA_0, OTA_1, OTADATA, TABLE_OFFSET};
use crate::otadata::{Otadata, Sector, State};
use crate::test_support::{
    APP, BOOTLOADER, FLASH_LEN, ImageBuilder, OTADATA_AT, OTADATA_FIRST_BOOT, OTADATA_LEN, Op,
    RamFlash, Rng, TABLE, app_descriptor_bytes, block_on, boot_flash, data, idf_loads, lend, run,
    segment_at,
};
use crate::{DeviceError, Entry, Flash, ReadError, Tier, split};

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
fn flashed(image: &[u8]) -> RamFlash {
    let flash = RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (TABLE_OFFSET, TABLE),
            (OTA_0.offset, image),
        ],
    );
    assert_eq!(boot_flash(&flash), Some(Ota0));
    assert_eq!(flash.at(OTADATA_AT, OTADATA_LEN), OTADATA_FIRST_BOOT);
    flash
}

/// The OTA mechanism of `flash` booted from `running`, which reaches the
/// full tier.
fn ota_on(flash: &RamFlash, running: Slot) -> Ota<'_> {
    let storage = run(split(lend(flash), running.partition().offset));
    assert_eq!(storage.report.tier(), Tier::Full, "{:?}", storage.report);
    storage.data.unwrap().ota.unwrap()
}

/// Boots `flash`, which has to start a slot.
fn reboot(flash: &RamFlash) -> Slot {
    boot_flash(flash).expect("no slot boots")
}

/// The otadata of `flash`.
fn otadata_of(flash: &RamFlash) -> Otadata {
    otadata::read(&flash.at(OTADATA_AT, OTADATA_LEN)[..])
}

/// The boot state `flash`'s otadata gives the image running from `running`.
fn state_of(flash: &RamFlash, running: Slot) -> BootState {
    otadata_of(flash).boot_state(running)
}

/// Whether `slot` of `flash` starts with `image`.
fn holds(flash: &RamFlash, slot: Slot, image: &[u8]) -> bool {
    let at = slot.partition().offset as usize;
    flash.with_bytes(|bytes| bytes[at..at + image.len()] == *image)
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
fn finish_with<'f>(ota: &mut Ota<'f>, bytes: &[u8]) -> Result<VerifiedImage<'f>, OtaError> {
    run(async {
        let mut update = ota.begin_update().await?;
        update.write(bytes).await?;
        update.finish().await
    })
}

/// Updates the board running from `running` to `image`, activates it and
/// boots it.
fn update_and_boot(flash: &RamFlash, running: Slot, image: &[u8], rng: &mut Rng) {
    let mut ota = ota_on(flash, running);
    let verified = run(update(&mut ota, image, rng, 0x8000)).unwrap();
    run(ota.activate(verified)).unwrap();
    assert_eq!(reboot(flash), running.other());
}

/// A board that ran `old` from `ota_0`, then updated to `new` in `ota_1`,
/// confirmed it and booted it again: `old` is the rollback target.
fn updated(old: &[u8], new: &[u8]) -> RamFlash {
    let flash = flashed(old);
    update_and_boot(&flash, Ota0, new, &mut Rng::new(0x0DD));
    run(ota_on(&flash, Ota1).mark_valid()).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    let state = state_of(&flash, Ota1);
    assert_eq!(state.state, Some(State::Valid));
    assert!(state.rollback_target);
    flash
}

/// The span of an erase or program.
fn op_span(op: Op) -> (u32, u32) {
    match op {
        Op::Erase(address) => (address, address + SECTOR_SIZE),
        Op::Program(address, len) => (address, address + len),
    }
}

/// Every erase and program `flash` logged from the `from`th on lies in the
/// otadata or in the slot that isn't `running`.
fn confined(flash: &RamFlash, from: usize, running: Slot) {
    for op in &flash.ops()[from..] {
        let (start, end) = op_span(*op);
        let inside = |entry: Entry| entry.offset <= start && end <= entry.end();
        assert!(
            inside(OTADATA) || inside(running.other().partition()),
            "{op:?} while running from {running:?}"
        );
    }
}

// A whole update.

#[test]
fn an_update_is_written_activated_booted_and_confirmed_and_then_another() {
    let old = APP;
    let new = app_version("0.2.0");
    let newer = app_version("0.3.0");
    let mut rng = Rng::new(0x07A);
    let flash = flashed(old);

    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state(),
        BootState {
            running: Ota0,
            selected: Some(Ota0),
            state: Some(State::Valid),
            rollback_target: false,
        }
    );
    let image = run(update(&mut ota, &new, &mut rng, 20_000)).unwrap();
    assert_eq!(image.slot(), Ota1);
    assert_eq!(image.size() as usize, new.len());
    assert_eq!(image.digest()[..], new[new.len() - 32..]);
    assert_eq!(image.version().to_str(), Some("0.2.0"));
    assert!(holds(&flash, Ota1, &new));
    run(ota.activate(image)).unwrap();
    // Selected, while the running image keeps its own state.
    let state = ota.boot_state();
    assert_eq!(state.selected, Some(Ota1));
    assert_eq!(state.state, Some(State::Valid));
    confined(&flash, 0, Ota0);
    assert!(holds(&flash, Ota0, old));

    // Started once, with its rollback armed, and confirmed.
    assert_eq!(reboot(&flash), Ota1);
    let ops = flash.ops().len();
    let mut ota = ota_on(&flash, Ota1);
    assert_eq!(
        ota.boot_state(),
        BootState {
            running: Ota1,
            selected: Some(Ota1),
            state: Some(State::PendingVerify),
            rollback_target: true,
        }
    );
    run(ota.mark_valid()).unwrap();
    assert_eq!(ota.boot_state().state, Some(State::Valid));
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(reboot(&flash), Ota1);
    let mut ota = ota_on(&flash, Ota1);
    assert_eq!(
        ota.boot_state(),
        BootState {
            running: Ota1,
            selected: Some(Ota1),
            state: Some(State::Valid),
            rollback_target: true,
        }
    );

    // And back the other way, over the old image.
    let image = run(update(&mut ota, &newer, &mut rng, 50_000)).unwrap();
    assert_eq!(image.slot(), Ota0);
    assert_eq!(image.version().to_str(), Some("0.3.0"));
    run(ota.activate(image)).unwrap();
    confined(&flash, ops, Ota1);
    assert!(holds(&flash, Ota1, &new));
    assert_eq!(reboot(&flash), Ota0);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(ota.boot_state().state, Some(State::PendingVerify));
    run(ota.mark_valid()).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::Valid));
    assert!(holds(&flash, Ota0, &newer));
    assert!(holds(&flash, Ota1, &new));
}

#[test]
fn the_parts_an_image_arrives_in_dont_change_what_is_written() {
    let new = small_app("2.0.0", 1000);
    let flash = flashed(&small_app("1.0.0", 0));
    let mut ota = ota_on(&flash, Ota0);
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
        let image = run(async {
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
    let image = run(async {
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
    let mut ota = ota_on(&flash, Ota0);
    let mut rng = Rng::new(0xE7A5);
    let from = flash.ops().len();
    let image = run(async {
        let mut update = ota.begin_update().await?;
        let begun = flash.ops().len();
        assert_eq!(flash.ops()[begun - 1], Op::Erase(OTA_1.offset));
        for part in parts(&new, &mut rng, 10_000) {
            update.write(part).await?;
            // Up to the sector the last whole word programmed lies in.
            let programmed = update.written() - update.written() % WORD_SIZE;
            let sectors = programmed.div_ceil(SECTOR_SIZE).max(1);
            let erased: Vec<_> = flash.ops()[begun - 1..]
                .iter()
                .filter_map(|op| match op {
                    Op::Erase(address) => Some(*address),
                    Op::Program(..) => None,
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
    for op in &flash.ops()[from..] {
        match *op {
            Op::Erase(address) => erased.push(address),
            Op::Program(address, len) if OTA_1.offset <= address => {
                let sector = address - address % SECTOR_SIZE;
                assert!(erased.contains(&sector), "{op:?}");
                assert_eq!(sector, (address + len - 1) / SECTOR_SIZE * SECTOR_SIZE);
            }
            Op::Program(..) => {}
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
    let mut ota = ota_on(&flash, Ota0);
    let at = OTA_1.offset;
    run(async {
        let mut update = ota.begin_update().await.unwrap();
        let ops = flash.ops().len();
        update.write(&[1, 2, 3]).await.unwrap();
        assert_eq!(flash.ops().len(), ops, "nothing programmed");
        assert_eq!(update.written(), 3);
        update.write(&[4]).await.unwrap();
        assert_eq!(flash.ops()[ops..], [Op::Program(at, 4)]);
        update.write(&[5, 6]).await.unwrap();
        update.write(&[7, 8, 9, 10, 11]).await.unwrap();
        assert_eq!(
            flash.ops()[ops..],
            [Op::Program(at, 4), Op::Program(at + 4, 4)]
        );
        assert_eq!(update.written(), 11);
        // The partial word is completed with 0xFF and programmed, with a
        // yield; the verification stops at the magic.
        let finish = block_on(update.finish());
        assert_eq!(finish.output, Err(OtaError::Image(ImageError::Magic(1))));
        assert_eq!(finish.wakes, 1);
        assert_eq!(flash.ops()[ops + 2..], [Op::Program(at + 8, 4)]);
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
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        finish_with(&mut ota, &[]),
        Err(OtaError::Image(ImageError::Magic(0xFF)))
    );
    // The header, then erased bytes where the first segment's header goes.
    assert_eq!(
        finish_with(&mut ota, &APP[..24]),
        Err(OtaError::Image(ImageError::SegmentLength {
            index: 0,
            len: u32::MAX
        }))
    );
    assert!(matches!(
        finish_with(&mut ota, &APP[..1000]),
        Err(OtaError::Image(_))
    ));
}

#[test]
fn a_truncated_image_is_refused() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
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
        let result = finish_with(&mut ota, &new[..new.len() - cut]);
        assert!(
            matches!(result, Err(OtaError::Image(_) | OtaError::Length { .. })),
            "{cut}: {result:?}"
        );
    }
    // With the same image left in the slot by an earlier update, an update
    // cut at a sector boundary leaves the image's tail in the sectors it
    // never reached. The image verifies, and only the length is wrong.
    assert!(finish_with(&mut ota, &new).is_ok());
    let last = (new.len() - 1) / 4096 * 4096;
    for end in [last, last - 4096, 0x1_0000] {
        assert_eq!(
            finish_with(&mut ota, &new[..end]),
            Err(OtaError::Length {
                written: end as u32,
                image: new.len() as u32,
            }),
            "{end:#x}"
        );
        assert!(finish_with(&mut ota, &new).is_ok());
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
    let mut ota = ota_on(&flash, Ota0);
    let len = image.len() as u32;
    assert_eq!(
        finish_with(&mut ota, &image[..image.len() - 1]),
        Err(OtaError::Length {
            written: len - 1,
            image: len,
        })
    );
    assert!(holds(&flash, Ota1, &image));
    assert_eq!(
        finish_with(&mut ota, &image).map(|image| image.size()),
        Ok(len)
    );
}

#[test]
fn bytes_after_the_image_are_refused() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    for extra in [&[0xFF][..], &[0; 3], &[0xFF; 4], &[1; 100], &[0xFF; 4096]] {
        let mut bytes = new.clone();
        bytes.extend_from_slice(extra);
        assert_eq!(
            finish_with(&mut ota, &bytes),
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
    let mut ota = ota_on(&flash, Ota0);
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
        let result = finish_with(&mut ota, &bytes);
        assert!(
            matches!(result, Err(OtaError::Image(_))),
            "{at:#x}: {result:?}"
        );
    }
    assert!(finish_with(&mut ota, &new).is_ok());
}

#[test]
fn an_image_for_another_chip_is_refused() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let mut other = APP.to_vec();
    // The ESP32-C3's, with the digest made again.
    other[12..14].copy_from_slice(&0x0005_u16.to_le_bytes());
    let other = restamp(&other, |_| {});
    assert_eq!(
        finish_with(&mut ota, &other),
        Err(OtaError::Image(ImageError::Chip(5)))
    );
    assert_eq!(
        run(ota.verify_other()),
        Err(OtaError::Image(ImageError::Chip(5)))
    );
}

#[test]
fn an_image_of_another_project_is_refused() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let images = [
        app_project("another"),
        app_project("eveningstar2"),
        app_project("evening"),
        app_project(""),
        small_image("another", "1.0.0", 0),
    ];
    for image in images {
        assert_eq!(finish_with(&mut ota, &image), Err(OtaError::Project));
        assert_eq!(run(ota.verify_other()), Err(OtaError::Project));
    }
    // Bytes after the name's NUL don't count, as they aren't part of it.
    let padded = restamp(APP, |image| {
        image[DESCRIPTOR + 48 + 30] = b'x';
    });
    assert_eq!(
        finish_with(&mut ota, &padded).map(|image| image.size()),
        Ok(APP.len() as u32)
    );
}

#[test]
fn the_project_is_the_running_app_s() {
    // A board running a small image of another project updates only to that
    // project.
    let theirs = small_image("another", "1.0.0", 0);
    let flash = flashed(&theirs);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(finish_with(&mut ota, APP), Err(OtaError::Project));
    let next = small_image("another", "2.0.0", 5);
    assert_eq!(
        finish_with(&mut ota, &next).map(|image| image.size()),
        Ok(next.len() as u32)
    );
}

#[test]
fn writing_past_the_end_of_the_slot_is_refused_before_anything_is_written() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let slot = OTA_1.size as usize;
    run(async {
        let mut update = ota.begin_update().await.unwrap();
        let ops = flash.ops().len();
        assert_eq!(
            update.write(&vec![0; slot + 1]).await,
            Err(OtaError::TooLarge)
        );
        assert_eq!(flash.ops().len(), ops);
        assert_eq!(update.written(), 0);

        let zeros = vec![0; 0x1_0000];
        for _ in 0..slot / zeros.len() - 1 {
            update.write(&zeros).await.unwrap();
        }
        update.write(&zeros[..zeros.len() - 3]).await.unwrap();
        let ops = flash.ops().len();
        assert_eq!(update.write(&[0; 4]).await, Err(OtaError::TooLarge));
        assert_eq!(flash.ops().len(), ops, "nothing of it written");
        // The update goes on.
        update.write(&[0; 3]).await.unwrap();
        assert_eq!(update.written() as usize, slot);
        assert_eq!(update.write(&[0]).await, Err(OtaError::TooLarge));
        update.write(&[]).await.unwrap();
        assert_eq!(
            update.finish().await,
            Err(OtaError::Image(ImageError::Magic(0)))
        );
    });
    confined(&flash, 0, Ota0);
    let last = flash.ops().last().copied().unwrap();
    assert_eq!(op_span(last).1, OTA_1.end());
}

#[test]
fn a_failed_write_ends_the_update() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    run(async {
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
    let image = finish_with(&mut ota, &new).unwrap();
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert!(holds(&flash, Ota1, &new));
}

// Stale and changed images.

#[test]
fn an_update_begun_since_makes_a_verified_image_stale() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let finished = finish_with(&mut ota, &new).unwrap();
    let verified = run(ota.verify_other()).unwrap();
    assert_eq!(finished, verified);
    // An update begun and abandoned.
    assert!(run(ota.begin_update()).is_ok());
    let ops = flash.ops().len();
    assert_eq!(run(ota.activate(finished)), Err(OtaError::Stale));
    assert_eq!(run(ota.activate(verified)), Err(OtaError::Stale));
    assert_eq!(flash.ops().len(), ops, "nothing written");
    assert_eq!(ota.boot_state().selected, Some(Ota0));
    // The abandoned update erased the slot's first sector.
    assert_eq!(
        run(ota.verify_other()),
        Err(OtaError::Image(ImageError::Magic(0xFF)))
    );
    assert_eq!(reboot(&flash), Ota0);
}

#[test]
fn a_refused_update_still_makes_a_verified_image_stale() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &new, &mut Rng::new(1));
    let mut ota = ota_on(&flash, Ota1);
    // The old image, which verifies, while the new one isn't confirmed.
    let old = run(ota.verify_other()).unwrap();
    let ops = flash.ops().len();
    assert_eq!(
        run(ota.begin_update()).err(),
        Some(OtaError::Plan(PlanError::PendingVerify))
    );
    assert_eq!(run(ota.activate(old)), Err(OtaError::Stale));
    assert_eq!(flash.ops().len(), ops, "nothing written");
}

#[test]
fn an_image_changed_since_it_was_verified_isnt_activated() {
    let new = app_version("0.2.0");
    let newer = app_version("0.3.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &new).unwrap();
    let ops = flash.ops().len();

    let poke = |offset: usize, bytes: &[u8]| flash.poke(OTA_1.offset + offset as u32, bytes);
    let flip = |offset: usize| poke(offset, &[new[offset] ^ 0x10]);
    // A byte of the data, of the digest, of the header.
    for offset in [0x5000, new.len() - 1, 2] {
        let image = run(ota.verify_other()).unwrap();
        flip(offset);
        assert_eq!(
            run(ota.activate(image)),
            Err(OtaError::Changed),
            "{offset:#x}"
        );
        poke(offset, &new[offset..=offset]);
    }
    // Another image that verifies.
    let verified = run(ota.verify_other()).unwrap();
    poke(0, &newer);
    assert_eq!(run(ota.activate(verified)), Err(OtaError::Changed));
    // A slot that can't be read.
    poke(0, &new);
    let verified = run(ota.verify_other()).unwrap();
    flash.fail_reads(OTA_1.offset + 0x8000..OTA_1.offset + 0x8001);
    assert_eq!(
        run(ota.activate(verified)),
        Err(OtaError::Image(ImageError::Read(ReadError::Device)))
    );
    flash.heal();
    assert_eq!(flash.ops().len(), ops, "nothing written");
    assert_eq!(ota.boot_state().selected, Some(Ota0));

    // Back as it was verified, the first image still activates.
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
}

#[test]
fn verify_other_reads_what_the_other_slot_holds() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        run(ota.verify_other()),
        Err(OtaError::Image(ImageError::Magic(0xFF)))
    );
    let mut rng = Rng::new(0x6A7);
    flash.poke(OTA_1.offset, &data(&mut rng, 0x4000));
    assert!(matches!(run(ota.verify_other()), Err(OtaError::Image(_))));
    flash.poke(OTA_1.offset, &[0xE9]);
    assert!(matches!(run(ota.verify_other()), Err(OtaError::Image(_))));
    let new = app_version("0.2.0");
    flash.poke(OTA_1.offset, &new);
    let image = run(ota.verify_other()).unwrap();
    assert_eq!(image.slot(), Ota1);
    assert_eq!(image.size() as usize, new.len());
    assert_eq!(image.version().to_str(), Some("0.2.0"));
    assert_eq!(flash.ops(), [], "verifying writes nothing");
}

#[test]
fn the_debug_output_names_the_slot_and_the_progress() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &app_version("0.2.0")).unwrap();
    let debug = format!("{image:?}");
    assert!(
        debug.contains("Ota1") && debug.contains("\"0.2.0\""),
        "{debug}"
    );
    let ota_debug = format!("{ota:?}");
    assert!(ota_debug.contains("generation: 1"), "{ota_debug}");
    run(async {
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
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &app_version("0.2.0")).unwrap();
    let again = run(ota.verify_other()).unwrap();
    let ops = flash.ops().len();
    let inactive = otadata_of(&flash).active().unwrap().other();
    run(ota.activate(image)).unwrap();
    // The entry selecting the slot, written to the sector that wasn't active.
    let otadata = otadata_of(&flash);
    assert_eq!(otadata.active(), Some(inactive));
    let entry = otadata.active_entry().unwrap();
    assert_eq!((entry.slot(), entry.state), (Ota1, State::New));
    let at = OTADATA_AT + inactive.offset();
    assert_eq!(flash.ops()[ops..], [Op::Erase(at), Op::Program(at, 32)]);
    // Selecting it again isn't selecting the running slot's image away.
    assert_eq!(
        run(ota.activate(again)),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert_eq!(flash.ops().len(), ops + 2);
}

#[test]
fn an_image_isnt_activated_while_the_running_one_is_unconfirmed() {
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(2));
    let mut ota = ota_on(&flash, Ota1);
    let old = run(ota.verify_other()).unwrap();
    assert_eq!(
        run(ota.activate(old)),
        Err(OtaError::Plan(PlanError::PendingVerify))
    );
    // Rolling back is the way back.
    run(ota.roll_back()).unwrap();
    assert_eq!(reboot(&flash), Ota0);
}

#[test]
fn the_image_in_the_other_slot_is_activated_again() {
    let new = app_version("0.2.0");
    let flash = updated(APP, &new);
    let mut ota = ota_on(&flash, Ota1);
    let old = run(ota.verify_other()).unwrap();
    assert_eq!(old.version().to_str(), Some("0.1.0"));
    run(ota.activate(old)).unwrap();
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
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &new).unwrap();
    run(ota.activate(image)).unwrap();
    assert_eq!(ota.boot_state().selected, Some(Ota1));
    let ops = flash.ops().len();
    let activation = otadata_of(&flash).active().unwrap();
    let at = OTADATA_AT + activation.offset();
    assert!(run(ota.begin_update()).is_ok());
    // The activation rewritten in place as Invalid, which leaves the other
    // entry, for the running slot, active; then a second entry for the
    // running slot in the sector that isn't active, the same one; then the
    // slot's first sector.
    assert_eq!(
        flash.ops()[ops..],
        [
            Op::Erase(at),
            Op::Program(at, 32),
            Op::Erase(at),
            Op::Program(at, 32),
            Op::Erase(OTA_1.offset),
        ]
    );
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota0));
    }
    assert_eq!(ota.boot_state().selected, Some(Ota0));
    // The slot is rewritten and activated.
    let newer = app_version("0.3.0");
    let image = finish_with(&mut ota, &newer).unwrap();
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert!(holds(&flash, Ota1, &newer));
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn beginning_an_update_after_a_fallback_takes_the_broken_slot_out_of_the_selection() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &new).unwrap();
    run(ota.activate(image)).unwrap();
    // The image breaks before the boot, so the bootloader falls back.
    flash.poke(OTA_1.offset + 0x100, &[!new[0x100]]);
    assert_eq!(reboot(&flash), Ota0);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state(),
        BootState {
            running: Ota0,
            selected: Some(Ota1),
            state: Some(State::Valid),
            rollback_target: false,
        }
    );
    assert_eq!(
        run(ota.mark_valid()),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert_eq!(
        run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert!(matches!(run(ota.verify_other()), Err(OtaError::Image(_))));

    assert!(run(ota.begin_update()).is_ok());
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota0));
    }
    let image = finish_with(&mut ota, &new).unwrap();
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn beginning_an_update_is_refused_while_the_running_image_is_unconfirmed() {
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(3));
    let mut ota = ota_on(&flash, Ota1);
    let ops = flash.ops().len();
    assert_eq!(
        run(ota.begin_update()).err(),
        Some(OtaError::Plan(PlanError::PendingVerify))
    );
    assert_eq!(flash.ops().len(), ops, "nothing written");
    assert!(holds(&flash, Ota0, APP));
    run(ota.mark_valid()).unwrap();
    assert!(run(ota.begin_update()).is_ok());
}

#[test]
fn beginning_an_update_without_a_valid_entry_selects_the_running_slot_first() {
    // An aborted entry and a corrupt one; ota_0 doesn't load, so the
    // bootloader started ota_1.
    let mut otadata = vec![0xFF; OTADATA_LEN];
    otadata[..32].copy_from_slice(&otadata::SelectEntry::new(3, State::Aborted).to_bytes());
    let mut corrupt = otadata::SelectEntry::new(4, State::Valid);
    corrupt.crc ^= 1;
    otadata[4096..4096 + 32].copy_from_slice(&corrupt.to_bytes());
    let flash = RamFlash::with(
        FLASH_LEN,
        &[
            (0, BOOTLOADER),
            (TABLE_OFFSET, TABLE),
            (OTADATA_AT, &otadata),
            (OTA_1.offset, APP),
        ],
    );
    assert_eq!(reboot(&flash), Ota1);
    let mut ota = ota_on(&flash, Ota1);
    assert_eq!(ota.boot_state().selected, None);
    let new = app_version("0.2.0");
    let image = finish_with(&mut ota, &new).unwrap();
    let otadata = otadata_of(&flash);
    for sector in Sector::ALL {
        assert_eq!(otadata.valid(sector).map(|entry| entry.slot()), Some(Ota1));
    }
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota0);
    assert_eq!(state_of(&flash, Ota0).state, Some(State::PendingVerify));
    assert_eq!(reboot(&flash), Ota1);
}

/// A flash whose programs of the otadata report success and change nothing.
struct Forgetful<'a>(&'a RamFlash);

impl Flash for Forgetful<'_> {
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
    let storage = run(split(&mut forgetful, OTA_0.offset));
    let mut ota = storage.data.unwrap().ota.unwrap();
    assert_eq!(run(ota.begin_update()).err(), Some(OtaError::Unsettled));
    // Two writes of the second sector, and nothing of the slot.
    let second = OTADATA_AT + SECTOR_SIZE;
    assert_eq!(flash.ops(), [Op::Erase(second), Op::Erase(second)]);
}

// Confirming.

#[test]
fn marking_valid_confirms_the_running_image_once() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    run(ota.mark_valid()).unwrap();
    assert_eq!(flash.ops(), [], "already valid");

    update_and_boot(&flash, Ota0, &app_version("0.2.0"), &mut Rng::new(4));
    let mut ota = ota_on(&flash, Ota1);
    let ops = flash.ops().len();
    let active = otadata_of(&flash).active().unwrap();
    run(ota.mark_valid()).unwrap();
    // The active entry, rewritten in place.
    let at = OTADATA_AT + active.offset();
    assert_eq!(flash.ops()[ops..], [Op::Erase(at), Op::Program(at, 32)]);
    assert_eq!(otadata_of(&flash).active(), Some(active));
    run(ota.mark_valid()).unwrap();
    assert_eq!(flash.ops().len(), ops + 2);
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::Valid));
}

#[test]
fn marking_valid_is_refused_while_another_slot_is_selected() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    let image = finish_with(&mut ota, &app_version("0.2.0")).unwrap();
    run(ota.activate(image)).unwrap();
    let ops = flash.ops().len();
    assert_eq!(
        run(ota.mark_valid()),
        Err(OtaError::Plan(PlanError::NotSelected))
    );
    assert_eq!(flash.ops().len(), ops);
}

// Rolling back.

#[test]
fn an_update_that_isnt_confirmed_rolls_back_at_the_next_boot() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    update_and_boot(&flash, Ota0, &new, &mut Rng::new(5));
    assert_eq!(reboot(&flash), Ota0);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        ota.boot_state(),
        BootState {
            running: Ota0,
            selected: Some(Ota0),
            state: Some(State::Valid),
            rollback_target: false,
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
        run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    // The image is intact and can be activated again.
    let image = run(ota.verify_other()).unwrap();
    run(ota.activate(image)).unwrap();
    assert_eq!(reboot(&flash), Ota1);
    assert_eq!(state_of(&flash, Ota1).state, Some(State::PendingVerify));
}

#[test]
fn rolling_back_starts_the_other_slot_at_the_next_boot() {
    let new = app_version("0.2.0");
    for confirmed in [false, true] {
        let flash = flashed(APP);
        update_and_boot(&flash, Ota0, &new, &mut Rng::new(6));
        let mut ota = ota_on(&flash, Ota1);
        if confirmed {
            run(ota.mark_valid()).unwrap();
        }
        let ops = flash.ops().len();
        let active = otadata_of(&flash).active().unwrap();
        run(ota.roll_back()).unwrap();
        let at = OTADATA_AT + active.offset();
        assert_eq!(flash.ops()[ops..], [Op::Erase(at), Op::Program(at, 32)]);
        assert_eq!(
            otadata_of(&flash).entry(active).unwrap().state,
            State::Invalid
        );
        assert_eq!(reboot(&flash), Ota0, "confirmed: {confirmed}");
        let mut ota = ota_on(&flash, Ota0);
        assert_eq!(
            ota.boot_state(),
            BootState {
                running: Ota0,
                selected: Some(Ota0),
                state: Some(State::Valid),
                rollback_target: false,
            }
        );
        assert_eq!(
            run(ota.roll_back()),
            Err(OtaError::Plan(PlanError::NoRollbackTarget))
        );
        assert_eq!(reboot(&flash), Ota0);
    }
}

#[test]
fn rolling_back_needs_a_target() {
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    assert_eq!(
        run(ota.roll_back()),
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    // An image in the other slot that no entry selects, which isn't hashed.
    flash.poke(OTA_1.offset, &app_version("0.2.0"));
    let done = block_on(ota.roll_back());
    assert_eq!(
        done.output,
        Err(OtaError::Plan(PlanError::NoRollbackTarget))
    );
    assert_eq!(done.wakes, 0);
    assert_eq!(flash.ops(), []);
}

#[test]
fn rolling_back_needs_an_image_that_verifies() {
    let flash = updated(APP, &app_version("0.2.0"));
    let mut ota = ota_on(&flash, Ota1);
    let ops = flash.ops().len();
    // A byte of the old image decays.
    flash.poke(OTA_0.offset + 0x9000, &[!APP[0x9000]]);
    assert!(matches!(run(ota.roll_back()), Err(OtaError::Image(_))));
    // Another project's image.
    flash.poke(OTA_0.offset, &app_project("another"));
    assert_eq!(run(ota.roll_back()), Err(OtaError::Project));
    assert_eq!(flash.ops().len(), ops, "nothing written");
    assert_eq!(reboot(&flash), Ota1);
    // Restored, it rolls back.
    flash.poke(OTA_0.offset, APP);
    run(ota.roll_back()).unwrap();
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
        let from = flash.ops().len();
        let mut rng = Rng::new(7);
        let mut ota = ota_on(flash, running);
        // Refusals.
        assert!(finish_with(&mut ota, &app_project("another")).is_err());
        assert!(finish_with(&mut ota, &newer[..1000]).is_err());
        run(async {
            let mut update = ota.begin_update().await.unwrap();
            let slot = OTA_0.size as usize;
            assert!(update.write(&vec![0; slot + 1]).await.is_err());
        });
        let _ = run(ota.roll_back());
        // An update, cancelled, and another, activated.
        let image = run(update(&mut ota, &newer, &mut rng, 30_000)).unwrap();
        run(ota.activate(image)).unwrap();
        let image = run(update(&mut ota, &newer, &mut rng, 30_000)).unwrap();
        run(ota.activate(image)).unwrap();
        let _ = run(ota.mark_valid());
        confined(flash, from, running);
        assert!(holds(flash, running, running_image));
        // Booted, confirmed and rolled back from the new slot.
        assert_eq!(reboot(flash), running.other());
        let from = flash.ops().len();
        let mut ota = ota_on(flash, running.other());
        run(ota.mark_valid()).unwrap();
        run(ota.roll_back()).unwrap();
        confined(flash, from, running.other());
        assert!(holds(flash, running.other(), &newer));
        assert_eq!(reboot(flash), running);
    }
}

#[test]
fn the_guard_refuses_what_a_wrong_span_would_write() {
    let flash = flashed(APP);
    let region = |entry: Entry| Region::new(&flash, entry.offset, entry.size).unwrap();
    let project = run(split(lend(&flash), OTA_0.offset))
        .report
        .app
        .unwrap()
        .project_name;
    // The running slot in place of the other one.
    let mut ota = Ota::new(region(OTA_0), region(OTADATA), Ota0, project);
    assert_eq!(
        run(ota.begin_update()).err(),
        Some(OtaError::Access(AccessError::Protected))
    );
    // The boot chain in place of the otadata, holding no valid entry.
    let boot = Region::new(&flash, 0, OTADATA.size).unwrap();
    let mut ota = Ota::new(region(OTA_1), boot, Ota0, project);
    assert_eq!(
        run(ota.begin_update()).err(),
        Some(OtaError::Access(AccessError::Protected))
    );
    assert!(holds(&flash, Ota0, APP));
    assert_eq!(flash.at(0, BOOTLOADER.len()), BOOTLOADER);
    // Only the real otadata was written.
    confined(&flash, 0, Ota0);
    assert!(
        flash
            .ops()
            .iter()
            .all(|op| op_span(*op).0 >= OTADATA.offset && op_span(*op).1 <= OTADATA.end())
    );
}

// Yields.

#[test]
fn long_operations_yield_to_the_executor() {
    let new = app_version("0.2.0");
    let flash = flashed(APP);
    let mut ota = ota_on(&flash, Ota0);
    // What verifying the image yields, on its own.
    let slot = Region::new(&flash, OTA_1.offset, OTA_1.size).unwrap();
    flash.poke(OTA_1.offset, &new);
    let verifying = block_on(image::verify_app(&slot)).wakes;
    assert_eq!(verifying, new.len() / 4096);

    let image = run(async {
        // One otadata write, an erase and a program, and the slot's first
        // sector.
        let begin = block_on(ota.begin_update());
        assert_eq!(begin.wakes, 3);
        let mut update = begin.output.unwrap();
        // 16 sectors: 15 erased, as the first is, and 16 programmed.
        assert_eq!(block_on(update.write(&new[..0x1_0000])).wakes, 31);
        // Two words into the next sector, which is erased first; two bytes
        // held.
        assert_eq!(block_on(update.write(&new[0x1_0000..0x1_000A])).wakes, 2);
        // Only held.
        assert_eq!(block_on(update.write(&new[0x1_000A..0x1_000B])).wakes, 0);
        update.write(&new[0x1_000B..]).await.unwrap();
        // The image hashed.
        let finish = block_on(update.finish());
        assert_eq!(finish.wakes, verifying);
        finish.output
    })
    .unwrap();
    // Hashed again, then one otadata write.
    let activate = block_on(ota.activate(image));
    assert_eq!(activate.output, Ok(()));
    assert_eq!(activate.wakes, verifying + 2);
    assert_eq!(block_on(ota.verify_other()).wakes, verifying);

    assert_eq!(reboot(&flash), Ota1);
    let mut ota = ota_on(&flash, Ota1);
    assert_eq!(block_on(ota.mark_valid()).wakes, 2);
    let verifying = block_on(image::verify_app(APP)).wakes;
    let roll_back = block_on(ota.roll_back());
    assert_eq!(roll_back.output, Ok(()));
    assert_eq!(roll_back.wakes, verifying + 2);
}
