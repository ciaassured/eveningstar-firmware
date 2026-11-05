//! Roles: what a region holds, and so what its handles can do.
//!
//! A role is an uninhabited marker type. It names its device ([`Internal`])
//! and its class, publishable or sensitive. A role of the compiled layout
//! also has its size at compile time ([`LayoutSize`]), and its offset when
//! that doesn't depend on the slot that booted ([`Placed`]).
//!
//! Each role below gives its span, then its class, whether it is
//! [`LayoutSize`] and [`Placed`], and what its writer does, if it has one.
//!
//! - [`Bootloader`]: 0x0 up to the located table, or up to the compiled table
//!   offset. Publishable.
//! - [`PartitionTable`]: the compiled table's sector. Publishable;
//!   [`LayoutSize`], [`Placed`].
//! - [`Otadata`]: `otadata`. Publishable; [`LayoutSize`], [`Placed`]; the
//!   writer erases and programs.
//! - [`CurrentApp`]: the slot that booted. Publishable; [`LayoutSize`].
//! - [`AlternateApp`]: the other slot. Publishable; [`LayoutSize`]; the writer
//!   erases and programs.
//! - [`CurrentConfig`]: the current slot's configuration. Sensitive;
//!   [`LayoutSize`]; the writer erases and programs.
//! - [`AlternateConfig`]: the alternate slot's configuration. Sensitive;
//!   [`LayoutSize`]; the writer erases only.
//! - [`Diag`]: `diag`. Sensitive; [`LayoutSize`], [`Placed`]; the writer
//!   erases and programs.
//! - [`Factory`]: `factory`. Sensitive; [`LayoutSize`], [`Placed`].
//! - [`Chip`]: the whole flash. Sensitive.
//! - [`FoundTable`]: the sector of the one table a search finds. Publishable.
//! - [`FoundApp`]: the located table's app partition at the booted offset, or
//!   the image there when no valid table is located. Publishable.
//! - [`FoundOtadata`]: the located table's last `data`/`ota` partition.
//!   Sensitive.
//!
//! Only [`Publishable`] roles have their contents published. The otadata is
//! publishable at its compiled offset, where the layout places it; the
//! located otadata of the unverified tier isn't, as only its location is
//! known, not that it is the otadata. Roles with a
//! writer are [`Writable`]; those whose writer also programs are
//! [`Programmable`]. Every trait here is sealed.
//!
//! ```
//! use eveningstar_storage::roles::{CurrentConfig, Diag, LayoutSize, Placed, PartitionTable};
//!
//! const { assert!(CurrentConfig::SIZE >= 2 * eveningstar_storage::nor::SECTOR_SIZE) };
//! const { assert!(Diag::OFFSET == eveningstar_storage::layout::DIAG.offset) };
//! const { assert!(PartitionTable::SIZE == eveningstar_storage::nor::SECTOR_SIZE) };
//! ```
//!
//! The current and alternate roles have a size but no compiled offset:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::roles::{AlternateApp, Placed};
//!
//! const OFFSET: u32 = AlternateApp::OFFSET;
//! ```
//!
//! and the bootloader's span is set at runtime:
//!
//! ```compile_fail,E0599
//! use eveningstar_storage::roles::{Bootloader, LayoutSize};
//!
//! const SIZE: u32 = Bootloader::SIZE;
//! ```

use crate::layout;
use crate::nor::SECTOR_SIZE;
use crate::span::Span;

mod sealed {
    /// Implemented by the roles and the device markers of this module only.
    pub trait Sealed {}
}

use sealed::Sealed;

/// The internal flash: the NOR flash the chip boots from, laid out by the
/// compiled layout.
#[derive(Debug)]
pub enum Internal {}

impl Sealed for Internal {}

/// A device marker: one flash chip.
pub trait Device: Sealed {
    /// The device's name, for logs.
    const NAME: &'static str;
}

impl Device for Internal {
    const NAME: &'static str = "internal";
}

/// A region's role: what it holds, on which device, and so what its handles
/// can do.
pub trait Role: Sealed {
    /// A marker naming the device, such as [`Internal`].
    type Device: Device;
    /// The role's name, for logs.
    const NAME: &'static str;
}

/// A role whose size comes from the compiled layout. The current and
/// alternate roles have one, though their offset depends on the slot booted.
pub trait LayoutSize: Role {
    /// Size of the region in bytes.
    const SIZE: u32;
}

/// A role at a fixed offset of the compiled layout, whatever slot booted.
pub trait Placed: LayoutSize {
    /// Offset of the region from the start of its device.
    const OFFSET: u32;
}

/// A role located from a known point, whose verified contents may be
/// published.
pub trait Publishable: Role {}

/// A role with a writer, whose sessions erase.
pub trait Writable: Role {}

/// A role whose writer's sessions also program.
pub trait Programmable: Writable {}

/// The bootloader at 0x0, up to the located table, or up to the compiled
/// table offset when no table is located.
#[derive(Debug)]
pub enum Bootloader {}

/// The compiled table's sector, at [`layout::TABLE_OFFSET`].
#[derive(Debug)]
pub enum PartitionTable {}

/// The `otadata` partition of the compiled layout.
#[derive(Debug)]
pub enum Otadata {}

/// The app slot that booted.
#[derive(Debug)]
pub enum CurrentApp {}

/// The app slot that didn't boot.
#[derive(Debug)]
pub enum AlternateApp {}

