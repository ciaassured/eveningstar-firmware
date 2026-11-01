//! The regions of the internal flash, named by what they are to the running
//! firmware: whether their contents may leave the device, which the firmware
//! holds a handle for this boot, and what each is to the boot.
//!
//! The boot chain's regions come first, the bootloader and the partition
//! table, then the layout's partitions in table order. An app slot or a
//! configuration is named by its role this boot ([`AppRole`]): the slot the
//! app booted from is the current one, so the names swap when the device
//! boots the other slot. When the app didn't boot from either slot, neither
//! slot nor configuration has a role, so they are left out, and the running
//! app is listed by its handle's span alone, when the firmware holds one.

use eveningstar_storage::image::{self, AppDescriptor, AppInfo, BootloaderDescriptor, ImageError};
use eveningstar_storage::otadata::{self, Otadata};
use eveningstar_storage::partitions::Read;
use eveningstar_storage::table::{self, Match};
use eveningstar_storage::{Entry, Slot, Source, Tier, layout};

use super::{AppRole, Check, Service, StorageView};

/// A region of the internal flash, named by what it is to the running
/// firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub(crate) enum RegionName {
    /// From the start of the flash up to the partition table.
    Bootloader,
    /// The partition table the bootloader read.
    PartitionTable,
    /// The otadata.
    Otadata,
    /// An app slot, by its role: the running app, or the slot that isn't
    /// running.
    App(AppRole),
    /// The configuration paired with an app slot, by the slot's role.
    Config(AppRole),
    /// Diagnostics.
    Diag,
    /// Data written when the board was made.
    Factory,
    /// Space kept for later firmware.
    Reserved,
}

/// Whether a region's contents may leave the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub(crate) enum Contents {
    /// They may: the regions whose handles implement [`Read`].
    Public,
    /// They never leave the device, as bytes or as a digest: the
    /// configurations, diagnostics and factory data.
    Sensitive,
    /// No handle covers the region, and nothing is in it yet.
    Reserved,
}

impl RegionName {
    /// Whether the region's contents may leave the device. A property of the
    /// region, the same in every tier.
    pub(crate) const fn contents(self) -> Contents {
        match self {
            Self::Bootloader | Self::PartitionTable | Self::Otadata | Self::App(_) => {
                Contents::Public
            }
            Self::Config(_) | Self::Diag | Self::Factory => Contents::Sensitive,
            Self::Reserved => Contents::Reserved,
        }
    }
}

/// A region as the firmware finds it this boot.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    /// Its name.
    pub(crate) name: RegionName,
    /// The layout's entry for it; `None` for the boot chain, and for the
    /// running app when it didn't boot from a slot.
    pub(crate) entry: Option<Entry>,
    /// Offset from the start of the flash: the handle's when the firmware
    /// holds one, else the layout's.
    pub(crate) offset: u32,
    /// Size in bytes: the handle's when the firmware holds one, else the
    /// layout's.
    pub(crate) size: u32,
    /// Whether the firmware holds a handle for it this boot. The service
    /// holds the public handles only, and no task owns a sensitive one yet,
    /// so a sensitive region is never available.
    pub(crate) available: bool,
    /// How the table at the compiled offset compares with the layout's entry
    /// for it; `None` without an entry.
    pub(crate) layout: Option<Match>,
    /// What it is to the boot.
    pub(crate) roles: Roles,
}

/// What a region is to the boot.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Roles {
    /// It is the app slot running.
    pub(crate) running: bool,
    /// It is the app slot the otadata selects for the next boot.
    pub(crate) selected: bool,
    /// It is the app slot the bootloader would roll back to.
    pub(crate) rollback_target: bool,
}

