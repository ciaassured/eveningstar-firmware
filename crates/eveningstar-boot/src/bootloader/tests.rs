#![allow(
    clippy::cast_possible_truncation,
    reason = "test offsets and lengths are small"
)]

use eveningstar_esp_formats::image::ImageError;
use eveningstar_esp_formats::pace::Yield;
use eveningstar_esp_formats::test_support::{
    BOOTLOADER, ESPFLASH_BOOTLOADER, bootloader_image, flash_with,
};
use eveningstar_storage::roles::AlternateApp;
use eveningstar_storage::test_support::{RamNor, TestFlash, run};
use eveningstar_storage::{PublicRead, ReadError, Slot, ViewError, layout};

use super::*;
use crate::digest;
use crate::test_support::{Counting, nix_board, sha256, split_board};

fn sha() -> sha2::Sha256 {
    sha2::Sha256::default()
}

fn load(module: &Bootloader<'_>) -> Result<BootloaderModel, LoadError> {
    run(module.load(sha(), &mut Yield, &mut || {}))?;
    Ok(module.model().unwrap())
}

/// The length of the bootloader image in `bytes`.
fn image_len(bytes: &[u8]) -> u32 {
    run(image::verify_bootloader(bytes)).unwrap().size()
}

/// The span a test places a bootloader region at: in `ota_1`, which a
/// writer can write.
fn at_ota_1(len: u32) -> Span {
    Span::new(layout::OTA_1.offset, len)
}

/// A module over `bytes` placed in `ota_1`, its region `len` bytes long,
/// and the flash.
fn module_over(bytes: &[u8], len: u32) -> (Bootloader<'static>, TestFlash) {
    let device = RamNor::with(layout::FLASH_SIZE, &[(layout::OTA_1.offset, bytes)]);
    let flash = TestFlash::new(device);
    (
        Bootloader::new(flash.reader::<roles::Bootloader>(at_ota_1(len))),
        flash,
    )
}

/// Every byte of `view`, read in one call.
fn read_all(view: &impl PublicRead) -> Vec<u8> {
    let mut bytes = vec![0; view.len() as usize];
    run(view.read(0, &mut bytes)).unwrap();
    bytes
}

#[test]
fn the_nix_bootloader_is_verified_known_and_published() {
    let (storage, _) = split_board(nix_board(), layout::OTA_0.offset);
    let module = Bootloader::new(storage.bootloader);
    assert_eq!(module.span(), Span::new(0, layout::TABLE_OFFSET));
    assert_eq!(module.model(), None);

    let model = load(&module).unwrap();
    let size = image_len(BOOTLOADER);
    let image = model.image().unwrap();
    assert_eq!(image.size(), size);
    let bytes = &BOOTLOADER[..size as usize];
    assert_eq!(image.file_sha256(), &sha256(bytes));
    assert_eq!(image.digest()[..], bytes[size as usize - 32..]);
    assert_eq!(
        image.known().map(|known| known.name),
        Some("ESP-IDF v5.5.2, app rollback")
    );
    assert!(module.descriptor(&model).unwrap().unwrap().is_some());
    assert_eq!(
        model.flash_size_code(),
        Some(model.header().unwrap().spi_size)
    );
    assert_eq!(model.chip_size(), Some(layout::FLASH_SIZE));
    // The chain found the same.
    let report = storage.report.bootloader;
    let id = report.image.unwrap();
    assert_eq!((id.size, &id.digest), (size, image.digest()));
    assert_eq!(report.known, image.known());
    assert_eq!(report.flash_size_code, model.flash_size_code());
    assert_eq!(report.header.as_ref().ok(), model.header().ok());

    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), size);
    assert_eq!(view.sanitized(), []);
    assert_eq!(read_all(&view), bytes);
}

#[test]
fn an_unknown_bootloader_that_verifies_is_published() {
    let size = image_len(ESPFLASH_BOOTLOADER);
    let (module, _) = module_over(ESPFLASH_BOOTLOADER, 0x8000);
    let model = load(&module).unwrap();
    let image = model.image().unwrap();
    assert_eq!(image.known(), None);
    assert_eq!(image.size(), size);
    let view = module.published(&model).unwrap();
    assert_eq!(read_all(&view), ESPFLASH_BOOTLOADER[..size as usize]);
}

#[test]
fn the_bytes_after_the_image_are_never_published() {
    let size = image_len(BOOTLOADER);
    let mut region = flash_with(0x8000, &[(0, &BOOTLOADER[..size as usize])]);
    region[size as usize..].fill(0xA5);
    let (module, _) = module_over(&region, 0x8000);
    let model = load(&module).unwrap();
    let view = module.published(&model).unwrap();
    assert_eq!(view.len(), size);
    assert_eq!(read_all(&view), BOOTLOADER[..size as usize]);
    let mut past = [0; 1];
    assert_eq!(run(view.read(size, &mut past)), Err(ViewError::OutOfRange));
    assert_eq!(past, [0]);
    let whole = run(digest(
        &view,
        Span::new(0, size),
        sha(),
        &mut Yield,
        &mut || {},
    ))
    .unwrap();
    assert_eq!(whole.sha256, sha256(&BOOTLOADER[..size as usize]));
}