/// The configuration partition paired with the slot that booted.
#[derive(Debug)]
pub enum CurrentConfig {}

/// The configuration partition paired with the slot that didn't boot.
#[derive(Debug)]
pub enum AlternateConfig {}

/// The `diag` partition of the compiled layout.
#[derive(Debug)]
pub enum Diag {}

/// The `factory` partition of the compiled layout.
#[derive(Debug)]
pub enum Factory {}

/// The whole flash, whose size is read at runtime.
#[derive(Debug)]
pub enum Chip {}

/// The sector of the one table a search of the flash finds.
#[derive(Debug)]
pub enum FoundTable {}

/// The app the bootloader started: the app partition at the booted offset of
/// the located table, as the bootloader loads them, or the image at the
/// booted offset when no valid table is located.
#[derive(Debug)]
pub enum FoundApp {}

/// The located table's `data`/`ota` partition, as the bootloader loads them:
/// the last such entry.
#[derive(Debug)]
pub enum FoundOtadata {}

const _: () = assert!(layout::OTA_0.size == layout::OTA_1.size);
const _: () = assert!(layout::CONFIG_0.size == layout::CONFIG_1.size);

/// The spans the compiled layout gives to sensitive data: `factory`, both
/// configurations and `diag`.
const SENSITIVE: [Span; 4] = [
    Span::of(&layout::FACTORY),
    Span::of(&layout::CONFIG_0),
    Span::of(&layout::CONFIG_1),
    Span::of(&layout::DIAG),
];

/// Whether `span`, absolute on the internal flash, shares a byte with a span
/// the compiled layout gives to sensitive data. No view is published over
/// one, and no reader of a role located at runtime is made over one.
pub(crate) fn is_sensitive(span: Span) -> bool {
    SENSITIVE.iter().any(|sensitive| sensitive.overlaps(span))
}

macro_rules! roles {
    ($($role:ident = $name:literal;)*) => {$(
        impl Sealed for $role {}

        impl Role for $role {
            type Device = Internal;
            const NAME: &'static str = $name;
        }
    )*};
}

roles! {
    Bootloader = "bootloader";
    PartitionTable = "partition table";
    Otadata = "otadata";
    CurrentApp = "current app";
    AlternateApp = "alternate app";
    CurrentConfig = "current config";
    AlternateConfig = "alternate config";
    Diag = "diag";
    Factory = "factory";
    Chip = "chip";
    FoundTable = "found table";
    FoundApp = "found app";
    FoundOtadata = "found otadata";
}

macro_rules! layout_size {
    ($($role:ident = $size:expr $(, at $offset:expr)?;)*) => {$(
        impl LayoutSize for $role {
            const SIZE: u32 = $size;
        }

        $(impl Placed for $role {
            const OFFSET: u32 = $offset;
        })?
    )*};
}

layout_size! {
    PartitionTable = SECTOR_SIZE, at layout::TABLE_OFFSET;
    Otadata = layout::OTADATA.size, at layout::OTADATA.offset;
    CurrentApp = layout::OTA_0.size;
    AlternateApp = layout::OTA_0.size;
    CurrentConfig = layout::CONFIG_0.size;
    AlternateConfig = layout::CONFIG_0.size;
    Diag = layout::DIAG.size, at layout::DIAG.offset;
    Factory = layout::FACTORY.size, at layout::FACTORY.offset;
}

macro_rules! mark {
    ($trait:ident: $($role:ident),*) => {$(
        impl $trait for $role {}
    )*};
}

mark!(Publishable: Bootloader, PartitionTable, Otadata, CurrentApp, AlternateApp, FoundTable, FoundApp);
mark!(Writable: Otadata, AlternateApp, CurrentConfig, AlternateConfig, Diag);
mark!(Programmable: Otadata, AlternateApp, CurrentConfig, Diag);

#[cfg(test)]
mod tests {
    use super::*;

    fn publishable<R: Publishable>() -> &'static str {
        R::NAME
    }

    fn programmable<R: Programmable>() -> &'static str {
        R::NAME
    }

    #[test]
    fn placed_roles_lie_where_the_layout_puts_them() {
        assert_eq!(
            (PartitionTable::OFFSET, PartitionTable::SIZE),
            (layout::TABLE_OFFSET, SECTOR_SIZE)
        );
        assert_eq!(
            (Otadata::OFFSET, Otadata::SIZE),
            (layout::OTADATA.offset, layout::OTADATA.size)
        );
        assert_eq!(
            (Diag::OFFSET, Diag::SIZE),
            (layout::DIAG.offset, layout::DIAG.size)
        );
        assert_eq!(
            (Factory::OFFSET, Factory::SIZE),
            (layout::FACTORY.offset, layout::FACTORY.size)
        );
    }

    #[test]
    fn slot_roles_have_the_slot_sizes() {
        assert_eq!(CurrentApp::SIZE, layout::OTA_1.size);
        assert_eq!(AlternateApp::SIZE, layout::OTA_1.size);
        assert_eq!(CurrentConfig::SIZE, layout::CONFIG_1.size);
        assert_eq!(AlternateConfig::SIZE, layout::CONFIG_1.size);
    }

    #[test]
    fn roles_name_themselves_and_their_device() {
        assert_eq!(publishable::<FoundApp>(), "found app");
        assert_eq!(publishable::<Otadata>(), "otadata");
        assert_eq!(programmable::<Otadata>(), "otadata");
        assert_eq!(<<Chip as Role>::Device as Device>::NAME, "internal");
    }
}
