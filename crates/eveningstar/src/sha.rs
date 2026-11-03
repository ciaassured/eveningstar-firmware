//! The chip's SHA accelerator, shared by every user of SHA in the firmware.
//!
//! [`start`] takes the peripheral once per boot and starts esp-hal's SHA
//! backend, which runs the hashing of every hash in progress through one work
//! queue, one operation at a time, and keeps running for the rest of the
//! boot. It returns a [`Sha`] handle, which is `Copy`, so each user holds its
//! own. A hash in progress, a [`Sha256`], keeps its state between operations
//! rather than in the peripheral, so hashes of different users interleave
//! without disturbing one another.
//!
//! A [`Sha256`] is a [`Sha256Hasher`], so the storage crates' long jobs hash
//! with it. An operation hashes its bytes before it returns, busy-waiting on
//! the peripheral, so one call to [`Sha256Hasher::update`] holds the executor
//! for as long as its bytes take to hash.

use esp_hal::peripherals::SHA;
use esp_hal::sha::{Sha256Context, ShaBackend, ShaWorkQueueDriver};
use eveningstar_esp_formats::hash::Sha256Hasher;
use static_cell::StaticCell;

/// Starts the SHA backend on `sha` and returns the handle that hashes with
/// it.
///
/// # Panics
///
/// If called more than once.
pub(crate) fn start(sha: SHA<'static>) -> Sha {
    static BACKEND: StaticCell<ShaBackend<'static>> = StaticCell::new();
    static DRIVER: StaticCell<ShaWorkQueueDriver<'static, 'static>> = StaticCell::new();

    let backend = BACKEND.init(ShaBackend::new(sha));
    // Kept for the rest of the boot: the backend stops when it is dropped.
    DRIVER.init(backend.start());
    Sha(())
}

/// Permission to hash with the SHA accelerator. Only [`start`] creates one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sha(());

impl Sha {
    /// A new SHA-256 hash, of nothing yet.
    #[expect(
        clippy::unused_self,
        reason = "the handle is the permission to hash, not data"
    )]
    pub(crate) fn sha256(self) -> Sha256 {
        Sha256(Sha256Context::new())
    }
}

/// A SHA-256 hash in progress on the SHA accelerator; see [`Sha::sha256`].
/// A clone carries the hash so far.
#[derive(Clone)]
pub(crate) struct Sha256(Sha256Context);

impl Sha256Hasher for Sha256 {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes).wait_blocking();
    }

    fn finalize(mut self) -> [u8; 32] {
        let mut digest = [0; 32];
        self.0.finalize(&mut digest).wait_blocking();
        digest
    }
}
