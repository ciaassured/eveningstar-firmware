//! Fixtures shared by the crate's tests.

use core::future::Future;

use eveningstar_esp_formats::pace::Pace;
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
