#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use core::pin::pin;
use core::task::{Context, Poll, Waker};

use super::*;
use crate::layout::{FACTORY, FLASH_SIZE, OTA_0, OTA_1, RESERVED, TABLE_OFFSET};
use crate::roles::{
    AlternateApp, AlternateConfig, Chip, CurrentConfig, Diag, FoundApp, Internal, Otadata,
    Programmable,
};
use crate::test_support::{Call, RamNor, TestFlash, block_on, run};
use crate::{AccessError, ReadError, Slot, SnapshotError, Source, Span, WriteState, layout};

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
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
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
