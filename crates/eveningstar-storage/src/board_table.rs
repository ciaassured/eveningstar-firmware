//! The board's partition table compared with the generated layout.

use eveningstar_esp_formats::Source;
use eveningstar_esp_formats::table::{Comparison, compare};

use crate::layout;

/// The comparison of the board's table with the generated layout.
pub type LayoutComparison = Comparison<{ layout::PARTITIONS.len() }>;

/// Compares the table at [`layout::TABLE_OFFSET`] in `flash`, a source that
/// starts at the start of the flash, with [`layout::PARTITIONS`].
pub fn compare_layout<S: Source + ?Sized>(flash: &S) -> LayoutComparison {
    compare(flash, layout::TABLE_OFFSET, &layout::PARTITIONS)
}

#[cfg(test)]
mod tests;
