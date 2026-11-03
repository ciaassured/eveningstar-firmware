#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, Waker};

use super::*;
use crate::layout::{FACTORY, FLASH_SIZE, OTA_0, OTA_1, RESERVED, TABLE_OFFSET};
use crate::roles::{
    AlternateApp, AlternateConfig, Bootloader, Chip, CurrentConfig, Diag, FoundApp, Internal,
    Otadata, PartitionTable, Programmable,
};
use crate::test_support::{Call, RamNor, TestFlash, block_on, run};
use crate::{
    AccessError, MAX_SANITIZED, PublicRead, PublishError, ReadError, Slot, SnapshotError, Source,
    Span, ViewError, WriteState, layout,
};

const SECTOR: u32 = SECTOR_SIZE;

fn flash() -> TestFlash {
    TestFlash::new(RamNor::erased(FLASH_SIZE))
}

fn diag(flash: &TestFlash) -> Writer<'static, Diag> {
    flash.writer(Span::from(layout::DIAG), Slot::Ota0)
}

/// The alternate slot's reader and writer while `ota_0` runs.
fn alternate(flash: &TestFlash) -> (Reader<'static, AlternateApp>, Writer<'static, AlternateApp>) {
    let span = Span::from(OTA_1);
    (flash.reader(span), flash.writer(span, Slot::Ota0))
}

/// The epoch of a state that has to be stable.
fn stable(state: WriteState) -> crate::Epoch {
    assert!(state.is_stable(), "{state:?}");
    state.epoch()
}

/// Polls `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

// Making handles.

#[test]
fn a_reader_lies_inside_the_device() {
    let flash = TestFlash::new(RamNor::erased(0x4000));
    let state = State::new();
    let device = flash.device();
    assert!(Reader::<Otadata>::new(device, &state, Span::new(0, 0x4000)).is_some());
    assert!(Reader::<Otadata>::new(device, &state, Span::new(0x4000, 0)).is_some());
    assert!(Reader::<Otadata>::new(device, &state, Span::new(0x3FFF, 2)).is_none());
    assert!(Reader::<Otadata>::new(device, &state, Span::new(1, u32::MAX)).is_none());
}

#[test]
fn a_writer_lies_inside_one_physical_span() {
    let flash = flash();
    let state = State::new();
    let device = flash.device();
    let writer = |span| Writer::<Diag>::new(device, &state, span, Slot::Ota0);
    assert!(writer(Span::from(layout::DIAG)).is_some());
    assert!(writer(Span::new(layout::DIAG.offset + SECTOR, SECTOR)).is_some());
    // Across two partitions, in the gap after the otadata, past the layout.
    assert!(writer(Span::new(layout::OTADATA.offset, 3 * SECTOR)).is_none());
    assert!(writer(Span::new(0xC000, SECTOR)).is_none());
    assert!(writer(Span::new(FLASH_SIZE, SECTOR)).is_none());
    // Inside the layout but past a smaller device.
    let small = TestFlash::new(RamNor::erased(0x10_0000));
    assert!(
        Writer::<Diag>::new(small.device(), &state, Span::from(layout::DIAG), Slot::Ota0).is_none()
    );
}

#[test]
fn handles_report_their_span() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    assert_eq!(reader.span(), Span::from(OTA_1));
    assert_eq!(reader.len(), OTA_1.size);
    assert!(!reader.is_empty());
    assert_eq!(writer.span(), Span::from(OTA_1));
    assert_eq!(writer.len(), OTA_1.size);
    assert!(!writer.is_empty());
    let session = writer.begin();
    assert_eq!(session.span(), Span::from(OTA_1));
    assert_eq!(session.len(), OTA_1.size);
    assert!(!session.is_empty());
    let empty: Reader<'_, Otadata> = flash.reader(Span::new(0x1000, 0));
    assert!(empty.is_empty());
}

// Direct reads of a writer.

#[test]
fn a_writer_reads_its_region_directly() {
    let flash = flash();
    flash
        .device()
        .poke(layout::DIAG.offset + 0x1FFE, &[1, 2, 3, 4]);
    let writer = diag(&flash);

    let mut buf = [0; 4];
    Source::read(&writer, 0x1FFE, &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4]);
    assert_eq!(writer.size(), layout::DIAG.size);

    let mut big = vec![0; 0x2004];
    let read = block_on(writer.read_async(0x2, &mut big));
    read.output.unwrap();
    assert_eq!(big[0x1FFC..0x2000], [1, 2, 3, 4]);
    // One device call and one yield per full 4 KiB.
    assert_eq!(read.wakes, 2);
    assert_eq!(
        flash.device().calls()[1..],
        [
            Call::Read(layout::DIAG.offset + 2, 0x1000),
            Call::Read(layout::DIAG.offset + 0x1002, 0x1000),
            Call::Read(layout::DIAG.offset + 0x2002, 4),
        ]
    );
}

