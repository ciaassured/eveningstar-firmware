use eveningstar_storage::test_support::{RamNor, TestFlash, run};
use eveningstar_storage::{PublicRead, Slot, ViewError};

use core::sync::atomic::{AtomicUsize, Ordering};

use eveningstar_storage::chain::Handles;
use eveningstar_storage::nor::{Flash, NorDevice};
use eveningstar_storage::{DeviceError, split};

use super::*;
use crate::test_support::{
    COMMENT, KEYNUM, KEYS, manifest_line as line, nix_board, sha256, signed_file as file_with,
    trust,
};

const OTHER_KEYNUM: Keynum = [9; 8];
const IMAGE: &[u8] = b"an image";

/// The release's file: the image and its ELF.
fn good() -> Vec<u8> {
    file_with(
        KEYNUM,
        &format!("{}{}", line("a.bin", IMAGE), line("a.elf", b"an elf")),
    )
}

/// The alternate signature region, `sig_1` while `ota_0` runs, holding
/// `bytes` from its start, and the flash.
fn region_with(bytes: &[u8]) -> (SignatureRegion<'static, AlternateSignature>, TestFlash) {
    let device = RamNor::with(layout::FLASH_SIZE, &[(layout::SIG_1.offset, bytes)]);
    let flash = TestFlash::new(device);
    let span = Span::new(layout::SIG_1.offset, layout::SIG_1.size);
    (SignatureRegion::new(flash.reader(span)), flash)
}

fn load<R: SignatureRole>(region: &SignatureRegion<'_, R>) -> SignatureModel<R> {
    let mut buf = [0; FILE_MAX];
    region.load(&mut buf).unwrap();
    region.model().unwrap()
}

fn content_of(bytes: &[u8]) -> Content {
    let (region, _flash) = region_with(bytes);
    *load(&region).content()
}

fn check<R: SignatureRole>(region: &SignatureRegion<'_, R>, image: &[u8]) -> Signature {
    let model = load(region);
    let mut buf = [0; FILE_MAX];
    let judged = region
        .check(&model, trust(), &sha256(image), &mut buf)
        .unwrap();
    assert_eq!(judged.epoch(), model.epoch(), "judged at the model's epoch");
    *judged.signature()
}

fn read_all(view: &impl PublicRead) -> Vec<u8> {
    let mut bytes = vec![0; view.len() as usize];
    run(view.read(0, &mut bytes)).unwrap();
    bytes
}

#[test]
fn a_release_file_parses_verifies_and_names_the_image() {
    let file = good();
    let (region, _flash) = region_with(&file);
    assert_eq!(region.model(), None);
    let model = load(&region);
    let Content::File(parsed) = *model.content() else {
        panic!("{model:?}");
    };
    assert_eq!(parsed.len() as usize, file.len());
    assert_eq!(parsed.keynum(), &KEYNUM);
    let comment = parsed.comment();
    assert_eq!(
        &file[comment.offset as usize..comment.end().unwrap() as usize],
        COMMENT.as_bytes()
    );

    assert_eq!(
        check(&region, IMAGE),
        Signature::Verified {
            key: &KEYS[0],
            line: 0
        }
    );
    assert_eq!(
        check(&region, b"an elf"),
        Signature::Verified {
            key: &KEYS[0],
            line: 1
        }
    );
    assert_eq!(
        check(&region, b"another image"),
        Signature::Stale { key: &KEYS[0] }
    );
}

#[test]
fn an_erased_region_is_unsigned() {
    assert_eq!(content_of(&[]), Content::Unsigned);
    let (region, _flash) = region_with(&[]);
    assert_eq!(check(&region, IMAGE), Signature::Unsigned);
    let model = load(&region);
    assert_eq!(
        region.published(&model).err(),
        Some(Unpublished::Unverified)
    );
}

#[test]
fn every_byte_after_the_file_must_be_erased() {
    let file = good();
    let mut after = file.clone();
    after.extend_from_slice(&[0xFF; 10]);
    after.push(0x00);
    assert_eq!(content_of(&after), Content::Invalid(Invalid::Padding));

    // The last byte of the region.
    let mut last = vec![0xFF; FILE_MAX];
    last[..file.len()].copy_from_slice(&file);
    last[FILE_MAX - 1] = 0xFE;
    assert_eq!(content_of(&last), Content::Invalid(Invalid::Padding));
}

