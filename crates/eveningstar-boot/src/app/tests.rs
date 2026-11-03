#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use core::pin::pin;

use eveningstar_esp_formats::pace::Yield;
use eveningstar_esp_formats::test_support::{APP, Rng, app_image, data};
use eveningstar_storage::chain::Handles;
use eveningstar_storage::nor::Writer;
use eveningstar_storage::test_support::{Call, RamNor, TestFlash, run};
use eveningstar_storage::{PublicRead, SECTOR_SIZE, Slot, ViewError, layout};

use super::*;
use crate::digest;
use crate::test_support::{Counting, drive, nix_board, poll_once, sha256, split_board};

fn sha() -> sha2::Sha256 {
    sha2::Sha256::default()
}

/// Runs `slot`'s check to completion.
fn check<R: AppRole>(slot: &AppSlot<'_, R>) {
    run(slot.check(&sha(), &mut Yield, &mut || {}));
}

/// A small app image of the firmware's project.
fn image() -> Vec<u8> {
    app_image().build()
}

/// A different one: the same with other data in its last segment.
fn other_image() -> Vec<u8> {
    let mut builder = app_image();
    builder.segments[2].1 = vec![0x5A; 0x40];
    builder.build()
}

/// Length of the "secret" bytes a test places after an image.
const TAIL: usize = 0x1_0000;

/// A flash holding `bytes` at the start of `ota_1`, followed by [`TAIL`]
/// bytes that aren't `0xFF`, the alternate slot's module over it, and a
/// writer of the slot while the app runs from `ota_0`.
fn alternate_over(
    bytes: &[u8],
) -> (
    AppSlot<'static, AlternateApp>,
    Writer<'static, AlternateApp>,
    TestFlash,
) {
    let offset = layout::OTA_1.offset;
    let tail = vec![0xA5; TAIL];
    let device = RamNor::with(
        layout::FLASH_SIZE,
        &[(offset, bytes), (offset + bytes.len() as u32, &tail)],
    );
    let flash = TestFlash::new(device);
    let span = Span::from(layout::OTA_1);
    (
        AppSlot::new(flash.reader::<AlternateApp>(span)),
        flash.writer::<AlternateApp>(span, Slot::Ota0),
        flash,
    )
}

/// Every byte of `view`.
fn served(view: &impl PublicRead) -> Vec<u8> {
    let mut bytes = vec![0; view.len() as usize];
    run(view.read(0, &mut bytes)).unwrap();
    bytes
}

/// Replaces the first bytes of the slot `writer` writes with `bytes`.
fn rewrite(writer: &mut Writer<'_, AlternateApp>, bytes: &[u8]) {
    let mut session = writer.begin();
    let erased = (bytes.len() as u32).next_multiple_of(SECTOR_SIZE);
    run(session.erase(0, erased)).unwrap();
    run(session.program(0, bytes)).unwrap();
}

#[test]
fn the_nix_boards_slots_are_checked() {
    let (storage, _) = split_board(nix_board(), layout::OTA_0.offset);
    let Handles::Boot(handles, _) = storage.handles else {
        panic!("{:?}", storage.report.tier);
    };
    let current = AppSlot::new(handles.current_app);
    let alternate = AppSlot::new(handles.alternate_app);
    let app = App::layout(&current, &alternate);
    let current = app.current().unwrap();
    let alternate = app.alternate().unwrap();
    assert_eq!(current.span(), Span::from(layout::OTA_0));
    assert_eq!(alternate.span(), Span::from(layout::OTA_1));
    assert!(app.found_app().is_none());
    assert_eq!(current.model(), None);

    check(current);
    check(alternate);
    let model = current.model().unwrap();
    let verified = run(image::verify_app(APP)).unwrap();
    let image = model.image().unwrap();
    assert_eq!(image.size(), verified.size());
    assert_eq!(image.digest(), verified.digest());
    assert_eq!(
        image.file_sha256(),
        &sha256(&APP[..verified.size() as usize])
    );
    assert_eq!(*image.header(), *verified.header());
    let descriptor = current.descriptor(&model).unwrap().unwrap();
    assert_eq!(descriptor.project_name().to_str(), Some("eveningstar"));
    assert_eq!(
        served(&current.published(&model).unwrap()),
        APP[..verified.size() as usize]
    );

    // The alternate slot is erased: nothing verifies, nothing is published.
    let empty = alternate.model().unwrap();
    assert_eq!(empty.image(), Err(ImageFailure::Magic));
    assert_eq!(
        alternate.published(&empty).unwrap_err(),
        Unpublished::Unverified
    );
    assert_eq!(alternate.descriptor(&empty), Ok(Err(ImageFailure::Magic)));
}

