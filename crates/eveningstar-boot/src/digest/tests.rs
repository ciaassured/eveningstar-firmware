#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use eveningstar_esp_formats::pace::Yield;
use eveningstar_esp_formats::test_support::{Rng, data};
use eveningstar_storage::roles::AlternateApp;
use eveningstar_storage::test_support::{Call, RamNor, TestFlash, block_on, run};
use eveningstar_storage::{Slot, layout};

use super::*;
use crate::test_support::{Counting, sha256};

/// Length of the bytes the tests publish.
const LEN: u32 = 5000;

/// The span of the published bytes that is sanitized.
const SANITIZED: Span = Span::new(100, 50);

/// A flash holding `LEN` random bytes at the start of `ota_1`, and those
/// bytes.
fn flash() -> (TestFlash, Vec<u8>) {
    let bytes = data(&mut Rng::new(0xD16E), LEN as usize);
    let device = RamNor::with(layout::FLASH_SIZE, &[(layout::OTA_1.offset, &bytes)]);
    (TestFlash::new(device), bytes)
}

/// The bytes the view serves: `bytes` with the sanitized span as `0xFF`.
fn served(bytes: &[u8]) -> Vec<u8> {
    let mut served = bytes.to_vec();
    let at = SANITIZED.offset as usize;
    served[at..at + SANITIZED.len as usize].fill(0xFF);
    served
}

fn sha() -> sha2::Sha256 {
    sha2::Sha256::default()
}

#[test]
fn the_digest_is_the_sha256_of_the_bytes_as_served() {
    let (flash, bytes) = flash();
    let reader = flash.reader::<AlternateApp>(Span::from(layout::OTA_1));
    let epoch = reader.write_state().epoch();
    let view = reader
        .publish(epoch, Span::new(0, LEN), &[SANITIZED])
        .unwrap();
    assert_eq!(view.sanitized(), [SANITIZED]);
    let served = served(&bytes);

    let whole = run(digest(
        &view,
        Span::new(0, LEN),
        sha(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(whole.sha256, sha256(&served));
    assert_ne!(whole.sha256, sha256(&bytes));
    assert_eq!(whole.epoch, epoch);

    // Sub-ranges, across the sanitized span and the 256-byte reads.
    for (offset, len) in [
        (0, 0),
        (0, 1),
        (99, 2),
        (100, 50),
        (149, 300),
        (255, 257),
        (4000, 1000),
        (LEN, 0),
    ] {
        let part = run(digest(
            &view,
            Span::new(offset, len),
            sha(),
            &mut Yield,
            &mut || {},
        ))
        .unwrap();
        let range = offset as usize..(offset + len) as usize;
        assert_eq!(part.sha256, sha256(&served[range]), "{offset} {len}");
    }
}

#[test]
fn a_range_outside_the_view_is_refused_before_anything_is_read() {
    let (flash, _) = flash();
    let reader = flash.reader::<AlternateApp>(Span::from(layout::OTA_1));
    let view = reader
        .publish(reader.write_state().epoch(), Span::new(0, LEN), &[])
        .unwrap();
    let calls = flash.device().calls().len();
    for span in [
        Span::new(0, LEN + 1),
        Span::new(LEN, 1),
        Span::new(LEN + 1, 0),
        Span::new(1, u32::MAX),
        Span::new(u32::MAX, 1),
    ] {
        assert_eq!(
            run(digest(&view, span, sha(), &mut Yield, &mut || {})),
            Err(ViewError::OutOfRange),
            "{span:?}"
        );
    }
    assert_eq!(flash.device().calls().len(), calls);
}

#[test]
fn the_digest_pauses_and_reports_progress_after_each_kib() {
    let (flash, _) = flash();
    let reader = flash.reader::<AlternateApp>(Span::from(layout::OTA_1));
    let view = reader
        .publish(reader.write_state().epoch(), Span::new(0, LEN), &[])
        .unwrap();
    let mut pace = Counting::default();
    let mut advanced = 0;
    let run = block_on(digest(
        &view,
        Span::new(0, LEN),
        sha(),
        &mut pace,
        &mut || advanced += 1,
    ));
    run.output.unwrap();
    assert_eq!(pace.pauses, (LEN / PAUSE_INTERVAL) as usize);
    assert_eq!(advanced, pace.pauses);
    // The pace given never yields, and the reads are shorter than the
    // view's 4 KiB steps.
    assert_eq!((run.polls, run.wakes), (1, 0));
    // Reads of 256 bytes.
    let reads: Vec<_> = flash
        .device()
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Read(_, len) => Some(len),
            _ => None,
        })
        .collect();
    assert!(reads.iter().all(|len| *len <= 256), "{reads:?}");
}

#[test]
fn a_digest_of_a_written_region_is_stale() {
    let (flash, _) = flash();
    let span = Span::from(layout::OTA_1);
    let reader = flash.reader::<AlternateApp>(span);
    let mut writer = flash.writer::<AlternateApp>(span, Slot::Ota0);
    let view = reader
        .publish(reader.write_state().epoch(), Span::new(0, LEN), &[])
        .unwrap();
    run(writer.begin().program(LEN + 4096, &[0; 4])).unwrap();
    assert_eq!(
        run(digest(
            &view,
            Span::new(0, 1),
            sha(),
            &mut Yield,
            &mut || {}
        )),
        Err(ViewError::Stale)
    );
}

#[test]
fn a_session_during_the_digest_makes_it_stale() {
    let (flash, _) = flash();
    let span = Span::from(layout::OTA_1);
    let reader = flash.reader::<AlternateApp>(span);
    let mut writer = flash.writer::<AlternateApp>(span, Slot::Ota0);
    let view = reader
        .publish(reader.write_state().epoch(), Span::new(0, LEN), &[])
        .unwrap();
    // Before the fifth read: a session programs a word past the view.
    flash.device().before_call(4, move || {
        run(writer.begin().program(LEN + 4096, &[0; 4])).unwrap();
    });
    assert_eq!(
        run(digest(
            &view,
            Span::new(0, LEN),
            sha(),
            &mut Yield,
            &mut || {}
        )),
        Err(ViewError::Stale)
    );
}

#[test]
fn a_session_that_wrote_nothing_leaves_the_digest_valid() {
    let (flash, bytes) = flash();
    let span = Span::from(layout::OTA_1);
    let reader = flash.reader::<AlternateApp>(span);
    let mut writer = flash.writer::<AlternateApp>(span, Slot::Ota0);
    let epoch = reader.write_state().epoch();
    let view = reader.publish(epoch, Span::new(0, LEN), &[]).unwrap();
    drop(writer.begin());
    let whole = run(digest(
        &view,
        Span::new(0, LEN),
        sha(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(
        whole,
        Digest {
            sha256: sha256(&bytes),
            epoch
        }
    );
}
