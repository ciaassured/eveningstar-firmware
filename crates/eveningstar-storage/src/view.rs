//! What readers hand out, whatever the device: snapshots and their errors.

/// Why a reader refused a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// A write session is open on the region.
    Busy,
    /// The span doesn't lie inside the region.
    OutOfRange,
}
