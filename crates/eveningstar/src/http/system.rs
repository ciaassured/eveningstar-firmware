//! `/system`: the device's identity and uptime, and reboots.

use embassy_time::Instant;

use eveningstar_api::v1;
use picoserve::ResponseSent;
use picoserve::Router;
use picoserve::extract::State;
use picoserve::io::Read;
use picoserve::response::{Connection, IntoResponse, Json, NoContent, ResponseWriter, StatusCode};
use picoserve::routing::{PathRouter, get};

use super::AppState;
use super::error::ApiJson;
use crate::services::reboot::{self, Holder, Reason, Reboot};
use crate::services::system::System;

/// Adds the `/system` endpoints to `routes`.
pub(super) fn routes(
    routes: Router<impl PathRouter<AppState>, AppState>,
) -> Router<impl PathRouter<AppState>, AppState> {
    routes
        .route(v1::paths::SYSTEM, get(get_system))
        .route(v1::paths::REBOOT, get(get_reboot).post(post_reboot))
}

async fn get_system(State(system): State<&'static System>) -> Json<v1::System> {
    Json(v1::System {
        name: system.name(),
        id: system.id(),
        uptime_s: System::uptime().as_secs(),
    })
}

// Every holder fits in a status.
const _: () = assert!(reboot::HOLDERS <= v1::REBOOT_HOLDERS_MAX);

async fn get_reboot(State(reboot): State<Reboot>) -> Json<v1::RebootStatus> {
    let status = reboot.status();
    let now = Instant::now();
    Json(v1::RebootStatus {
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

async fn post_reboot(
    State(reboot): State<Reboot>,
    ApiJson(request): ApiJson<v1::RebootRequest>,
) -> RebootAccepted {
    RebootAccepted {
        reboot,
        mode: request.mode,
    }
}

/// Answers 202 Accepted, then reboots once the client has the response.
struct RebootAccepted {
    reboot: Reboot,
    mode: v1::RebootMode,
}

impl IntoResponse for RebootAccepted {
    async fn write_to<R: Read, W: ResponseWriter<Error = R::Error>>(
        self,
        connection: Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        // Writing the response returns once the client has acknowledged it.
        // The length tells the client the response is complete, as the reboot
        // drops the connection without closing it.
        let accepted = (
            StatusCode::ACCEPTED,
            [("Content-Length", "0"), ("Connection", "close")],
            NoContent,
        );
        match self.mode {
            v1::RebootMode::Graceful => {
                // With no other hold, the reboot task would reset the chip
                // before the response is written.
                let _hold = self.reboot.holder(Holder::Http).hold();
                self.reboot.graceful(Reason::Api);
                accepted.write_to(connection, response_writer).await
            }
            v1::RebootMode::Immediate => {
                // Reboots whether or not the response reached the client.
                let _ = accepted.write_to(connection, response_writer).await;
                self.reboot.immediate(Reason::Api)
            }
        }
    }
}