/// An app slot the firmware holds a handle for, and what its headers say,
/// read without hashing when asked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AppSlot {
    /// The slot it is; `None` when the app didn't boot from one.
    pub(crate) slot: Option<Slot>,
    /// The handle's offset from the start of the flash.
    pub(crate) offset: u32,
    /// The handle's size in bytes.
    pub(crate) size: u32,
    /// The current result of its check, as [`StorageView::image`] gives it.
    pub(crate) check: Option<Check>,
    /// The image's header and length.
    pub(crate) info: Result<AppInfo, ImageError>,
}

impl StorageView {
    /// The flash's size in bytes, from its JEDEC ID; 0 when the ID isn't one
    /// the firmware knows.
    pub(crate) fn capacity(self) -> u32 {
        self.0.capacity
    }

    /// The offset of the app the bootloader started; `None` when the flash's
    /// memory mapping doesn't say.
    pub(crate) fn booted(self) -> Option<u32> {
        self.0.booted
    }

    /// Whether the tier reached lets the firmware save its configuration: the
    /// data tier and up.
    pub(crate) fn persistent_config(self) -> bool {
        self.0.report.tier() >= Tier::Data
    }

    /// Whether the tier reached lets the firmware install updates and roll
    /// them back: the full tier.
    pub(crate) fn updates(self) -> bool {
        self.0.report.tier() == Tier::Full
    }

    /// Every region: the bootloader, the partition table, the running app
    /// when it didn't boot from a slot and the firmware holds its handle, then
    /// the layout's partitions in table order.
    pub(crate) fn regions(self) -> impl Iterator<Item = Region> {
        let service = self.0;
        (0..BOOT_REGIONS + layout::PARTITIONS.len())
            .filter_map(move |index| service.region_at(index))
    }

    /// The app slot in `role`, when the firmware holds a handle for it, with
    /// its headers read now.
    pub(crate) fn app(self, role: AppRole) -> Option<AppSlot> {
        let service = self.0;
        let info = match role {
            AppRole::Current => image::inspect_app(&service.public.boot.running_app?),
            AppRole::Alternate => image::inspect_app(&service.public.other_app?),
        };
        let (offset, size) = service.span(role)?;
        Some(AppSlot {
            slot: service.slot(role),
            offset,
            size,
            check: service.current(role),
            info,
        })
    }

    /// The descriptor of the image in the app slot in `role`, read now;
    /// `None` when the firmware holds no handle for the slot or the
    /// descriptor doesn't read.
    pub(crate) fn app_descriptor(self, role: AppRole) -> Option<AppDescriptor> {
        let service = self.0;
        match role {
            AppRole::Current => read_descriptor(&service.public.boot.running_app?),
            AppRole::Alternate => read_descriptor(&service.public.other_app?),
        }
    }

    /// Both entries of the otadata, read now; `None` below the data tier,
    /// where the firmware holds no handle for it.
    pub(crate) fn otadata(self) -> Option<Otadata> {
        self.0.public.otadata.map(|handle| otadata::read(&handle))
    }

    /// The bootloader's descriptor, read now, when the bootloader verified at
    /// boot: `None` when it didn't, when it has no descriptor or when the
    /// read fails.
    pub(crate) fn bootloader_descriptor(self) -> Option<BootloaderDescriptor> {
        self.0.report.bootloader.ok()?;
        image::bootloader_descriptor(&self.0.public.boot.bootloader)
            .ok()
            .flatten()
    }
}

/// The regions before the layout's partitions: the bootloader, the table,
/// and the running app when it didn't boot from a slot.
const BOOT_REGIONS: usize = 3;

/// The descriptor of the app image in `handle`, a public handle. Never
/// inlined, so the read and its error stay out of its callers' frames.
#[inline(never)]
fn read_descriptor<P: Read + Source>(handle: &P) -> Option<AppDescriptor> {
    image::app_descriptor(handle).ok()
}

impl Service {
    /// The slot the app booted from: the one `split` found in the data and
    /// full tiers, else the slot whose offset the app booted from, whether or
    /// not the table on the board puts it there.
    fn current_slot(&self) -> Option<Slot> {
        self.report.running.or_else(|| {
            Slot::ALL
                .into_iter()
                .find(|slot| slot.partition().offset == self.report.booted)
        })
    }