#[test]
fn a_writer_refuses_reads_past_its_region() {
    let flash = flash();
    let writer = diag(&flash);
    let mut buf = [7; 4];
    let end = layout::DIAG.size;
    assert_eq!(
        Source::read(&writer, end - 3, &mut buf),
        Err(ReadError::OutOfRange)
    );
    assert_eq!(
        Source::read(&writer, u32::MAX, &mut buf),
        Err(ReadError::OutOfRange)
    );
    assert_eq!(
        run(writer.read_async(end - 3, &mut buf)),
        Err(ReadError::OutOfRange)
    );
    assert_eq!(buf, [0; 4], "a failed read keeps no bytes");
    Source::read(&writer, end, &mut []).unwrap();
    assert!(
        flash
            .device()
            .calls()
            .iter()
            .all(|call| *call == Call::Read(layout::DIAG.end(), 0))
    );
}

#[test]
fn a_session_reads_what_it_wrote() {
    let flash = flash();
    let mut writer = diag(&flash);
    let mut session = writer.begin();
    run(session.program(8, &[1, 2, 3, 4])).unwrap();
    let mut buf = [0; 6];
    Source::read(&session, 7, &mut buf).unwrap();
    assert_eq!(buf, [0xFF, 1, 2, 3, 4, 0xFF]);
    run(session.read_async(7, &mut buf)).unwrap();
    assert_eq!(buf, [0xFF, 1, 2, 3, 4, 0xFF]);
    assert_eq!(session.size(), layout::DIAG.size);
}

// Write state.

#[test]
fn a_session_marks_its_region_written_to_while_it_lives() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let before = stable(reader.write_state());
    let session = writer.begin();
    assert_eq!(reader.write_state(), WriteState::Writing(before));
    drop(session);
    assert_eq!(reader.write_state(), WriteState::Stable(before));
    assert_eq!(writer.write_state(), WriteState::Stable(before));
}

#[test]
fn a_session_that_issued_a_call_ends_at_a_new_epoch() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let before = stable(reader.write_state());
    let mut session = writer.begin();
    run(session.program(0, &[0; 4])).unwrap();
    assert_eq!(reader.write_state(), WriteState::Writing(before));
    drop(session);
    assert_ne!(stable(reader.write_state()), before);

    let before = stable(reader.write_state());
    run(writer.begin().erase(0, SECTOR)).unwrap();
    assert_ne!(stable(reader.write_state()), before);
}

#[test]
fn a_finished_session_returns_the_state_it_leaves() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let before = reader.write_state();
    // Nothing issued: the epoch it started from.
    assert_eq!(writer.begin().finish(), before);
    let mut session = writer.begin();
    run(session.program(0, &[0; 4])).unwrap();
    let after = session.finish();
    assert!(after.is_stable());
    assert_ne!(after, before);
    assert_eq!(reader.write_state(), after);
    assert_eq!(writer.write_state(), after);
}

#[test]
fn a_session_dropped_mid_erase_ends_at_a_new_epoch() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let before = stable(reader.write_state());
    let mut session = writer.begin();
    {
        let mut erase = pin!(session.erase(0, 3 * SECTOR));
        assert!(poll_once(erase.as_mut()).is_pending());
        // The erase is cancelled at its first yield, one sector in.
    }
    assert_eq!(flash.device().writes(), [Call::Erase(OTA_1.offset)]);
    assert_eq!(reader.write_state(), WriteState::Writing(before));
    drop(session);
    assert_ne!(stable(reader.write_state()), before);
}

#[test]
fn a_session_whose_calls_were_refused_ends_at_its_epoch() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let before = stable(reader.write_state());
    {
        let mut session = writer.begin();
        assert_eq!(run(session.erase(0, 1)), Err(AccessError::NotAligned));
        assert_eq!(run(session.erase(SECTOR, 0)), Err(AccessError::OutOfRange));
        assert_eq!(
            run(session.erase(0, OTA_1.size + SECTOR)),
            Err(AccessError::OutOfRange)
        );
        assert_eq!(
            run(session.program(2, &[0; 4])),
            Err(AccessError::NotAligned)
        );
        assert_eq!(
            run(session.program(0, &[0; 3])),
            Err(AccessError::NotAligned)
        );
        assert_eq!(
            run(session.program(OTA_1.size - 4, &[0; 8])),
            Err(AccessError::OutOfRange)
        );
        // Calls with nothing to do issue no device call either.
        run(session.erase(SECTOR, SECTOR)).unwrap();
        run(session.program(4, &[])).unwrap();
    }
    assert_eq!(flash.device().writes(), []);
    assert_eq!(reader.write_state(), WriteState::Stable(before));

    // The guard refuses before the device too.
    let current: Writer<'_, AlternateApp> = flash.writer(Span::from(OTA_0), Slot::Ota0);
    let current_state: Reader<'_, AlternateApp> = flash.reader(Span::from(OTA_0));
    let before = stable(current_state.write_state());
    let mut current = current;
    {
        let mut session = current.begin();
        assert_eq!(run(session.erase(0, SECTOR)), Err(AccessError::Protected));
        assert_eq!(
            run(session.program(0, &[0; 4])),
            Err(AccessError::Protected)
        );
    }
    assert_eq!(flash.device().writes(), []);
    assert_eq!(current_state.write_state(), WriteState::Stable(before));
}

