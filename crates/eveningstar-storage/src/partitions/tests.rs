use embedded_storage_async::nor_flash::{NorFlashError as _, NorFlashErrorKind};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::layout::{CONFIG_0, CONFIG_1, DIAG, OTA_0, OTA_1, OTADATA};
use crate::test_support::{APP, BOOTLOADER, Op, RamFlash, block_on, nix_board, run};
use crate::{Data, Entry, Slot, Storage, split};

const SECTOR: u32 = SECTOR_SIZE;

fn storage(flash: &RamFlash) -> Storage<'_> {
    run(split(flash, OTA_0.offset))
}

fn data(flash: &RamFlash) -> Data<'_> {
    storage(flash).data.expect("the data tier")
}

/// A board running from `ota_1`, with an otadata entry selecting it.
fn ota_1_board() -> RamFlash {
    let flash = nix_board();
    let entry = crate::otadata::SelectEntry::new(2, crate::otadata::State::Valid).to_bytes();
    let mut otadata = vec![0xFF; OTADATA.size as usize];
    otadata[..32].copy_from_slice(&entry);
    let board = crate::test_support::flash_with(
        crate::test_support::FLASH_LEN,
        &[
            (0, &flash.bytes()),
            (OTADATA.offset, &otadata),
            (OTA_1.offset, APP),
        ],
    );
    RamFlash::new(board)
}

fn span(p: &impl SensitiveRead) -> (u32, u32) {
    (p.offset(), p.len())
}

fn entry_span(entry: Entry) -> (u32, u32) {
    (entry.offset, entry.size)
}

#[test]
fn fixed_size_handles_are_their_layout_partitions_in_either_slot() {
    for (flash, running) in [(nix_board(), Slot::Ota0), (ota_1_board(), Slot::Ota1)] {
        let booted = running.partition().offset;
        let Storage { data, .. } = run(split(&flash, booted));
        let data = data.expect("the data tier");
        assert_eq!(span(&data.config), entry_span(running.config()));
        assert_eq!(
            span(&data.other_config),
            entry_span(running.other().config())
        );
        assert_eq!(span(&data.diag), entry_span(DIAG));
        assert_eq!(
            span(&data.other_app),
            entry_span(running.other().partition())
        );
        assert_eq!(span(&data.otadata), entry_span(OTADATA));

        assert_eq!(data.config.len(), Config::SIZE);
        assert_eq!(data.other_config.len(), OtherConfig::SIZE);
        assert_eq!(data.diag.len(), Diag::SIZE);
        assert_eq!(data.other_app.len(), OtherApp::SIZE);
        assert_eq!(data.otadata.len(), Otadata::SIZE);
    }
    assert_eq!(Config::SIZE, CONFIG_0.size);
    assert_eq!(Config::SIZE, CONFIG_1.size);
    assert_eq!(OtherApp::SIZE, OTA_1.size);
}

#[test]
fn roles_have_names() {
    let names = [
        Bootloader::NAME,
        PartitionTable::NAME,
        RunningApp::NAME,
        OtherApp::NAME,
        Otadata::NAME,
        Config::NAME,
        OtherConfig::NAME,
        Diag::NAME,
    ];
    for (i, a) in names.iter().enumerate() {
        assert_ne!(*a, "");
        for b in &names[i + 1..] {
            assert_ne!(a, b);
        }
    }
}

#[test]
fn public_handles_read_as_sources_and_copies() {
    let flash = nix_board();
    let storage = storage(&flash);
    let bootloader = storage.boot.bootloader;
    let copy = bootloader;
    assert_eq!(Source::size(&copy), 0x8000);
    let mut head = [0; 24];
    Source::read(&bootloader, 0, &mut head).unwrap();
    assert_eq!(head[..], BOOTLOADER[..24]);
    let verified = run(crate::image::verify_bootloader(&copy)).unwrap();
    assert_eq!(
        run(copy.sha256(0, verified.size() - 32)),
        Ok(*verified.digest())
    );
    assert_eq!(
        Source::read(&copy, 0x8000 - 3, &mut [0; 4]),
        Err(ReadError::OutOfRange)
    );

    let running = storage.boot.running_app.unwrap();
    let digest: [u8; 32] = Sha256::digest(APP).into();
    assert_eq!(
        run(running.sha256(0, u32::try_from(APP.len()).unwrap())),
        Ok(digest)
    );
    assert_eq!(
        run(running.sha256(OTA_0.size - 1, 2)),
        Err(ReadError::OutOfRange)
    );
}

