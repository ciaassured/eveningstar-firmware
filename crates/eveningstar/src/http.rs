//! HTTP server: a fixed pool of workers, each serving one TCP connection at a
//! time.
//!
//! Handlers only translate between HTTP and the [services](crate::services).
//! [`api`] serves each API request with its operation's handler, and each
//! other submodule holds the handlers of one group of related operations.
//! [`ui`] serves the web UI at `/`.

mod api;
mod system;
mod ui;

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_time::Duration;
use eveningstar_api::v1;
use picoserve::extract::FromRef;
use picoserve::routing::PathRouter;
use picoserve::{Router, Timeouts};
use static_cell::ConstStaticCell;

use crate::services::reboot::Reboot;
use crate::services::system::System;

const PORT: u16 = 80;

/// Connections served at once; each worker holds one IP stack socket.
const WORKERS: usize = 4;

/// Size of each worker's TCP receive and transmit buffers.
const TCP_BUFFER_LEN: usize = 1024;

/// Size of each worker's buffer for the request line, headers and body.
const HTTP_BUFFER_LEN: usize = 2048;

static CONFIG: picoserve::Config = picoserve::Config::new(Timeouts {
    start_read_request: Duration::from_secs(5),
    persistent_start_read_request: Duration::from_secs(1),
    read_request: Duration::from_secs(3),
    write: Duration::from_secs(1),
})
.keep_connection_alive();

/// A worker's buffers.
struct Buffers {
    tcp_rx: [u8; TCP_BUFFER_LEN],
    tcp_tx: [u8; TCP_BUFFER_LEN],
    http: [u8; HTTP_BUFFER_LEN],
}

/// The services the handlers reach. Each handler takes the one it needs,
/// `&'static Service` or a handle, through [`FromRef`].
#[derive(Clone, Copy)]
pub(crate) struct AppState {
    pub(crate) system: &'static System,
    pub(crate) reboot: Reboot,
}

impl FromRef<AppState> for &'static System {
    fn from_ref(state: &AppState) -> Self {
        state.system
    }
}

impl FromRef<AppState> for Reboot {
    fn from_ref(state: &AppState) -> Self {
        state.reboot
    }
}

fn router() -> Router<impl PathRouter<AppState>, AppState> {
    Router::new()
        .route_service("/", ui::Ui)
        // A nested service only sees paths that continue past its prefix, so
        // the prefix itself is routed separately.
        .route_service(v1::PATH, api::Api)
        .nest_service(v1::PATH, api::Api)
}

#[embassy_executor::task(pool_size = WORKERS)]
async fn worker(
    id: usize,
    stack: Stack<'static>,
    state: AppState,
    buffers: &'static mut Buffers,
) -> ! {
    let app = router().with_state(state);
    match picoserve::Server::new(&app, &CONFIG, &mut buffers.http)
        .listen_and_serve(id, stack, PORT, &mut buffers.tcp_rx, &mut buffers.tcp_tx)
        .await {}
}

/// Spawns the workers serving HTTP on `stack`.
pub(crate) fn start(spawner: Spawner, stack: Stack<'static>, state: AppState) {
    // Built at compile time, so the buffers never pass through the stack.
    static BUFFERS: [ConstStaticCell<Buffers>; WORKERS] = [const {
        ConstStaticCell::new(Buffers {
            tcp_rx: [0; TCP_BUFFER_LEN],
            tcp_tx: [0; TCP_BUFFER_LEN],
            http: [0; HTTP_BUFFER_LEN],
        })
    }; WORKERS];

    for (id, buffers) in BUFFERS.iter().enumerate() {
        spawner.spawn(defmt::unwrap!(worker(id, stack, state, buffers.take())));
    }
}