#[test]
fn a_failed_device_call_counts_as_issued() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    flash.device().fail_writes(true);
    let before = stable(reader.write_state());
    assert_eq!(
        run(writer.begin().erase(0, SECTOR)),
        Err(AccessError::Device)
    );
    let after_erase = stable(reader.write_state());
    assert_ne!(after_erase, before);
    assert_eq!(
        run(writer.begin().program(0, &[0; 4])),
        Err(AccessError::Device)
    );
    assert_ne!(stable(reader.write_state()), after_erase);
}

#[test]
fn epochs_never_repeat() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let mut seen = vec![stable(reader.write_state())];
    for i in 0..64_u32 {
        let before = stable(reader.write_state());
        // A session that writes nothing keeps the epoch.
        drop(writer.begin());
        assert_eq!(stable(reader.write_state()), before);
        run(writer.begin().program(4 * i, &[0; 4])).unwrap();
        let after = stable(reader.write_state());
        assert!(!seen.contains(&after), "{after:?} repeats");
        seen.push(after);
    }
}

#[test]
fn a_session_changes_only_the_epoch_of_its_spans() {
    let flash = flash();
    let (reader, _) = alternate(&flash);
    let mut config: Writer<'_, CurrentConfig> =
        flash.writer(Span::from(layout::CONFIG_0), Slot::Ota0);
    let whole: Reader<'_, Chip> = flash.reader(Span::new(0, FLASH_SIZE));
    let before = stable(reader.write_state());
    let whole_before = stable(whole.write_state());
    run(config.begin().erase(0, SECTOR)).unwrap();
    assert_eq!(stable(reader.write_state()), before);
    assert_ne!(stable(whole.write_state()), whole_before);
}

// Erases and programs.

#[test]
fn an_erase_runs_a_sector_per_call_and_yields_after_each() {
    let flash = flash();
    flash
        .device()
        .poke(layout::DIAG.offset + 0x1000, &[0; 0x2000]);
    let mut writer = diag(&flash);
    let mut session = writer.begin();
    let erase = block_on(session.erase(0x1000, 0x3000));
    erase.output.unwrap();
    assert_eq!(erase.wakes, 2);
    assert_eq!(
        flash.device().writes(),
        [
            Call::Erase(layout::DIAG.offset + 0x1000),
            Call::Erase(layout::DIAG.offset + 0x2000),
        ]
    );
    assert!(
        flash
            .device()
            .at(layout::DIAG.offset, 0x4000)
            .iter()
            .all(|b| *b == 0xFF)
    );
}

#[test]
fn a_program_runs_at_most_4_kib_per_call_and_ands_the_bytes_in() {
    let flash = flash();
    let mut writer = diag(&flash);
    let mut session = writer.begin();
    run(session.program(0, &[0xF0; 4])).unwrap();
    run(session.program(0, &[0x3C; 4])).unwrap();
    assert_eq!(flash.device().at(layout::DIAG.offset, 4), [0x30; 4]);

    let bytes = vec![0x5A; 0x2800];
    let program = block_on(session.program(0x1000, &bytes));
    program.output.unwrap();
    assert_eq!(program.wakes, 3);
    let at = layout::DIAG.offset + 0x1000;
    assert_eq!(
        flash.device().writes()[2..],
        [
            Call::Program(at, 0x1000),
            Call::Program(at + 0x1000, 0x1000),
            Call::Program(at + 0x2000, 0x800),
        ]
    );
    assert_eq!(flash.device().at(at, 0x2800), bytes);
}

#[test]
fn the_alternate_configuration_is_erased() {
    let flash = flash();
    flash.device().poke(layout::CONFIG_1.offset, &[0; 8]);
    let mut config: Writer<'_, AlternateConfig> =
        flash.writer(Span::from(layout::CONFIG_1), Slot::Ota0);
    run(config.begin().erase(0, SECTOR)).unwrap();
    assert_eq!(flash.device().at(layout::CONFIG_1.offset, 8), [0xFF; 8]);
}