#[test]
fn an_image_running_past_the_region_is_not_published() {
    let size = image_len(BOOTLOADER);
    for len in [size - 1, size - 32, size / 2, 24, 0] {
        let (module, _) = module_over(BOOTLOADER, len);
        let model = load(&module).unwrap();
        assert!(model.image().is_err(), "{len}");
        assert_eq!(
            module.published(&model).unwrap_err(),
            Unpublished::Unverified,
            "{len}"
        );
    }
    // Long enough, it verifies.
    let (module, _) = module_over(BOOTLOADER, size);
    assert!(module.published(&load(&module).unwrap()).is_ok());
}

#[test]
fn a_region_too_short_for_a_header_has_no_flash_size() {
    let (module, _) = module_over(BOOTLOADER, 23);
    let model = load(&module).unwrap();
    assert_eq!(model.flash_size_code(), None);
    assert_eq!(model.chip_size(), None);
    assert_eq!(
        model.header(),
        Err(ImageFailure::Read(ReadError::OutOfRange))
    );
}

#[test]
fn an_image_without_a_digest_is_not_published() {
    let mut builder = bootloader_image();
    builder.header[23] = 0;
    let image = builder.build();
    let (module, _) = module_over(&image, 0x8000);
    let model = load(&module).unwrap();
    assert_eq!(model.image(), Err(ImageFailure::NoDigest));
    // Its header reads.
    assert!(model.header().is_ok());
    assert_eq!(model.flash_size_code(), Some(3));
    assert_eq!(
        module.published(&model).unwrap_err(),
        Unpublished::Unverified
    );
}

#[test]
fn an_image_whose_digest_doesnt_match_is_not_published() {
    let mut image = bootloader_image().build();
    let last = image.len() - 1;
    image[last] ^= 1;
    let (module, _) = module_over(&image, 0x8000);
    let model = load(&module).unwrap();
    assert_eq!(model.image(), Err(ImageFailure::DigestMismatch));
    assert_eq!(
        module.published(&model).unwrap_err(),
        Unpublished::Unverified
    );
}

#[test]
fn failures_keep_no_value_of_the_bytes_read() {
    let mut image = bootloader_image().build();
    image[0] = 0x12;
    let (module, _) = module_over(&image, 0x8000);
    let model = load(&module).unwrap();
    assert_eq!(model.header(), Err(ImageFailure::Magic));
    assert_eq!(model.image(), Err(ImageFailure::Magic));
    assert_eq!(
        ImageFailure::from(ImageError::Checksum {
            stored: 1,
            computed: 2
        }),
        ImageFailure::Checksum
    );
    assert_eq!(
        ImageFailure::from(ImageError::SegmentLength { index: 3, len: 7 }),
        ImageFailure::SegmentLength { index: 3 }
    );
}

#[test]
fn a_model_is_kept_and_parsed_once() {
    let (module, flash) = module_over(BOOTLOADER, 0x8000);
    let mut pace = Counting::default();
    let mut advanced = 0;
    run(module.load(sha(), &mut pace, &mut || advanced += 1)).unwrap();
    let model = module.model().unwrap();
    let size = model.image().unwrap().size();
    assert_eq!(pace.pauses, (size - 32) as usize / 1024);
    assert_eq!(advanced, pace.pauses);
    let calls = flash.device().calls().len();
    assert_eq!(module.model(), Some(model));
    assert_eq!(load(&module), Ok(model));
    assert_eq!(flash.device().calls().len(), calls);
}

#[test]
fn a_model_and_its_view_go_stale_once_the_region_is_written() {
    let (module, flash) = module_over(BOOTLOADER, 0x8000);
    let mut writer = flash.writer::<AlternateApp>(Span::from(layout::OTA_1), Slot::Ota0);
    let model = load(&module).unwrap();
    let view = module.published(&model).unwrap();
    run(writer.begin().program(0x9000, &[0; 4])).unwrap();

    assert_eq!(module.model(), None);
    let mut byte = [0; 1];
    assert_eq!(run(view.read(0, &mut byte)), Err(ViewError::Stale));
    assert_eq!(module.published(&model).unwrap_err(), Unpublished::Stale);
    // A new parse has a new epoch, and publishes again.
    let again = load(&module).unwrap();
    assert_ne!(again.epoch(), model.epoch());
    assert!(module.published(&again).is_ok());

    // While a session is open, nothing parses.
    let session = writer.begin();
    assert_eq!(load(&module), Err(LoadError::Busy));
    drop(session);
}

#[test]
fn a_session_during_the_parse_makes_it_stale() {
    let (module, flash) = module_over(BOOTLOADER, 0x8000);
    let mut writer = flash.writer::<AlternateApp>(Span::from(layout::OTA_1), Slot::Ota0);
    flash.device().before_call(10, move || {
        run(writer.begin().program(0x9000, &[0; 4])).unwrap();
    });
    assert_eq!(load(&module), Err(LoadError::Stale));
    assert_eq!(module.model(), None);
    assert!(load(&module).is_ok());
}

#[test]
fn the_module_can_be_shared_from_a_static() {
    fn shared<T: Sync + Send>() {}
    shared::<Bootloader<'static>>();
}
