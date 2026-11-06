//! The storage service: the chain's report and the content modules of the
//! internal flash, which the storage API reads, and the boot's random value
//! its entity tags carry.

use core::fmt::Write as _;

use eveningstar_boot::{App, Bootloader, Otadata, PartitionTable};
use eveningstar_storage::Epoch;
use eveningstar_storage::chain::Report;

use crate::sha::Sha;

/// Most bytes of an entity tag: [`v1::storage::ETAG_MAX_LEN`].
///
/// [`v1::storage::ETAG_MAX_LEN`]: eveningstar_api::v1::storage::ETAG_MAX_LEN
pub(crate) const TAG_LEN: usize = eveningstar_api::v1::storage::ETAG_MAX_LEN;

/// The internal flash as the chain found it, and its content modules.
pub(crate) struct Storage {
    report: Report,
    bootloader: &'static Bootloader<'static>,
    table: &'static PartitionTable<'static>,
    otadata: Option<&'static Otadata<'static>>,
    app: App<'static, 'static>,
    sha: Sha,
    /// The boot's random value, which keeps entity tags of different boots
    /// apart, as epochs restart at every boot.
    boot: u64,
}

impl Storage {
    /// The service over `report` and the content modules, hashing views
    /// with `sha`, its tags carrying `boot`.
    pub(crate) const fn new(
        report: Report,
        bootloader: &'static Bootloader<'static>,
        table: &'static PartitionTable<'static>,
        otadata: Option<&'static Otadata<'static>>,
        app: App<'static, 'static>,
        sha: Sha,
        boot: u64,
    ) -> Self {
        Self {
            report,
            bootloader,
            table,
            otadata,
            app,
            sha,
            boot,
        }
    }

    /// What the chain found at boot.
    pub(crate) const fn report(&self) -> &Report {
        &self.report
    }

    /// The bootloader's module.
    pub(crate) const fn bootloader(&self) -> &'static Bootloader<'static> {
        self.bootloader
    }

    /// The located table's module.
    pub(crate) const fn table(&self) -> &'static PartitionTable<'static> {
        self.table
    }

    /// The otadata's module, when the chain located one.
    pub(crate) const fn otadata(&self) -> Option<&'static Otadata<'static>> {
        self.otadata
    }

    /// The app slots' modules.
    pub(crate) const fn app(&self) -> App<'static, 'static> {
        self.app
    }

    /// Permission to hash a view.
    pub(crate) const fn sha(&self) -> Sha {
        self.sha
    }

    /// The strong entity tag of a view at `epoch`, quotes included: the
    /// boot's random value in hex, a hyphen, and the epoch.
    pub(crate) fn tag(&self, epoch: Epoch) -> heapless::String<TAG_LEN> {
        let mut tag = heapless::String::new();
        // 18 + 1 + at most 20 digits of a 64-bit count, within `TAG_LEN`.
        let _ = write!(tag, "\"{:016x}-{epoch}\"", self.boot);
        tag
    }
}