#[test]
fn the_guard_refuses_every_protected_span() {
    let flash = flash();
    // Writers over protected spans exist only in tests: each lies inside one
    // physical span, and the guard refuses it.
    let protected = [
        Span::new(0, TABLE_OFFSET),
        Span::new(TABLE_OFFSET, SECTOR),
        Span::from(FACTORY),
        Span::from(RESERVED),
    ];
    for span in protected {
        let mut writer: Writer<'_, Diag> = flash.writer(span, Slot::Ota0);
        let mut session = writer.begin();
        assert_eq!(
            run(session.erase(0, SECTOR)),
            Err(AccessError::Protected),
            "{span:?}"
        );
        assert_eq!(
            run(session.program(span.len - 4, &[0; 4])),
            Err(AccessError::Protected),
            "{span:?}"
        );
    }
    // The running slot is protected, whichever it is, and the other isn't.
    for (running, slot) in [(Slot::Ota0, OTA_0), (Slot::Ota1, OTA_1)] {
        let mut writer: Writer<'_, AlternateApp> = flash.writer(Span::from(slot), running);
        assert_eq!(
            run(writer.begin().erase(0, SECTOR)),
            Err(AccessError::Protected)
        );
        let other = if running == Slot::Ota0 { OTA_1 } else { OTA_0 };
        let mut writer: Writer<'_, AlternateApp> = flash.writer(Span::from(other), running);
        run(writer.begin().erase(0, SECTOR)).unwrap();
    }
    assert_eq!(
        flash.device().writes(),
        [Call::Erase(OTA_1.offset), Call::Erase(OTA_0.offset)]
    );
}

// Snapshots.

/// Programs a word of zeros at `offset` of `writer`'s region in a session of
/// its own, polled to completion.
fn write_word<R: Programmable<Device = Internal>>(writer: &mut Writer<'_, R>, offset: u32) {
    run(writer.begin().program(offset, &[0; 4])).unwrap();
}

/// The number of reads `flash`'s device received.
fn reads(flash: &TestFlash) -> usize {
    flash
        .device()
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::Read(..)))
        .count()
}

#[test]
fn a_snapshot_reads_the_region_at_its_epoch() {
    let flash = flash();
    flash.device().poke(OTA_1.offset + 0x100, b"image");
    let (reader, _) = alternate(&flash);
    let snapshot = reader.snapshot(Span::new(0x100, 0x10)).unwrap();
    assert_eq!(snapshot.epoch(), stable(reader.write_state()));
    assert_eq!(snapshot.span(), Span::new(0x100, 0x10));
    assert_eq!(snapshot.size(), 0x10);
    let mut buf = [0; 5];
    Source::read(&snapshot, 0, &mut buf).unwrap();
    assert_eq!(&buf, b"image");
    run(snapshot.read_async(0, &mut buf)).unwrap();
    assert_eq!(&buf, b"image");
    assert_eq!(
        Source::read(&snapshot, 0xC, &mut buf),
        Err(ReadError::OutOfRange)
    );
    assert_eq!(buf, [0; 5]);
    assert_eq!(
        run(snapshot.read_async(0xC, &mut buf)),
        Err(ReadError::OutOfRange)
    );
}

#[test]
fn a_snapshot_lies_inside_its_region() {
    let flash = flash();
    let (reader, _) = alternate(&flash);
    assert!(reader.snapshot(Span::new(0, OTA_1.size)).is_ok());
    assert!(reader.snapshot(Span::new(OTA_1.size, 0)).is_ok());
    assert_eq!(
        reader.snapshot(Span::new(OTA_1.size, 1)).unwrap_err(),
        SnapshotError::OutOfRange
    );
    assert_eq!(
        reader.snapshot(Span::new(1, u32::MAX)).unwrap_err(),
        SnapshotError::OutOfRange
    );
}

#[test]
fn no_snapshot_is_taken_during_a_session() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let session = writer.begin();
    assert_eq!(
        reader.snapshot(Span::new(0, 4)).unwrap_err(),
        SnapshotError::Busy
    );
    drop(session);
    assert!(reader.snapshot(Span::new(0, 4)).is_ok());
}