    /// The name this boot of the layout's partition `entry`, or `None` for an
    /// app slot or configuration when the app didn't boot from a slot.
    fn name_of(&self, entry: &Entry) -> Option<RegionName> {
        let role = |slot: Slot| {
            self.current_slot().map(|current| {
                if current == slot {
                    AppRole::Current
                } else {
                    AppRole::Alternate
                }
            })
        };
        for slot in Slot::ALL {
            if *entry == slot.partition() {
                return role(slot).map(RegionName::App);
            }
            if *entry == slot.config() {
                return role(slot).map(RegionName::Config);
            }
        }
        [
            (layout::OTADATA, RegionName::Otadata),
            (layout::DIAG, RegionName::Diag),
            (layout::FACTORY, RegionName::Factory),
            (layout::RESERVED, RegionName::Reserved),
        ]
        .into_iter()
        .find_map(|(partition, name)| (partition == *entry).then_some(name))
    }

    /// The offset and length of the handle for the public region `name`,
    /// when the firmware holds one.
    fn handle_span(&self, name: RegionName) -> Option<(u32, u32)> {
        let boot = &self.public.boot;
        match name {
            RegionName::Bootloader => Some(span(&boot.bootloader)),
            RegionName::PartitionTable => boot.table.as_ref().map(span),
            RegionName::Otadata => self.public.otadata.as_ref().map(span),
            RegionName::App(role) => self.span(role),
            RegionName::Config(_)
            | RegionName::Diag
            | RegionName::Factory
            | RegionName::Reserved => None,
        }
    }

    /// The region at `index` in [`StorageView::regions`], or `None` when
    /// there is none there this boot.
    fn region_at(&self, index: usize) -> Option<Region> {
        let partition = index.checked_sub(BOOT_REGIONS);
        match (index, partition) {
            (0, _) => Some(self.region(RegionName::Bootloader, None, None)),
            (1, _) => Some(self.region(RegionName::PartitionTable, None, None)),
            (2, _) => (self.current_slot().is_none() && self.public.boot.running_app.is_some())
                .then(|| self.region(RegionName::App(AppRole::Current), None, None)),
            (_, Some(partition)) => {
                let entry = *layout::PARTITIONS.get(partition)?;
                let comparison = *self.report.table.partitions.get(partition)?;
                let name = self.name_of(&entry)?;
                Some(self.region(name, Some(entry), Some(comparison)))
            }
            (_, None) => None,
        }
    }

    /// The region `name`, whose layout entry is `entry` and compares with the
    /// table at the compiled offset as `comparison`.
    fn region(&self, name: RegionName, entry: Option<Entry>, comparison: Option<Match>) -> Region {
        let fallback = match (name, entry) {
            (_, Some(entry)) => (entry.offset, entry.size),
            (RegionName::PartitionTable, None) => (layout::TABLE_OFFSET, table::MAX_LEN),
            _ => (0, 0),
        };
        let (offset, size) = self.handle_span(name).unwrap_or(fallback);
        let boot = self.report.boot_state;
        let slot = match name {
            RegionName::App(role) => self.slot(role),
            _ => None,
        };
        Region {
            name,
            entry,
            offset,
            size,
            available: self.handle_span(name).is_some(),
            layout: comparison,
            roles: Roles {
                running: name == RegionName::App(AppRole::Current),
                selected: slot
                    .is_some_and(|slot| boot.is_some_and(|boot| boot.selected == Some(slot))),
                rollback_target: name == RegionName::App(AppRole::Alternate)
                    && boot.is_some_and(|boot| boot.rollback_target),
            },
        }
    }
}

/// The offset and length of a public handle.
fn span<P: Read>(handle: &P) -> (u32, u32) {
    (handle.offset(), handle.len())
}
