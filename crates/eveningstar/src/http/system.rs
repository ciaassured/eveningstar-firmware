//! `/system`: the device's identity and uptime.

use eveningstar_api::v1;
use picoserve::Router;
use picoserve::extract::State;
use picoserve::response::Json;
use picoserve::routing::{PathRouter, get};

use super::AppState;
use crate::services::system::System;

/// Adds the `/system` endpoints to `routes`.
pub(super) fn routes(
    routes: Router<impl PathRouter<AppState>, AppState>,
) -> Router<impl PathRouter<AppState>, AppState> {
    routes.route("/system", get(get_system))
}

async fn get_system(State(system): State<&'static System>) -> Json<v1::System> {
    Json(v1::System {
        name: system.name(),
        id: system.id(),
        uptime_s: System::uptime().as_secs(),
    })
}
