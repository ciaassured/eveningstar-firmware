//! `/system`: the device's identity and uptime, and reboots.

use core::convert::Infallible;

use embassy_time::Instant;

use eveningstar_api::v1;

use super::api::{Closing, Reply};
use crate::services::reboot::{self, Holder, Reason, Reboot};
use crate::services::system::System;

/// [`v1::GetSystem`].
pub(super) async fn get_system(system: &'static System, (): ()) -> Result<v1::System, Infallible> {
    Ok(v1::System {
        name: system.name(),
        id: system.id(),
        uptime_s: System::uptime().as_secs(),
    })
}

// Every holder fits in a status.
const _: () = assert!(reboot::HOLDERS <= v1::REBOOT_HOLDERS_MAX);

/// [`v1::GetRebootStatus`].
pub(super) async fn get_reboot(reboot: Reboot, (): ()) -> Result<v1::RebootStatus, Infallible> {
    let status = reboot.status();
    let now = Instant::now();
    Ok(v1::RebootStatus {
        pending: status.pending.map(|pending| v1::PendingReboot {
            reason: reason(pending.reason),
            remaining_s: pending.deadline.saturating_duration_since(now).as_secs(),
        }),
        holds: status
            .holds()
            .map(|(holder, count)| v1::RebootHold {
                holder: match holder {
                    Holder::Http => v1::RebootHolder::Http,
                },
                count,
            })
            .collect(),
    })
}

fn reason(reason: Reason) -> v1::RebootReason {
    match reason {
        Reason::LinkDown => v1::RebootReason::LinkDown,
        Reason::Api => v1::RebootReason::Api,
    }
}

/// [`v1::Reboot`]: reboots once the client has the response.
pub(super) async fn post_reboot(
    reboot: Reboot,
    request: v1::RebootRequest,
) -> Result<impl Reply<()>, Infallible> {
    // With no other hold, a graceful reboot would reset the chip before the
    // response is written.
    let hold = reboot.holder(Holder::Http).hold();
    if request.mode == v1::RebootMode::Graceful {
        reboot.graceful(Reason::Api);
    }
    Ok(Closing::new((), move || match request.mode {
        v1::RebootMode::Graceful => drop(hold),
        // Reboots whether or not the response reached the client.
        v1::RebootMode::Immediate => reboot.immediate(Reason::Api),
    }))
}
