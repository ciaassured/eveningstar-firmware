//! The device's identity and uptime.

use embassy_time::{Duration, Instant};
use eveningstar_api::DeviceId;

use crate::identity::{self, NAME_MAX_LEN};

/// The device's identity and how long it has been running.
#[derive(Debug)]
pub(crate) struct System {
    id: DeviceId,
}

impl System {
    pub(crate) const fn new(id: DeviceId) -> Self {
        Self { id }
    }

    /// The device's globally unique ID.
    pub(crate) fn id(&self) -> DeviceId {
        self.id
    }

    /// The device's name on networks, e.g. `eveningstar-0kd7-9c6`.
    pub(crate) fn name(&self) -> heapless::String<NAME_MAX_LEN> {
        identity::name(self.id)
    }

    /// Time since the device started.
    pub(crate) fn uptime() -> Duration {
        Duration::from_ticks(Instant::now().as_ticks())
    }
}
