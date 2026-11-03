use super::*;
use crate::test_support::{APP, BOOTLOADER, OTADATA_FIRST_BOOT, TABLE};

#[test]
fn a_cut_fails_every_write_from_its_index_and_tears_the_first_program() {
    let flash = RamNor::erased(0x4000);
    flash.cut_after(2, Tear::Prefix(2));
    flash.program(0, &[0; 4]).unwrap();
    flash.erase_sector(0x1000).unwrap();
    // Reads aren't counted.
    flash.read(0, &mut [0; 4]).unwrap();
    assert_eq!(flash.program(0x10, &[0; 8]), Err(DeviceError));
    assert_eq!(flash.program(0x20, &[0; 4]), Err(DeviceError));
    assert_eq!(flash.erase_sector(0), Err(DeviceError));
    assert_eq!(flash.at(0, 4), [0; 4]);
    assert_eq!(
        flash.at(0x10, 8),
        [0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    );
    assert_eq!(flash.at(0x20, 4), [0xFF; 4]);
    // The writes that failed are logged.
    assert_eq!(
        flash.writes(),
        [
            Call::Program(0, 4),
            Call::Erase(0x1000),
            Call::Program(0x10, 8),
            Call::Program(0x20, 4),
            Call::Erase(0),
        ]
    );
    assert_eq!(flash.calls().len(), 6);
    flash.heal();
    flash.erase_sector(0).unwrap();
    assert_eq!(flash.at(0, 4), [0xFF; 4]);
}

#[test]
fn a_cut_erase_lands_not_at_all_and_takes_the_tear() {
    let flash = RamNor::with(0x2000, &[(0x1000, &[0; 4])]);
    flash.cut_after(0, Tear::Prefix(4));
    assert_eq!(flash.erase_sector(0x1000), Err(DeviceError));
    assert_eq!(flash.program(0, &[0; 4]), Err(DeviceError));
    assert_eq!(flash.at(0x1000, 4), [0; 4]);
    assert_eq!(flash.at(0, 4), [0xFF; 4]);
}

#[test]
fn writes_fail_after_a_count_changing_nothing() {
    let flash = RamNor::erased(0x2000);
    flash.fail_after(1);
    flash.program(0, &[0xF0; 4]).unwrap();
    assert_eq!(flash.program(0, &[0; 4]), Err(DeviceError));
    assert_eq!(flash.at(0, 4), [0xF0; 4]);
}

#[test]
fn bytes_across_sectors_are_written_and_read_whole() {
    let flash = RamNor::erased(0x3000);
    let bytes: Vec<u8> = (0..0x1800_u32).map(|n| n.to_le_bytes()[0]).collect();
    flash.poke(0x800, &bytes);
    assert_eq!(flash.at(0x800, bytes.len()), bytes);
    flash.program(0xFFC, &[0; 8]).unwrap();
    assert_eq!(flash.at(0xFFC, 8), [0; 8]);
    assert_eq!(flash.at(0x2000, 0x1000), vec![0xFF; 0x1000]);
}

#[test]
fn a_copy_holds_the_bytes_and_nothing_else() {
    let flash = RamNor::with(0x3000, &[(0xFFE, &[1, 2, 3, 4])]);
    flash.erase_sector(0x2000).unwrap();
    flash.fail_writes(true);
    let copy = flash.copy();
    assert_eq!(copy.at(0xFFC, 8), [0xFF, 0xFF, 1, 2, 3, 4, 0xFF, 0xFF]);
    assert_eq!(copy.calls(), []);
    copy.program(0x2000, &[0; 4]).unwrap();
    assert_eq!(flash.at(0x2000, 4), [0xFF; 4]);
}

#[test]
fn the_simulated_bootloader_starts_the_slot_that_loads() {
    let flash = RamNor::with(
        layout::FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (layout::TABLE_OFFSET, TABLE),
            (layout::OTA_1.offset, APP),
        ],
    );
    assert_eq!(boot_device(&flash), Some(Slot::Ota1));
    assert_eq!(boot_device(&RamNor::erased(layout::FLASH_SIZE)), None);
    // On blank otadata, the bootloader writes an entry for the slot it
    // starts.
    let first = RamNor::with(layout::FLASH_SIZE, &[(layout::OTA_0.offset, APP)]);
    assert_eq!(boot_device(&first), Some(Slot::Ota0));
    assert_eq!(
        first.at(layout::OTADATA.offset, OTADATA_LEN),
        OTADATA_FIRST_BOOT
    );
    assert_eq!(first.writes(), []);
}