#[test]
fn a_snapshot_fails_once_the_region_is_written() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let snapshot = reader.snapshot(Span::new(0, 8)).unwrap();
    let mut buf = [0; 8];

    let session = writer.begin();
    assert_eq!(Source::read(&snapshot, 0, &mut buf), Err(ReadError::Stale));
    drop(session);
    // A session that issued no device call left the bytes as they were.
    Source::read(&snapshot, 0, &mut buf).unwrap();

    write_word(&mut writer, 4);
    buf = [7; 8];
    assert_eq!(Source::read(&snapshot, 0, &mut buf), Err(ReadError::Stale));
    assert_eq!(buf, [0; 8], "a stale read keeps no bytes");
    assert_eq!(run(snapshot.read_async(0, &mut buf)), Err(ReadError::Stale));
    // A read of a region written since checks the state before it reaches
    // the device: only the read between the sessions did.
    assert_eq!(reads(&flash), 1);

    let fresh = reader.snapshot(Span::new(0, 8)).unwrap();
    Source::read(&fresh, 0, &mut buf).unwrap();
    assert_eq!(buf, [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
}

#[test]
fn a_read_that_straddles_a_session_is_stale() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let snapshot = reader.snapshot(Span::new(0, 8)).unwrap();
    // Another task's session runs between the read's first load of the state
    // and its device call.
    flash
        .device()
        .before_call(0, move || write_word(&mut writer, 0));
    let mut buf = [7; 8];
    assert_eq!(Source::read(&snapshot, 0, &mut buf), Err(ReadError::Stale));
    assert_eq!(buf, [0; 8]);
    assert_eq!(flash.device().at(OTA_1.offset, 4), [0; 4]);
}

#[test]
fn a_read_that_straddles_a_session_without_calls_reads() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let snapshot = reader.snapshot(Span::new(0, 8)).unwrap();
    flash.device().before_call(0, move || drop(writer.begin()));
    let mut buf = [0; 8];
    Source::read(&snapshot, 0, &mut buf).unwrap();
    assert_eq!(buf, [0xFF; 8]);
}

#[test]
fn a_chunked_read_that_straddles_a_session_is_stale() {
    let flash = flash();
    flash.device().poke(OTA_1.offset, &[0x11; 0x3000]);
    let (reader, mut writer) = alternate(&flash);
    let snapshot = reader.snapshot(Span::new(0, 0x3000)).unwrap();
    // The session runs before the read's second chunk, after the first
    // chunk's bytes were read and checked.
    flash
        .device()
        .before_call(1, move || write_word(&mut writer, 0x2000));
    let mut buf = vec![0; 0x3000];
    assert_eq!(run(snapshot.read_async(0, &mut buf)), Err(ReadError::Stale));
    assert!(
        buf.iter().all(|b| *b == 0),
        "no chunk of a stale read is kept"
    );
    assert_eq!(reads(&flash), 2);
}

#[test]
fn a_snapshot_over_several_spans_fails_when_any_is_written() {
    let flash = flash();
    let (_, mut app) = alternate(&flash);
    let mut config: Writer<'_, CurrentConfig> =
        flash.writer(Span::from(layout::CONFIG_0), Slot::Ota0);
    let mut otadata: Writer<'_, Otadata> = flash.writer(Span::from(layout::OTADATA), Slot::Ota0);
    let chip: Reader<'_, Chip> = flash.reader(Span::new(0, FLASH_SIZE));
    // A found app from the otadata into the first slot, across the gap after
    // the otadata.
    let found: Reader<'_, FoundApp> = flash.reader(Span::new(0xB000, 0x6000));
    let mut buf = [0; 4];

    let chip_snapshot = chip.snapshot(Span::new(0, 4)).unwrap();
    let found_snapshot = found.snapshot(Span::new(0x5000, 4)).unwrap();
    write_word(&mut app, 0);
    // A write anywhere in the chip fails its snapshot, even outside the
    // snapshot's own span; the found app doesn't cover the alternate slot.
    assert_eq!(
        Source::read(&chip_snapshot, 0, &mut buf),
        Err(ReadError::Stale)
    );
    Source::read(&found_snapshot, 0, &mut buf).unwrap();

    // Each span the found app covers counts.
    write_word(&mut otadata, 0x1000);
    assert_eq!(
        Source::read(&found_snapshot, 0, &mut buf),
        Err(ReadError::Stale)
    );
    let found_snapshot = found.snapshot(Span::new(0, 4)).unwrap();
    let mut current: Writer<'_, AlternateApp> = flash.writer(Span::from(OTA_0), Slot::Ota1);
    write_word(&mut current, 0x1000);
    assert_eq!(
        Source::read(&found_snapshot, 0, &mut buf),
        Err(ReadError::Stale)
    );

    let found_snapshot = found.snapshot(Span::new(0, 4)).unwrap();
    let session = config.begin();
    assert_eq!(
        chip.snapshot(Span::new(0, 4)).unwrap_err(),
        SnapshotError::Busy
    );
    Source::read(&found_snapshot, 0, &mut buf).unwrap();
    drop(session);
}

// Publication.

/// Reads the whole of `view`, polled to completion.
fn read_view(view: &impl PublicRead) -> Result<Vec<u8>, ViewError> {
    let mut buf = vec![0; view.len() as usize];
    run(view.read(0, &mut buf)).map(|()| buf)
}