#[test]
fn a_line_after_the_manifest_fails_the_file() {
    let mut file = good();
    file.extend_from_slice(b"not a line of the manifest\n");
    assert!(matches!(
        content_of(&file),
        Content::Invalid(Invalid::Format(SigError::Manifest { line: 3 }))
    ));
    // Without its newline, the last line runs to the end of the region.
    let mut file = good();
    file.pop();
    assert!(matches!(
        content_of(&file),
        Content::Invalid(Invalid::Padding | Invalid::Format(_))
    ));
}

#[test]
fn a_region_that_starts_erased_and_holds_bytes_later_is_invalid() {
    // A wipe a power cut interrupted: the first sector's start erased, a
    // file's bytes later.
    let mut region = vec![0xFF; 16];
    region.extend_from_slice(&good());
    assert!(matches!(
        content_of(&region),
        Content::Invalid(Invalid::Format(SigError::Comment) | Invalid::Padding)
    ));
    let mut region = vec![0xFF; FILE_MAX];
    region[FILE_MAX - 1] = 0;
    assert!(!matches!(
        content_of(&region),
        Content::Unsigned | Content::File(_)
    ));
}

#[test]
fn the_longest_comment_and_a_twice_named_image() {
    // The comment isn't signed, so a longer one still verifies.
    let mut file = good();
    let at = b"untrusted comment: ".len();
    file.splice(at..at, core::iter::repeat_n(b'x', 1023 - COMMENT.len()));
    let Content::File(parsed) = content_of(&file) else {
        panic!("the file doesn't parse");
    };
    assert_eq!(parsed.comment().len, 1023);
    let (region, _flash) = region_with(&file);
    assert!(matches!(check(&region, IMAGE), Signature::Verified { .. }));

    let twice = file_with(
        KEYNUM,
        &format!("{}{}", line("a.bin", IMAGE), line("b.bin", IMAGE)),
    );
    let (region, _flash) = region_with(&twice);
    assert_eq!(
        check(&region, IMAGE),
        Signature::Invalid(Invalid::Ambiguous)
    );
}

#[test]
fn the_key_and_the_signature_are_judged_on_request() {
    let message = line("a.bin", IMAGE);
    let (region, _flash) = region_with(&file_with(OTHER_KEYNUM, &message));
    assert!(matches!(load(&region).content(), Content::File(_)));
    assert_eq!(
        check(&region, IMAGE),
        Signature::UnknownKey {
            keynum: OTHER_KEYNUM
        }
    );

    let mut forged = file_with(KEYNUM, &message);
    let last = forged.len() - 2;
    forged[last] = if forged[last] == b'0' { b'1' } else { b'0' };
    let (region, _flash) = region_with(&forged);
    assert!(matches!(load(&region).content(), Content::File(_)));
    assert_eq!(
        check(&region, IMAGE),
        Signature::Invalid(Invalid::Signature)
    );
}

#[test]
fn every_bit_flip_outside_the_comment_fails_the_check() {
    let file = good();
    let comment = b"untrusted comment: ".len()..b"untrusted comment: ".len() + COMMENT.len();
    for bit in 0..file.len() * 8 {
        if comment.contains(&(bit / 8)) {
            continue;
        }
        let mut flipped = file.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        let (region, _flash) = region_with(&flipped);
        let signature = check(&region, IMAGE);
        assert!(
            !matches!(signature, Signature::Verified { .. }),
            "bit {bit}: {signature:?}"
        );
    }
}

#[test]
fn the_view_is_the_file_with_its_comment_sanitized() {
    let file = good();
    let (region, _flash) = region_with(&file);
    let model = load(&region);
    let view = region.published(&model).unwrap();
    assert_eq!(view.len() as usize, file.len());
    let mut expected = file.clone();
    expected[19..19 + COMMENT.len()].fill(0xFF);
    assert_eq!(read_all(&view), expected);

    // An invalid region publishes nothing.
    let mut bad = file;
    bad.push(b'x');
    let (region, _flash) = region_with(&bad);
    let model = load(&region);
    assert_eq!(
        region.published(&model).err(),
        Some(Unpublished::Unverified)
    );
}