#[test]
fn every_handle_reads_inside_its_partition_only() {
    let flash = nix_board();
    let data = data(&flash);
    let mut buf = [0; 8];
    run(SensitiveRead::read(&data.otadata, 0, &mut buf)).unwrap();
    assert_eq!(buf, crate::test_support::OTADATA_FIRST_BOOT[..8]);
    run(data.config.read(Config::SIZE - 8, &mut buf)).unwrap();
    assert_eq!(buf, [0xFF; 8]);
    for offset in [Config::SIZE - 7, Config::SIZE, u32::MAX - 7, u32::MAX] {
        assert_eq!(
            run(data.config.read(offset, &mut buf)),
            Err(ReadError::OutOfRange),
            "{offset:#x}"
        );
    }
    run(data.diag.read(Diag::SIZE, &mut [])).unwrap();
    assert_eq!(
        run(data.diag.read(Diag::SIZE + 1, &mut [])),
        Err(ReadError::OutOfRange)
    );
    assert!(!data.diag.is_empty());
}

#[test]
fn config_and_diag_are_erased_and_programmed_with_nor_semantics() {
    let flash = nix_board();
    let Data {
        mut config,
        mut diag,
        ..
    } = data(&flash);
    for handle in [&mut config as &mut dyn Probe, &mut diag] {
        handle.check(&flash);
    }
}

/// The erase and program checks shared by the writable handles.
trait Probe {
    fn check(&mut self, flash: &RamFlash);
}

impl<P: Writable> Probe for P {
    fn check(&mut self, flash: &RamFlash) {
        let base = self.offset();
        let len = self.len();
        run(self.program(len - 4, &[0x0F, 0xF0, 0x00, 0xFF])).unwrap();
        run(self.program(len - 4, &[0x3C, 0x3C, 0xFF, 0x7E])).unwrap();
        assert_eq!(flash.at(base + len - 4, 4), [0x0C, 0x30, 0x00, 0x7E]);
        assert_eq!(
            run(self.program(len - 2, &[0; 4])),
            Err(AccessError::NotAligned)
        );
        assert_eq!(
            run(self.program(len, &[0; 4])),
            Err(AccessError::OutOfRange)
        );
        assert_eq!(
            run(self.erase(0, len + SECTOR)),
            Err(AccessError::OutOfRange)
        );
        assert_eq!(run(self.erase(SECTOR, 0)), Err(AccessError::OutOfRange));
        assert_eq!(run(self.erase(2, SECTOR)), Err(AccessError::NotAligned));
        run(self.erase(len - SECTOR, len)).unwrap();
        assert_eq!(flash.at(base + len - 4, 4), [0xFF; 4]);
        let done = block_on(self.erase(0, 3 * SECTOR));
        assert_eq!((done.output, done.wakes), (Ok(()), 3));
        let done = block_on(self.program(0, &[0; 4100]));
        assert_eq!((done.output, done.wakes), (Ok(()), 2));
    }
}

#[test]
fn the_other_config_is_erased() {
    let flash = nix_board();
    let mut other = data(&flash).other_config;
    run(other.erase(0, OtherConfig::SIZE)).unwrap();
    let sectors = OtherConfig::SIZE / SECTOR;
    assert_eq!(flash.ops().len(), sectors as usize);
    assert_eq!(flash.ops()[0], Op::Erase(CONFIG_1.offset));
    assert_eq!(
        run(other.erase(0, OtherConfig::SIZE + SECTOR)),
        Err(AccessError::OutOfRange)
    );
}