#[test]
fn a_view_serves_the_region_at_its_epoch() {
    let flash = flash();
    flash.device().poke(OTA_1.offset + 0x10, b"0123456789");
    let (reader, _) = alternate(&flash);
    let epoch = stable(reader.write_state());
    let view = reader.publish(epoch, Span::new(0x10, 10), &[]).unwrap();
    assert_eq!(view.len(), 10);
    assert!(!view.is_empty());
    assert_eq!(view.epoch(), epoch);
    assert_eq!(view.span(), Span::new(0x10, 10));
    assert_eq!(view.sanitized(), []);
    assert_eq!(read_view(&view).unwrap(), b"0123456789");
    let mut buf = [0; 4];
    run(view.read(6, &mut buf)).unwrap();
    assert_eq!(&buf, b"6789");
    assert_eq!(run(view.read(7, &mut buf)), Err(ViewError::OutOfRange));
    assert_eq!(buf, [0; 4]);
}

#[test]
fn a_view_reads_in_4_kib_steps() {
    let flash = flash();
    let (reader, _) = alternate(&flash);
    let epoch = stable(reader.write_state());
    let view = reader.publish(epoch, Span::new(0, 0x2800), &[]).unwrap();
    let mut buf = vec![0; 0x2800];
    let read = block_on(view.read(0, &mut buf));
    read.output.unwrap();
    assert_eq!(read.wakes, 2);
    assert_eq!(reads(&flash), 3);
}

#[test]
fn publish_refuses_an_epoch_that_moved() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let epoch = stable(reader.write_state());
    write_word(&mut writer, 0);
    assert_eq!(
        reader.publish(epoch, Span::new(0, 4), &[]).unwrap_err(),
        PublishError::Stale
    );
    let epoch = stable(reader.write_state());
    let session = writer.begin();
    assert_eq!(
        reader.publish(epoch, Span::new(0, 4), &[]).unwrap_err(),
        PublishError::Stale
    );
    drop(session);
    assert!(reader.publish(epoch, Span::new(0, 4), &[]).is_ok());
}

#[test]
fn publish_refuses_another_regions_epoch() {
    let flash = flash();
    let (reader, _) = alternate(&flash);
    let table: Reader<'_, PartitionTable> = flash.reader(Span::new(TABLE_OFFSET, SECTOR));
    let other = stable(table.write_state());
    assert_eq!(
        reader.publish(other, Span::new(0, 4), &[]).unwrap_err(),
        PublishError::Stale
    );
}

#[test]
fn publish_refuses_spans_outside_the_region_and_the_view() {
    let flash = flash();
    let (reader, _) = alternate(&flash);
    let epoch = stable(reader.write_state());
    let publish = |span, sanitized: &[Span]| reader.publish(epoch, span, sanitized).map(|_| ());
    assert_eq!(
        publish(Span::new(OTA_1.size - 3, 4), &[]),
        Err(PublishError::OutOfRange)
    );
    assert_eq!(
        publish(Span::new(4, u32::MAX), &[]),
        Err(PublishError::OutOfRange)
    );
    assert_eq!(
        publish(Span::new(0x100, 0x10), &[Span::new(0xC, 5)]),
        Err(PublishError::SanitizedOutOfRange)
    );
    assert_eq!(
        publish(
            Span::new(0x100, 0x10),
            &[Span::new(0, 1), Span::new(1, u32::MAX)]
        ),
        Err(PublishError::SanitizedOutOfRange)
    );
    let four = [Span::new(0, 1); MAX_SANITIZED];
    publish(Span::new(0x100, 0x10), &four).unwrap();
    let five = [Span::new(0, 1); MAX_SANITIZED + 1];
    assert_eq!(
        publish(Span::new(0x100, 0x10), &five),
        Err(PublishError::TooManySanitized)
    );
    publish(Span::new(OTA_1.size, 0), &[Span::new(0, 0)]).unwrap();
}

