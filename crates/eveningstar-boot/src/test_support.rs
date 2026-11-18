//! Fixtures shared by the crate's tests: boards in RAM split by the storage
//! crate, a pace that counts, and a loop that polls two futures.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::Signer as _;
use eveningstar_esp_formats::pace::Pace;
use eveningstar_esp_formats::test_support::{APP, BOOTLOADER, OTADATA_FIRST_BOOT, TABLE};
use eveningstar_signify::{Generation, Purpose, TrustSet, TrustedKey};
use eveningstar_storage::nor::{Flash, NorDevice};
use eveningstar_storage::test_support::{RamNor, run};
use eveningstar_storage::{DeviceError, Storage, layout, split};
use sha2::{Digest as _, Sha256};

/// A pace that counts its pauses and never hands control back.
#[derive(Debug, Default)]
pub(crate) struct Counting {
    pub(crate) pauses: usize,
}

impl Pace for Counting {
    fn pause(&mut self) -> impl Future<Output = ()> {
        self.pauses += 1;
        core::future::ready(())
    }
}

/// The SHA-256 of `bytes`, from `sha2` directly.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A shared [`RamNor`] as a device `split` can take mutably, while the test
/// keeps reading and hooking it.
struct Lent(&'static RamNor);

impl NorDevice for Lent {
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

/// `device`, split for the app booted at `booted`, with the device kept for
/// the test. Both are leaked, so the handles are `'static`.
pub(crate) fn split_board(device: RamNor, booted: u32) -> (Storage<'static>, &'static RamNor) {
    let device: &'static RamNor = Box::leak(Box::new(device));
    let lent: &'static mut dyn NorDevice = Box::leak(Box::new(Lent(device)));
    let flash = Box::leak(Box::new(Flash::new(lent)));
    (run(split(flash, booted)), device)
}

/// A board flashed by `flash-all`: the Nix bootloader and table, the otadata
/// the bootloader writes on its first boot, and the app in `ota_0`.
pub(crate) fn nix_board() -> RamNor {
    RamNor::with(
        layout::FLASH_SIZE,
        &[
            (0, BOOTLOADER),
            (layout::TABLE_OFFSET, TABLE),
            (layout::OTADATA.offset, OTADATA_FIRST_BOOT),
            (layout::OTA_0.offset, APP),
        ],
    )
}

/// The most polls [`drive`] makes before it gives up.
const MAX_POLLS: usize = 10_000_000;

/// Polls `background` and `foreground` in turn, with a waker that does
/// nothing, until `foreground` completes, and returns its output. Panics
/// when `background` completes first or after [`MAX_POLLS`] polls, which
/// stand for a deadlock.
pub(crate) fn drive<B: Future, F: Future>(background: B, foreground: F) -> F::Output {
    let mut cx = Context::from_waker(Waker::noop());
    let mut background = pin!(background);
    let mut foreground = pin!(foreground);
    for _ in 0..MAX_POLLS {
        if let Poll::Ready(output) = foreground.as_mut().poll(&mut cx) {
            return output;
        }
        assert!(
            background.as_mut().poll(&mut cx).is_pending(),
            "the background future completed"
        );
    }
    panic!("no progress after {MAX_POLLS} polls");
}

/// Polls `future` once with a waker that does nothing.
pub(crate) fn poll_once<F: Future + ?Sized>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// The keynum of the test signing key.
pub(crate) const KEYNUM: eveningstar_signify::Keynum = [1, 2, 3, 4, 5, 6, 7, 8];

/// The comment of the signature files [`signed_file`] writes.
pub(crate) const COMMENT: &str = "verify with eveningstar-1-fw.pub";

/// The test signing key.
pub(crate) fn signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7; 32])
}

/// The keys of [`trust`]: the test signing key, as release generation 1.
pub(crate) static KEYS: std::sync::LazyLock<&'static [TrustedKey]> =
    std::sync::LazyLock::new(|| {
        Box::leak(Box::new([TrustedKey::for_tests(
            Purpose::Firmware,
            Generation::Release(1),
            KEYNUM,
            signing_key().verifying_key().to_bytes(),
        )]))
    });

/// A trust set holding the test signing key alone.
pub(crate) fn trust() -> &'static TrustSet {
    static TRUST: std::sync::LazyLock<TrustSet> =
        std::sync::LazyLock::new(|| TrustSet::for_tests(Purpose::Firmware, &KEYS));
    &TRUST
}

/// `bytes` as lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

/// The manifest line of a file `name` holding `data`.
pub(crate) fn manifest_line(name: &str, data: &[u8]) -> String {
    format!("SHA256 ({name}) = {}\n", hex(&sha256(data)))
}

/// A signature file over `message` as `signify -S -e` writes it, naming
/// `keynum`, signed by the test key.
pub(crate) fn signed_file(keynum: eveningstar_signify::Keynum, message: &str) -> Vec<u8> {
    let mut decoded = b"Ed".to_vec();
    decoded.extend_from_slice(&keynum);
    decoded.extend_from_slice(&signing_key().sign(message.as_bytes()).to_bytes());
    let mut file = format!(
        "untrusted comment: {COMMENT}\n{}\n",
        STANDARD.encode(&decoded)
    )
    .into_bytes();
    file.extend_from_slice(message.as_bytes());
    file
}
