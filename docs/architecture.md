# Architecture

How the firmware is structured, and why. This covers the decisions that span
crates and modules. The details of each part are in its crate and module
documentation (`//!` comments), which this document links to by name.

## Crates

The workspace's crates live flat under `crates/`.

| Crate               | Holds                                                     | Builds for        |
|---------------------|-----------------------------------------------------------|-------------------|
| `eveningstar-board` | Board support: the chip's pins, named by their role       | ESP32-C6          |
| `eveningstar-api`   | The API contract: endpoints, request and response types   | ESP32-C6 and host |
| `eveningstar`       | The firmware: services, transports, drivers and watchdogs | ESP32-C6          |

### `eveningstar-board`

All knowledge of the board lives here: which pin does what, how shared buses
are wired, and what each pin's starting state is. The board revision is chosen
with a cargo feature. `Board::new` consumes the chip's peripherals and hands out
pins by role, so the firmware never names a GPIO number.

The crate's documentation states what the crate itself guarantees, never what
its users do with what it hands out. It can't know what they do, now or later.

### `eveningstar-api`

The contract between the device and its clients: the types sent over the wire,
how they are serialised, the endpoints that carry them, and the constants
clients need, such as length limits. It has no behaviour and depends on no
transport, so other Rust programs can use it as a typed client library.

The crate is `no_std` and allocation-free, so the firmware can use it. The
`schema` feature adds JSON Schemas of the types and the OpenAPI document, which
need `alloc` and are only built on the host. The crate is listed in
`host_crates` in `nix/checks.nix`, so its tests run on the host.

### `eveningstar`

The firmware itself, split into services, which own the device's state and
work, and transports, which make the services reachable over a protocol. The
other modules are drivers and the [watchdogs](#faults-and-reboots).

## Services and transports

A service owns a piece of the device's state or work. A transport only
translates between its protocol and the services. HTTP is the only transport
today. MQTT, Modbus TCP or SNMP can be added later, beside it, without
changing the services.

- Each resource has a single owner. For example, one task will own the MeterBus
  UART, publish the latest readings, and take writes as commands, rather than
  every transport talking to the bus.
- Handlers hold no logic of their own. A handler takes a request, calls a
  service, and maps the result to a response. Validation that depends on the
  device's state belongs in the service, so every transport gets it.

### Services are capabilities

A service has no public free functions. Starting it returns a handle, and
holding the handle is the permission to use it. Handles are zero-sized `Copy`
structs with a private field where possible, so they cost nothing to pass
around and can't be made outside the service.

A service can hand out narrower handles derived from its full one. Each caller
gets the narrowest handle that does its job. The reboot service is the example
to follow: `reboot::start` returns the one `Reboot`, and from it come a
`GracefulReboot`, which can only request graceful reboots, and a
`RebootHolder`, which can only hold them off. The Ethernet link monitor gets a
`GracefulReboot`, so it can't reboot the device at once.

## The API

Every endpoint is under `/api/v1` (`v1::PATH`). A later, incompatible version
would live beside it under its own path.

### Each endpoint is declared once

The shape of the API is declared once, in `eveningstar-api`. The firmware and
the OpenAPI document both read it from there, so they can't disagree.

- Each endpoint is a type implementing `v1::Endpoint`. It declares the method,
  path, success status, summary and description, request and response bodies
  (`Empty` or `Json<T>`), and the errors the operation itself can answer with.
- Error sets are types too: `Infallible` for none, or an enum made by the
  `errors!` macro, each of whose variants is an `ErrorCode`. A `Json` request
  body adds `BodyError` (400 and 413) on its own.
- The `operations!` table in `v1.rs` lists every endpoint and builds
  `v1::Operation`, an enum with one variant per endpoint.
  `Operation::find(method, path)` gives the operation a request is for, or a
  `RouteError` (404 or 405).

### Serving the API

The firmware serves everything under `/api/v1` with one service, `http/api.rs`.
It calls `Operation::find`, then hands the operation to its handler in an
exhaustive `match`. The generic `serve` function reads the request body, calls
the handler, and writes the response with the endpoint's status code, or a
JSON `v1::Error`.

The compiler enforces the contract:

- An endpoint in the table with no handler leaves the `match` non-exhaustive.
- A handler whose request, response or error types differ from its endpoint's
  fails to type-check against `serve`.
- Status codes, paths and methods are only written in the endpoint
  declarations, so the firmware can't get them wrong.

Handlers live in one module per group of related operations, such as
`http/system.rs`. They take the service handle they need through picoserve's
`FromRef`, plus the parsed body, and return the response body or one of their
endpoint's errors. A reply that must act once the response is written, like a
reboot, returns `Closing`.

Only `http.rs` and `http/api.rs` use picoserve. Replacing the HTTP server means
rewriting those two files. The handlers and the API crate stay as they are.

### Adding an endpoint

1. Add its request and response types to `v1.rs`, deriving `JsonSchema` under
   the `schema` feature. Their doc comments become the schema descriptions.
2. Add a unit struct for the endpoint and implement `Endpoint` for it.
3. Add the struct to the `operations!` table.
4. Write the handler in the module for its group, and add its arm to the
   `match` in `http/api.rs`. The firmware doesn't build until you do.

### The OpenAPI document

The API is described code-first. `v1::openapi()` builds an OpenAPI 3.1 document
by walking `Operation::ALL`, with schemas generated by schemars. Run
`nix build .#openapi` to write it to `result/v1.json`.

The document is a build artifact and is never committed, so it can't go stale.

### The web UI

The planned web UI will be a static client that uses only the public API. It
gets no routes, form handlers or server-side rendering of its own, so
everything it does can also be done through the API directly. It will depend
on the generated OpenAPI document, never on the Rust crates.

### Authentication

Authentication isn't implemented yet. It is planned as a picoserve layer around
the API service that covers every endpoint by default, with explicit
exclusions such as a login endpoint. Endpoints mustn't be designed in a way
that would bypass it.

## Faults and reboots

The board has no reset button and runs unattended, so it must recover on its
own.

- **Faults reset the chip at once**, because the system's state can't be
  trusted. Faults are a panic, a supervised task missing its watchdog
  deadline, or a hung executor.
- **Everything else requests a reboot** through the reboot service, with a
  reason, such as the Ethernet link staying down. A graceful reboot waits for
  holds to be released, up to a deadline. An immediate one doesn't wait.

Only three places may reset the chip: the reboot service, the watchdog
supervisor and the panic handler. Clippy's `disallowed_methods`, configured in
`.clippy.toml`, rejects the reset functions everywhere else. Those three places
`#[expect]` the lint, with a reason.

The watchdogs come in three layers, each a backstop for the one above it: a
software supervisor over tasks, then the chip's main and RTC watchdogs. See
`watchdog.rs` for what each layer catches.

## Planned crates

- `morningstar`: the MeterBus and Modbus RTU protocol, the charge controllers'
  register maps and scaling. Pure `no_std`, tested on the host.