#[test]
fn publish_refuses_every_sensitive_span() {
    let flash = flash();
    for entry in [FACTORY, layout::CONFIG_0, layout::CONFIG_1, layout::DIAG] {
        // A found region over the whole flash, as a table could place one.
        let found: Reader<'_, FoundApp> = flash.reader(Span::new(0, FLASH_SIZE));
        let epoch = stable(found.write_state());
        let publish = |span| found.publish(epoch, span, &[]).map(|_| ());
        assert_eq!(
            publish(Span::from(entry)),
            Err(PublishError::Sensitive),
            "{}",
            entry.label
        );
        assert_eq!(
            publish(Span::new(entry.offset - 4, 5)),
            Err(PublishError::Sensitive),
            "{}",
            entry.label
        );
        assert_eq!(
            publish(Span::new(entry.end() - 1, 5)),
            Err(PublishError::Sensitive),
            "{}",
            entry.label
        );
        assert_eq!(
            publish(Span::new(entry.offset + 8, 0)),
            Err(PublishError::Sensitive),
            "{}",
            entry.label
        );
        // A region found over the partition alone is refused just the same.
        let inside: Reader<'_, FoundApp> = flash.reader(Span::from(entry));
        let epoch = stable(inside.write_state());
        assert_eq!(
            inside.publish(epoch, Span::new(0, 4), &[]).unwrap_err(),
            PublishError::Sensitive
        );
    }
    // Bytes next to them, and the other partitions, publish.
    let found: Reader<'_, FoundApp> = flash.reader(Span::new(0, FLASH_SIZE));
    let epoch = stable(found.write_state());
    for span in [
        Span::new(FACTORY.offset - 4, 4),
        Span::new(FACTORY.end(), 4),
        Span::from(layout::OTADATA),
        Span::from(OTA_0),
        Span::new(OTA_1.offset, OTA_1.size),
        Span::new(layout::DIAG.end(), 4),
        Span::from(RESERVED),
    ] {
        assert!(found.publish(epoch, span, &[]).is_ok(), "{span:?}");
    }
}

#[test]
fn sanitized_bytes_are_served_as_erased() {
    let flash = flash();
    flash.device().poke(TABLE_OFFSET, &[0x11; 0x40]);
    // The second sanitized span holds a byte that isn't erased; the first
    // and the third are erased already.
    flash.device().poke(TABLE_OFFSET + 0x10, &[0xFF; 0x10]);
    flash.device().poke(TABLE_OFFSET + 0x2C, &[0xFF; 4]);
    flash.device().poke(TABLE_OFFSET + 0x3F, &[0xFF]);
    let table: Reader<'_, PartitionTable> = flash.reader(Span::new(TABLE_OFFSET, SECTOR));
    let epoch = stable(table.write_state());
    let sanitized = [
        Span::new(0x10, 0x10),
        Span::new(0x28, 8),
        Span::new(0x3F, 1),
    ];
    let view = table
        .publish(epoch, Span::new(0, 0x40), &sanitized)
        .unwrap();
    assert_eq!(view.sanitized(), [Span::new(0x28, 8)]);

    let mut expected = vec![0x11; 0x40];
    expected[0x10..0x20].fill(0xFF);
    expected[0x28..0x30].fill(0xFF);
    expected[0x3F] = 0xFF;
    assert_eq!(read_view(&view).unwrap(), expected);
    // A read that starts or ends inside a sanitized span.
    let mut buf = [0; 6];
    run(view.read(0x26, &mut buf)).unwrap();
    assert_eq!(buf, [0x11, 0x11, 0xFF, 0xFF, 0xFF, 0xFF]);
    run(view.read(0x2E, &mut buf)).unwrap();
    assert_eq!(buf, [0xFF, 0xFF, 0x11, 0x11, 0x11, 0x11]);
}

#[test]
fn a_view_fails_once_the_region_is_written() {
    let flash = flash();
    flash.device().poke(OTA_1.offset, &[0x22; 8]);
    let (reader, mut writer) = alternate(&flash);
    let epoch = stable(reader.write_state());
    let view = reader.publish(epoch, Span::new(0, 8), &[]).unwrap();
    write_word(&mut writer, 0x1000);
    let mut buf = [7; 8];
    assert_eq!(run(view.read(0, &mut buf)), Err(ViewError::Stale));
    assert_eq!(buf, [0; 8]);
}

#[test]
fn a_view_read_that_straddles_a_session_is_stale() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let epoch = stable(reader.write_state());
    let view = reader.publish(epoch, Span::new(0, 0x2000), &[]).unwrap();
    flash
        .device()
        .before_call(1, move || write_word(&mut writer, 0));
    assert_eq!(read_view(&view), Err(ViewError::Stale));
}

#[test]
fn publish_fails_when_a_session_straddles_the_sanitized_check() {
    let flash = flash();
    flash.device().poke(OTA_1.offset, &[0; 8]);
    let (reader, mut writer) = alternate(&flash);
    let epoch = stable(reader.write_state());
    // A session that issues no call leaves the epoch as it was.
    let mut idle: Writer<'_, AlternateApp> = flash.writer(Span::from(OTA_1), Slot::Ota0);
    flash.device().before_call(0, move || drop(idle.begin()));
    let view = reader
        .publish(epoch, Span::new(0, 0x10), &[Span::new(0, 4)])
        .unwrap();
    assert_eq!(view.sanitized(), [Span::new(0, 4)]);
    // One that writes fails the check of the sanitized span.
    flash
        .device()
        .before_call(0, move || write_word(&mut writer, 0x8));
    assert_eq!(
        reader
            .publish(epoch, Span::new(0, 0x10), &[Span::new(0, 4)])
            .unwrap_err(),
        PublishError::Stale
    );
}