#[test]
fn the_nor_adapter_behaves_as_embedded_storage_expects() {
    let flash = nix_board();
    let Data { config, diag, .. } = data(&flash);
    let mut config = config.into_nor_flash();
    let mut diag = diag.into_nor_flash();
    assert_eq!(<NorPartition<'_, Config> as ReadNorFlash>::READ_SIZE, 1);
    assert_eq!(<NorPartition<'_, Config> as NorFlash>::WRITE_SIZE, 4);
    assert_eq!(<NorPartition<'_, Diag> as NorFlash>::ERASE_SIZE, 4096);
    assert_eq!(config.capacity(), CONFIG_0.size as usize);
    assert_eq!(diag.capacity(), DIAG.size as usize);
    let capacity = u32::try_from(config.capacity()).unwrap();

    // Reads at any alignment, inside the capacity.
    let mut buf = [0; 3];
    run(ReadNorFlash::read(&mut config, 1, &mut buf)).unwrap();
    run(ReadNorFlash::read(&mut config, capacity - 3, &mut buf)).unwrap();
    let error = run(ReadNorFlash::read(&mut config, capacity - 2, &mut buf)).unwrap_err();
    assert_eq!(error.kind(), NorFlashErrorKind::OutOfBounds);

    // Writes of whole words, inside the capacity, any number of times.
    run(config.write(8, &[0xF0; 8])).unwrap();
    run(config.write(8, &[0x3C; 4])).unwrap();
    let mut buf = [0; 8];
    run(ReadNorFlash::read(&mut config, 8, &mut buf)).unwrap();
    assert_eq!(buf, [0x30, 0x30, 0x30, 0x30, 0xF0, 0xF0, 0xF0, 0xF0]);
    for (offset, len) in [(1, 4), (0, 2), (6, 4)] {
        let error = run(config.write(offset, &vec![0; len])).unwrap_err();
        assert_eq!(
            error.kind(),
            NorFlashErrorKind::NotAligned,
            "{len} at {offset}"
        );
    }
    for offset in [capacity, capacity - 4 + 4, u32::MAX - 3] {
        let error = run(config.write(offset, &[0; 4])).unwrap_err();
        assert_eq!(error.kind(), NorFlashErrorKind::OutOfBounds, "{offset:#x}");
    }
    let error = run(config.write(capacity - 4, &[0; 8])).unwrap_err();
    assert_eq!(error.kind(), NorFlashErrorKind::OutOfBounds);
    run(config.write(capacity - 4, &[0; 4])).unwrap();

    // Erases of whole sectors, `to` exclusive; `from > to` is out of bounds.
    run(config.erase(0, 4096)).unwrap();
    run(ReadNorFlash::read(&mut config, 8, &mut buf)).unwrap();
    assert_eq!(buf, [0xFF; 8]);
    for (from, to, kind) in [
        (4096, 0, NorFlashErrorKind::OutOfBounds),
        (0, capacity + 4096, NorFlashErrorKind::OutOfBounds),
        (1, 4096, NorFlashErrorKind::NotAligned),
        (0, 4095, NorFlashErrorKind::NotAligned),
    ] {
        let error = run(config.erase(from, to)).unwrap_err();
        assert_eq!(error.kind(), kind, "{from}..{to}");
    }
    run(config.erase(capacity - 4096, capacity)).unwrap();
    run(config.erase(capacity, capacity)).unwrap();

    // The adapters stay inside their partitions.
    run(diag.write(0, &[0; 4])).unwrap();
    assert_eq!(flash.at(DIAG.offset, 4), [0; 4]);
    assert_eq!(flash.at(CONFIG_0.end(), 4), [0xFF; 4]);
}

#[test]
fn the_debug_output_names_the_role_and_span_only() {
    let flash = nix_board();
    let data = data(&flash);
    assert_eq!(
        format!("{:?}", data.config),
        format!(
            "Partition {{ role: \"config\", offset: {}, len: {} }}",
            CONFIG_0.offset, CONFIG_0.size
        )
    );
    assert_eq!(
        format!("{:?}", data.diag.into_nor_flash()),
        format!(
            "NorPartition {{ role: \"diag\", offset: {}, len: {} }}",
            DIAG.offset, DIAG.size
        )
    );
}
