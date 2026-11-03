use embedded_storage_async::nor_flash::NorFlashErrorKind;

use super::*;
use crate::SECTOR_SIZE;
use crate::layout::{CONFIG_0, CONFIG_1, DIAG, FACTORY, OTADATA, RESERVED};

/// Every protected range, absolute, when running from `running`.
fn protected(running: Slot) -> [(u32, u32); 4] {
    let span = |entry: Entry| (entry.offset, entry.end());
    [
        (0, BOOT_END),
        span(FACTORY),
        span(RESERVED),
        span(running.partition()),
    ]
}

#[test]
fn the_boot_chain_ends_at_the_first_partition() {
    assert_eq!(BOOT_END, 0x9000);
    assert_eq!(BOOT_END, FACTORY.offset);
}

#[test]
fn the_guard_covers_exactly_the_protected_ranges() {
    for running in Slot::ALL {
        for (start, end) in protected(running) {
            // Every sector inside, and ranges crossing either edge.
            for sector in (start..end).step_by(SECTOR_SIZE as usize).take(64) {
                assert!(is_protected(sector, sector + 1, running), "{sector:#x}");
            }
            assert!(is_protected(end - 1, end, running));
            assert!(is_protected(start, start + 1, running));
            if start > 0 {
                assert!(is_protected(start - 1, start + 1, running));
            }
            assert!(is_protected(end - 1, end + 1, running));
            // Empty ranges touch nothing.
            assert!(!is_protected(start, start, running));
        }
        // What the writers write: the configurations, diag, the otadata and
        // the slot that didn't boot.
        let writable = [
            CONFIG_0,
            CONFIG_1,
            DIAG,
            OTADATA,
            running.other().partition(),
        ];
        for entry in writable {
            assert!(
                !is_protected(entry.offset, entry.end(), running),
                "{}",
                entry.label
            );
        }
        // A range from the other slot into the running one is refused whole.
        let (low, high) = match running {
            Slot::Ota0 => (running.partition(), running.other().partition()),
            Slot::Ota1 => (running.other().partition(), running.partition()),
        };
        assert!(is_protected(
            low.end() - SECTOR_SIZE,
            high.offset + SECTOR_SIZE,
            running
        ));
    }
}

#[test]
fn access_errors_map_to_nor_flash_kinds() {
    assert_eq!(
        AccessError::OutOfRange.kind(),
        NorFlashErrorKind::OutOfBounds
    );
    assert_eq!(
        AccessError::NotAligned.kind(),
        NorFlashErrorKind::NotAligned
    );
    assert_eq!(AccessError::Protected.kind(), NorFlashErrorKind::Other);
    assert_eq!(AccessError::Device.kind(), NorFlashErrorKind::Other);
    assert_eq!(
        AccessError::from(ReadError::OutOfRange),
        AccessError::OutOfRange
    );
    assert_eq!(AccessError::from(ReadError::Device), AccessError::Device);
    assert_eq!(AccessError::from(ReadError::Stale), AccessError::Device);
}