#[test]
fn a_bootloader_view_lies_inside_its_region() {
    let flash = flash();
    flash.device().poke(0, &[0xE9; 4]);
    let bootloader: Reader<'_, Bootloader> = flash.reader(Span::new(0, TABLE_OFFSET));
    let epoch = stable(bootloader.write_state());
    let view = bootloader.publish(epoch, Span::new(0, 4), &[]).unwrap();
    assert_eq!(read_view(&view).unwrap(), [0xE9; 4]);
    assert_eq!(
        bootloader
            .publish(epoch, Span::new(0, TABLE_OFFSET + 1), &[])
            .unwrap_err(),
        PublishError::OutOfRange
    );
}

// Waiting for a change.

/// A waker that counts its wakes.
#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// A future and the waker that counts its wakes.
struct Waiting<F> {
    future: Pin<Box<F>>,
    counter: Arc<Counter>,
    waker: Waker,
}

impl<F: Future<Output = ()>> Waiting<F> {
    fn new(future: F) -> Self {
        let counter = Arc::new(Counter::default());
        Self {
            future: Box::pin(future),
            waker: Waker::from(counter.clone()),
            counter,
        }
    }

    /// Whether the future completes when polled.
    fn ready(&mut self) -> bool {
        self.future
            .as_mut()
            .poll(&mut Context::from_waker(&self.waker))
            .is_ready()
    }

    fn wakes(&self) -> usize {
        self.counter.0.load(Ordering::SeqCst)
    }
}

#[test]
fn changed_completes_on_a_new_epoch_only() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let mut waiting = Waiting::new(reader.changed());
    assert!(!waiting.ready());

    // A session that issues no call wakes the task, which waits on.
    drop(writer.begin());
    assert_eq!(waiting.wakes(), 1);
    assert!(!waiting.ready());
    // So does one the guard refused.
    let _ = run(writer.begin().erase(0, 1));
    assert!(!waiting.ready());

    write_word(&mut writer, 0);
    assert!(waiting.wakes() >= 3);
    assert!(waiting.ready());
}

#[test]
fn changed_wakes_every_waiter() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let chip: Reader<'_, Chip> = flash.reader(Span::new(0, FLASH_SIZE));
    let config: Reader<'_, CurrentConfig> = flash.reader(Span::from(layout::CONFIG_0));
    let mut app_waiters: Vec<_> = (0..4).map(|_| Waiting::new(reader.changed())).collect();
    let mut chip_waiter = Waiting::new(chip.changed());
    let mut config_waiter = Waiting::new(config.changed());
    for waiting in &mut app_waiters {
        assert!(!waiting.ready());
    }
    assert!(!chip_waiter.ready());
    assert!(!config_waiter.ready());

    write_word(&mut writer, 0);
    for waiting in &mut app_waiters {
        assert_eq!(waiting.wakes(), 1);
        assert!(waiting.ready());
    }
    assert_eq!(chip_waiter.wakes(), 1);
    assert!(chip_waiter.ready());
    // A waiter on another region is woken, and waits on.
    assert_eq!(config_waiter.wakes(), 1);
    assert!(!config_waiter.ready());
}

#[test]
fn as_many_waiters_as_slots_are_each_woken_once() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let mut waiters: Vec<_> = (0..crate::state::WAITERS)
        .map(|_| Waiting::new(reader.changed()))
        .collect();
    for waiting in &mut waiters {
        assert!(!waiting.ready());
        // Polling again registers nothing new.
        assert!(!waiting.ready());
    }
    assert!(waiters.iter().all(|waiting| waiting.wakes() == 0));
    write_word(&mut writer, 0);
    for waiting in &mut waiters {
        assert_eq!(waiting.wakes(), 1);
        assert!(waiting.ready());
    }
}

#[test]
fn changed_takes_the_state_when_called() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let changed = reader.changed();
    write_word(&mut writer, 0);
    // Polled for the first time after the write, it completes.
    assert!(Waiting::new(changed).ready());
    // Called after it, it waits for the next.
    let mut waiting = Waiting::new(reader.changed());
    assert!(!waiting.ready());
}

#[test]
fn changed_called_during_a_session_completes_when_it_ends() {
    let flash = flash();
    let (reader, mut writer) = alternate(&flash);
    let session = writer.begin();
    let mut waiting = Waiting::new(reader.changed());
    assert!(!waiting.ready());
    drop(session);
    assert!(waiting.ready());
}