#[test]
fn a_slot_publishes_its_image_and_never_the_bytes_after_it() {
    let image = image();
    let len = image.len() as u32;
    let (slot, _writer, flash) = alternate_over(&image);
    check(&slot);
    let model = slot.model().unwrap();
    let view = slot.published(&model).unwrap();
    assert_eq!(view.len(), len);
    assert_eq!(view.sanitized(), []);
    assert_eq!(served(&view), image);

    let mut past = [0; 1];
    assert_eq!(run(view.read(len, &mut past)), Err(ViewError::OutOfRange));
    assert_eq!(
        run(view.read(len - 1, &mut [0; 2])),
        Err(ViewError::OutOfRange)
    );
    let whole = run(digest(
        &view,
        Span::new(0, len),
        sha(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(whole.sha256, sha256(&image));
    assert_eq!(
        run(digest(
            &view,
            Span::new(0, len + 1),
            sha(),
            &mut Yield,
            &mut || {}
        )),
        Err(ViewError::OutOfRange)
    );
    assert_eq!(
        slot.descriptor(&model).unwrap().unwrap().version().to_str(),
        Some("1.2.3")
    );

    // No read, of the check, the view, the digest or the descriptor, reached
    // a byte after the image.
    let end = layout::OTA_1.offset + len;
    for call in flash.device().calls() {
        if let Call::Read(address, read) = call {
            assert!(address + read <= end, "{call:?}");
        }
    }
}

#[test]
fn an_image_without_an_appended_digest_publishes_nothing() {
    let mut builder = app_image();
    builder.header[23] = 0;
    let (slot, _writer, _flash) = alternate_over(&builder.build());
    check(&slot);
    let model = slot.model().unwrap();
    assert_eq!(model.image(), Err(ImageFailure::NoDigest));
    assert_eq!(slot.published(&model).unwrap_err(), Unpublished::Unverified);
}

#[test]
fn a_found_app_over_leftover_data_publishes_nothing() {
    let mut mismatched = image();
    let last = mismatched.len() - 1;
    mismatched[last] ^= 0x80;
    let leftover = data(&mut Rng::new(0x1EF7), 0x4000);
    for (bytes, failure) in [
        (mismatched, ImageFailure::DigestMismatch),
        (leftover, ImageFailure::Magic),
    ] {
        let span = Span::new(layout::OTA_1.offset, bytes.len() as u32);
        let device = RamNor::with(layout::FLASH_SIZE, &[(span.offset, &bytes)]);
        let flash = TestFlash::new(device);
        let slot = AppSlot::new(flash.reader::<FoundApp>(span));
        let app = App::found(Some(&slot));
        assert!(app.current().is_none() && app.alternate().is_none());
        let found = app.found_app().unwrap();
        check(found);
        let model = found.model().unwrap();
        assert_eq!(model.image(), Err(failure));
        assert_eq!(
            found.published(&model).unwrap_err(),
            Unpublished::Unverified
        );
    }
}

#[test]
fn a_found_app_that_verifies_is_published() {
    let image = image();
    let span = Span::new(layout::OTA_1.offset, image.len() as u32);
    let device = RamNor::with(layout::FLASH_SIZE, &[(span.offset, &image)]);
    let flash = TestFlash::new(device);
    let slot = AppSlot::new(flash.reader::<FoundApp>(span));
    let found = App::found(Some(&slot)).found_app().unwrap();
    check(found);
    let model = found.model().unwrap();
    assert_eq!(served(&found.published(&model).unwrap()), image);
}

#[test]
fn the_check_runs_again_after_a_write_and_the_verdict_follows_the_epoch() {
    let first = image();
    let second = other_image();
    let (slot, mut writer, _flash) = alternate_over(&first);
    check(&slot);
    let before = slot.model().unwrap();
    assert_eq!(run(slot.verdict(before.epoch())), Ok(before));
    let old_view = slot.published(&before).unwrap();

    assert_eq!(slot.write_state(), WriteState::Stable(before.epoch()));
    rewrite(&mut writer, &second);
    let epoch = writer.write_state().epoch();
    assert_ne!(epoch, before.epoch());
    assert_eq!(slot.write_state(), writer.write_state());
    // The old epoch is stale, and so is what was published at it.
    assert_eq!(run(slot.verdict(before.epoch())), Err(LoadError::Stale));
    assert_eq!(slot.model(), None);
    assert_eq!(slot.published(&before).unwrap_err(), Unpublished::Stale);
    assert_eq!(run(old_view.read(0, &mut [0; 4])), Err(ViewError::Stale));

    // The new epoch waits for the check.
    let mut verdict = pin!(slot.verdict(epoch));
    assert!(poll_once(verdict.as_mut()).is_pending());
    assert!(poll_once(verdict.as_mut()).is_pending());
    check(&slot);
    let Poll::Ready(Ok(after)) = poll_once(verdict.as_mut()) else {
        panic!("no verdict");
    };
    assert_eq!(after.epoch(), epoch);
    assert_eq!(after.image().unwrap().file_sha256(), &sha256(&second));
    assert_eq!(served(&slot.published(&after).unwrap()), second);
}

#[test]
fn a_verdict_waiting_while_a_session_opens_is_stale() {
    let (slot, mut writer, _flash) = alternate_over(&image());
    let epoch = writer.write_state().epoch();
    let mut verdict = pin!(slot.verdict(epoch));
    assert!(poll_once(verdict.as_mut()).is_pending());
    let session = writer.begin();
    assert_eq!(
        poll_once(verdict.as_mut()),
        Poll::Ready(Err(LoadError::Stale))
    );
    drop(session);
    // A session that wrote nothing leaves the epoch, which then verifies.
    check(&slot);
    assert_eq!(
        run(slot.verdict(epoch)).map(|model| model.epoch()),
        Ok(epoch)
    );
}

#[test]
fn the_check_waits_out_an_open_session() {
    let image = image();
    let (slot, mut writer, _flash) = alternate_over(&image);
    let mut session = writer.begin();
    run(session.erase(0, SECTOR_SIZE)).unwrap();
    let (sha, mut pace, mut progress) = (sha(), Yield, || {});
    let mut check = pin!(slot.check(&sha, &mut pace, &mut progress));
    for _ in 0..10 {
        assert!(poll_once(check.as_mut()).is_pending());
    }
    assert_eq!(slot.model(), None);
    drop(session);
    let mut polls = 0;
    while poll_once(check.as_mut()).is_pending() {
        polls += 1;
        assert!(polls < 1_000_000);
    }
    let model = slot.model().unwrap();
    // The first sector is erased now.
    assert_eq!(model.image(), Err(ImageFailure::Magic));
    assert_eq!(Some(model.epoch()), Some(slot.reader.write_state().epoch()));
}

#[test]
fn a_session_during_the_check_keeps_no_result_and_checks_again() {
    let image = image();
    let (slot, mut writer, flash) = alternate_over(&image);
    // Before the 31st read of the check, a few KiB in: a session programs a
    // word in the tail, past the image.
    let past = (image.len() as u32).next_multiple_of(4) + 0x100;
    flash.device().before_call(30, move || {
        run(writer.begin().program(past, &[0; 4])).unwrap();
    });
    let mut pace = Counting::default();
    run(slot.check(&sha(), &mut pace, &mut || {}));
    let model = slot.model().unwrap();
    assert_eq!(model.epoch(), slot.reader.write_state().epoch());
    assert_eq!(model.image().unwrap().file_sha256(), &sha256(&image));
    // Both runs paused, the first cut short.
    assert!(pace.pauses > (image.len() - 32) / 1024);
}

#[test]
fn the_check_paces_and_reports_progress_per_kib() {
    let image = image();
    let (slot, _writer, _flash) = alternate_over(&image);
    let mut pace = Counting::default();
    let mut advanced = 0;
    run(slot.check(&sha(), &mut pace, &mut || advanced += 1));
    assert_eq!(pace.pauses, (image.len() - 32) / 1024);
    assert_eq!(advanced, pace.pauses);
    // Checked once: a second check finds the result kept.
    run(slot.check(&sha(), &mut pace, &mut || advanced += 1));
    assert_eq!(advanced, pace.pauses);
    assert_eq!(pace.pauses, (image.len() - 32) / 1024);
}

#[test]
fn run_checks_every_slot_and_again_after_a_write() {
    let first = image();
    let second = other_image();
    let device = RamNor::with(
        layout::FLASH_SIZE,
        &[
            (layout::OTA_0.offset, &first),
            (layout::OTA_1.offset, &first),
        ],
    );
    let flash = TestFlash::new(device);
    let current = AppSlot::new(flash.reader(Span::from(layout::OTA_0)));
    let alternate = AppSlot::new(flash.reader(Span::from(layout::OTA_1)));
    let app = App::layout(&current, &alternate);
    let mut writer = flash.writer::<AlternateApp>(Span::from(layout::OTA_1), Slot::Ota0);
    let current = app.current().unwrap();
    let alternate = app.alternate().unwrap();
    let mut pace = Yield;
    let mut progress = || {};
    let (before, after) = drive(app.run(sha(), &mut pace, &mut progress), async {
        let epoch = current.reader.write_state().epoch();
        let current = current.verdict(epoch).await.unwrap();
        assert_eq!(current.image().unwrap().file_sha256(), &sha256(&first));
        let before = alternate
            .verdict(alternate.reader.write_state().epoch())
            .await
            .unwrap();
        rewrite(&mut writer, &second);
        let after = alternate
            .verdict(writer.write_state().epoch())
            .await
            .unwrap();
        (before, after)
    });
    assert_eq!(before.image().unwrap().file_sha256(), &sha256(&first));
    assert_eq!(after.image().unwrap().file_sha256(), &sha256(&second));
    assert_ne!(before.epoch(), after.epoch());
}

#[test]
fn run_with_no_slot_waits_for_good() {
    let app = App::found(None);
    assert!(app.found_app().is_none());
    let mut pace = Yield;
    let mut progress = || {};
    let mut run = pin!(app.run(sha(), &mut pace, &mut progress));
    for _ in 0..10 {
        assert!(poll_once(run.as_mut()).is_pending());
    }
}

#[test]
fn the_modules_can_be_shared_from_a_static() {
    fn shared<T: Sync + Send>() {}
    shared::<App<'static, 'static>>();
    shared::<AppSlot<'static, AlternateApp>>();
}