#[test]
fn a_write_makes_the_model_stale() {
    let (region, flash) = region_with(&good());
    let model = load(&region);
    let span = Span::new(layout::SIG_1.offset, layout::SIG_1.size);
    let mut writer = flash.writer::<AlternateSignature>(span, Slot::Ota0);
    let mut buf = [0; FILE_MAX];

    {
        let _session = writer.begin();
        assert_eq!(region.load(&mut buf), Err(LoadError::Busy));
        assert_eq!(
            region.check(&model, trust(), &sha256(IMAGE), &mut buf),
            Err(LoadError::Busy)
        );
    }
    run(writer.begin().wipe()).unwrap();
    assert_eq!(region.model(), None);
    assert_eq!(
        region.check(&model, trust(), &sha256(IMAGE), &mut buf),
        Err(LoadError::Stale)
    );
    assert_eq!(region.published(&model).err(), Some(Unpublished::Stale));
    let wiped = load(&region);
    assert_ne!(wiped.epoch(), model.epoch());
    assert_eq!(*wiped.content(), Content::Unsigned);

    // A view taken before a write reads nothing after it.
    let (region, flash) = region_with(&good());
    let model = load(&region);
    let view = region.published(&model).unwrap();
    let mut writer = flash.writer::<AlternateSignature>(span, Slot::Ota0);
    run(writer.begin().wipe()).unwrap();
    let mut bytes = vec![0; view.len() as usize];
    assert_eq!(run(view.read(0, &mut bytes)), Err(ViewError::Stale));
}

/// A device whose next `failing` reads fail, then read `inner` again.
struct FlakyReads {
    inner: &'static RamNor,
    failing: AtomicUsize,
}

impl NorDevice for FlakyReads {
    fn capacity(&self) -> u32 {
        self.inner.capacity()
    }
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError> {
        let failing = self.failing.load(Ordering::SeqCst);
        if failing > 0 {
            self.failing.store(failing - 1, Ordering::SeqCst);
            return Err(DeviceError);
        }
        self.inner.read(address, buf)
    }
    fn erase_sector(&self, address: u32) -> Result<(), DeviceError> {
        self.inner.erase_sector(address)
    }
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError> {
        self.inner.program(address, bytes)
    }
}

/// The current signature region of the Nix board with `file` in `sig_0`,
/// split over a device whose reads can be made to fail, and the device.
fn flaky_region(
    file: &[u8],
) -> (
    SignatureRegion<'static, CurrentSignature>,
    &'static FlakyReads,
) {
    let board = nix_board();
    board.poke(layout::SIG_0.offset, file);
    let device: &'static FlakyReads = Box::leak(Box::new(FlakyReads {
        inner: Box::leak(Box::new(board)),
        failing: AtomicUsize::new(0),
    }));
    let lent: &'static mut dyn NorDevice = Box::leak(Box::new(Lend(device)));
    let flash = Box::leak(Box::new(Flash::new(lent)));
    let storage = run(split(flash, layout::OTA_0.offset));
    let Handles::Boot(handles, _) = storage.handles else {
        panic!("{:?}", storage.report.tier);
    };
    (SignatureRegion::new(handles.current_signature), device)
}

/// A shared device as one `split` can take mutably.
struct Lend(&'static FlakyReads);

impl NorDevice for Lend {
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
        self.0.program(address, bytes)
    }
}

#[test]
fn a_read_the_device_fails_is_tried_again() {
    let (region, device) = flaky_region(&good());
    let mut buf = [0; FILE_MAX];

    // Fewer failures than attempts: the region still parses and verifies.
    device.failing.store(READ_ATTEMPTS - 1, Ordering::SeqCst);
    region.load(&mut buf).unwrap();
    let model = region.model().unwrap();
    assert!(matches!(model.content(), Content::File(_)));
    device.failing.store(READ_ATTEMPTS - 1, Ordering::SeqCst);
    let judged = region
        .check(&model, trust(), &sha256(IMAGE), &mut buf)
        .unwrap();
    assert!(matches!(judged.signature(), Signature::Verified { .. }));

    // As many failures as attempts: the region can't be read.
    let (region, device) = flaky_region(&good());
    device.failing.store(READ_ATTEMPTS, Ordering::SeqCst);
    region.load(&mut buf).unwrap();
    assert_eq!(
        *region.model().unwrap().content(),
        Content::Invalid(Invalid::Read)
    );
}
