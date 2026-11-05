# Architecture

How the firmware is structured, and why. This covers the decisions that span
crates and modules. The details of each part are in its crate and module
documentation (`//!` comments), which this document links to by name.

## Crates

The workspace's crates live flat under `crates/`.

| Crate                     | Holds                                                                |
| ------------------------- | -------------------------------------------------------------------- |
| `eveningstar-board`       | Board support: the chip's pins and flash, named by role              |
| `eveningstar-api`         | The API contract: endpoints, request and response types              |
| `eveningstar-esp-formats` | The ESP-IDF formats: images, the partition table, the otadata        |
| `eveningstar-storage`     | The flash devices, their regions and handles, the verification chain |
| `eveningstar-boot`        | The content modules of the boot regions: parsed models and views     |
| `eveningstar-ota`         | The OTA mechanism: updates, activation, confirmation and rollback    |
| `eveningstar-system`      | Reboots, reboot holds and the task watchdog supervisor               |
| `eveningstar`             | The firmware: services, transports, drivers and watchdogs            |

`eveningstar-board` and `eveningstar` build for the ESP32-C6 only; the others
build for the ESP32-C6 and the host.

### `eveningstar-board`

All knowledge of the board lives here: which pin does what, how shared buses
are wired, and what each pin's starting state is. The board revision is chosen
with a cargo feature. `Board::new` consumes the chip's peripherals and hands out
pins by role, so the firmware never names a GPIO number. The two flash chips
are handed out separately: `internal_flash`, inside the ESP32-C6 module, with
`flash_cache` (SPI0, whose MMU says which flash page is mapped where), and
`external_flash`, the SPI NAND on the shared bus.

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

### The storage crates

`eveningstar-esp-formats`, `eveningstar-storage`, `eveningstar-boot` and
`eveningstar-ota` hold everything about the internal flash that doesn't need
the chip: the formats on the flash, the regions and their handles, the
verification chain, the content modules and the OTA mechanism. They have no
Espressif dependencies and reach the flash through the storage crate's
`NorDevice` trait, so their tests run on the host against a RAM flash that can
lose power at any erase or program. They are listed in `host_crates` in
`nix/checks.nix`. See [Storage](#storage).

They are system crates, meant to be reusable by other firmware: none depends
on the application, the `eveningstar` crate or the charge-controller code, so
they can be renamed and moved out of the workspace.

### `eveningstar-system`

The services any firmware on the board runs besides its application: the
reboot service with its holds, and the task watchdog supervisor (see
[Faults and reboots](#faults-and-reboots)). It holds their logic only and
names no part of the chip or of the firmware:

- The chip's software reset and hardware watchdogs are traits, `Reset` and
  `HardwareWatchdogs`, which the services take at start-up.
- Reboot reasons and reboot holders are type parameters, which the firmware
  fills with its own enums. Each holder is registered at start-up, in one of a
  fixed number of slots, and the reboot status lists holds by holder.
- Supervised tasks are registered at start-up, each in one of a fixed number
  of slots, with a name for the log and a marker type the firmware declares.
  A task's handle is a `WatchdogHandle` of its marker, so one task's handle
  can't be passed to another, and a marker registered twice panics at boot.

The services are futures, `RebootTask::run` and `Supervisor::run`, which the
firmware spawns as its tasks. The crate logs through `defmt` only with its
`defmt` feature, which the firmware enables. It is listed in `host_crates` in
`nix/checks.nix`, and its tests run the services on embassy-time's mock time
driver.

### `eveningstar`

The firmware itself, split into services, which own the device's state and
work, and transports, which make the services reachable over a protocol. The
other modules are drivers and the chip's side of the
[system services](#faults-and-reboots): its software reset and hardware
watchdogs.

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
to follow: starting it returns the one `Reboot`, and from it come a
`GracefulReboot`, which can only request graceful reboots, and a
`RebootHolder`, which can only hold them off, for the holder it registered.
The Ethernet link monitor gets a `GracefulReboot`, so it can't reboot the
device at once. The reboot handles each hold a reference to the service's
state, a `static` in the firmware, so they are `Copy` but not zero-sized.

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
  body adds `BodyError` (400, 413 and 415) on its own.
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

A request with a body must have the `Content-Type` `application/json`
(`v1::is_json_content_type`), and any other is answered with 415 before its body
is read. A form on another site can send only a few other types, and a request
of this type from another site needs the device's consent in a preflight that it
doesn't give, so a page you visit can't make the device act. The check is by
header, so it holds with or without authentication.

Every answer the API gives, errors included, carries the headers in
`http/api.rs`: `X-Content-Type-Options: nosniff` so a browser can't read the
data as HTML or script, `Cache-Control: no-store`,
`Cross-Origin-Resource-Policy: same-origin`, and a Content-Security-Policy of
`default-src 'none'; frame-ancestors 'none'` with its older form
`X-Frame-Options: DENY`, so nothing in an answer runs or loads if someone opens
one and it can't be framed. These are the headers of OWASP's REST security cheat
sheet that matter for JSON; the headers that only concern pages that load and
run things are left to the page (see [The web UI](#serving)). A path that no
route takes is answered with the API's `404` in JSON, with the same headers, not
the router's own plain text, which repeats the path it was asked for.

### Adding an endpoint

1. Add its request and response types to `v1.rs`, deriving `JsonSchema` under
   the `schema` feature. Their doc comments become the schema descriptions.
2. Add a unit struct for the endpoint and implement `Endpoint` for it, including
   the `SCOPE` a session needs to call it.
3. Add the struct to the `operations!` table.
4. Write the handler in the module for its group, and add its arm to the
   `match` in `http/api.rs`. The firmware doesn't build until you do.

### The OpenAPI document

The API is described code-first. `v1::openapi()` builds an OpenAPI 3.1 document
by walking `Operation::ALL`, with schemas generated by schemars. Run
`nix build .#openapi` to write it to `result/v1.json`.

The document is a build artifact and is never committed, so it can't go stale.

### Authentication

Authentication isn't implemented yet. It is planned as a picoserve layer around
the API service that covers every endpoint by default, with explicit
exclusions such as a login endpoint. Endpoints mustn't be designed in a way
that would bypass it.

## The web UI

The web UI is a static client of the public API. It has no routes, form
handlers or server-side rendering of its own, so everything it does can also be
done through the API directly. It depends on the generated OpenAPI document,
never on the Rust crates. The firmware serves it as a single page at `/`, and
the page routes with the URL fragment.

Security is a primary requirement, at runtime and during development. The
device runs unattended for years, often without updates, on networks its owner
doesn't fully control. The choices below that trade convenience for isolation,
fewer dependencies or a stricter browser policy were made on purpose.

### The build chain

Nix builds the page from the API crate to the bytes the firmware embeds. Its
sandboxed steps reach the network only to fetch the locked packages, and no npm
code runs outside a sandbox.

```
eveningstar-api ──host build──▶ openapi (v1.json)
                                   │
                                   ▼
                web-types (v1.d.ts, own lockfile)
                                   │
web/ + web-node-modules ──────────▶ web-ui (vite build → dist/)
                                   │
                                   ▼
                web-ui-bundle (inline → hash → gzip → budget)
                                   │ EVENINGSTAR_WEB_UI
                                   ▼
                    app (firmware, include_bytes!)
```

- `openapi` is built from the API crate and the other members' manifests only,
  so a firmware edit doesn't rebuild the document or anything after it.
- `web-types` turns the document into TypeScript declarations with
  `openapi-typescript`, which has its own lockfile and derivation. Only
  declarations come out of it, so a compromised generator can at worst produce
  wrong types. A short script of our own (`nix/web/scopes.mjs`, plain
  Node) writes the scope of each operation from the `security` requirement in
  the document, and fails on one the document doesn't define.
- `web-node-modules` installs `web/` from its lockfile with install scripts off.
- `web-ui` runs Vite with one chunk, no module preload, no source maps and every
  asset inlined.
- `web-ui-bundle` (`nix/web/inline.mjs`) makes one `index.html` with exactly one
  inline module script and one inline style, and fails on anything else: another
  script, a stylesheet link, an external URL, a build output left over. It
  writes the Content-Security-Policy with the `sha256` of each inline body,
  gzips the page with zopfli (without a timestamp), derives the `ETag` from the
  gzip, and fails if the gzip is over 64 KiB. The output is byte-identical
  across builds.
- `web-lint`, `web-typecheck` and `web-test` are sandboxed derivations too.
  `nix run .#checks` builds them in its `lint` and `test` groups, and its
  `build` group builds the bundle and hands it to cargo.

### Serving

`crates/eveningstar/build.rs` reads the bundle from `EVENINGSTAR_WEB_UI`. When
the variable isn't set it generates a placeholder page and a cargo warning, so
a plain `cargo run` and clippy need no Node. The `app` derivation sets the
variable; the dependency derivation doesn't, so a new page rebuilds no
dependencies.

`http/ui.rs` answers `GET` and `HEAD` on `/` with the gzipped page, whatever the
request accepts. `If-None-Match` answers `304` with no body. Every answer with
the page carries the policy and these headers: `ETag`, `Cache-Control:
no-cache`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
`X-Frame-Options: DENY`, `Cross-Origin-Opener-Policy` and
`Cross-Origin-Resource-Policy` (`same-origin`), and a `Permissions-Policy`
that turns off the camera, microphone, geolocation, USB and serial.

The policy admits only the page's own script and style by hash, connections to
its own origin, and `data:` images. It also forbids frames, forms to other
origins and a `<base>`, and requires Trusted Types with none allowed, so a
script can't write HTML.

### The client

The client lives in `web/src`. Dependencies point one way: `pages` use
`device`, `session`, `config` and `live`, which use `api`, which uses its
`transport`. `ui` holds presentational components and imports only `lib` and
itself. Side effects, fetching and timers, live in services and stores, never in
components, so nearly everything is plain TypeScript tested without a DOM, with
the clock, `fetch` and the browser injected.

The typed client takes each operation's method, path, request and response
shape from the generated types. Each operation also has a descriptor, and a
descriptor that disagrees with the document, or is missing, doesn't type-check.

The transport sends one request at a time, because the device has four HTTP
workers and eight sockets in all:

1. Requests wait in a queue by priority: a user's action, then a view's load,
   then a background poll.
2. Polls of one resource coalesce into one request.
3. A request without a body times out after 8 s, one with a body after 10 s,
   unless its operation says otherwise.
4. Only idempotent requests are retried, after a network error, a timeout, a
   503 or a 429 (honouring `Retry-After`), up to four attempts with
   exponential backoff and jitter. Polls aren't retried, as the poller backs
   off itself.
5. A non-idempotent request that fails without an answer has an unknown
   outcome. It is never retried, and the failure says so. Reconciling means
   reading state.
6. Leaving a page aborts its queued and running view loads.
7. Every result is a value, never a thrown error.

Failures are one union (`api/failure.ts`). A single function maps each kind to a
title, a likely cause and the actions that apply, and the details of the request
sit in an expandable section. A view loads all-or-nothing through `LoadGate`: it
shows once every request has succeeded, or says why not and retries transient
failures after a visible countdown.

Live values come from sources that poll while the tab is visible and in use,
every 5 s, backing off to 30 s while requests fail. Polling pauses after 30
minutes without input until the person resumes it. Each value shows its age, and
is dimmed and marked stale after three intervals without an update. A
connection state, `online`, `degraded` or `offline`, follows the outcome of
every request and is announced to assistive technology. An uptime that goes down
means the device restarted, and the values are fetched again.

A reboot is a state machine: the request, the pending reboot with its holds,
the device going away, and the first answer with an uptime lower than before. It
gives up after two minutes.

### Sessions, permissions and configuration

The API has no authentication, permissions or configuration endpoints yet. The
client has the seams they will use, and works without them.

- An `AuthProvider` decorates requests and says what to do on a 401: the
  transport pauses its queue, the router goes to `#/login`, and the queue
  resumes after a login. Only `NoAuth` exists. Credentials will live in memory,
  never in web storage.
- Each operation needs a scope: `read`, `operate` or `configure`. The API
  crate declares it on the `Endpoint`, and the OpenAPI document carries it in
  the operation's standard `security` requirement, `[{bearerAuth: [operate]}]`,
  beside a `bearerAuth` scheme and a `Scope` schema. Tools that read OpenAPI
  understand it, and it is what an OAuth-style token's `scope` claim holds, so
  a token's scopes and the operations line up whichever way the device
  later grants them. Roles can sit on top as named sets of scopes, such as a
  viewer with `read` and an operator with `read` and `operate`. `web-types`
  writes the table the client uses from the document, so an endpoint without a
  scope doesn't compile, and the page and the device can't disagree about what
  an operation needs. The session holds the granted scopes, and `Requires`
  disables or hides what the session can't use. This shapes the page; the
  device has no authentication yet, so it enforces no scope, and the document
  says so.
- Configuration editing follows JunOS: an exclusive lock held as a lease, a
  candidate configuration, a diff, a check, a commit that is one atomic request
  and can roll itself back unless confirmed, and a reconciliation for a commit
  whose answer was lost. The reducer, the controller that holds the lease, the
  diff, which never holds a secret, and the `ConfigService` interface are
  written and tested, and shown nowhere until the API exists. The candidate
  lives in memory only, and secret fields are write-only.

### The theme

Eventide is layered neutral greys with one blue for everything interactive, the
sun and moon colours for solar and battery data and never for status, and a 3 px
bar marking the selected item and what needs attention. Light and dark follow
the browser. The tokens are in `web/src/styles/tokens.css`, and a test checks
every text and graphic pairing in both themes against WCAG AA. Status is always
a shape and a word: off, standby, normal, caution, serious and critical each
have their own shape. There are no font files and no image files; the mark and
wordmark are inline SVG redrawn from the artwork in `web/brand`. The mark's star
is split by the day-night curve: its day half takes `--mark-day`, and its night
half, the orbit ring and its dots take `--mark-night`, so the mark is amber and
navy on light surfaces and amber and pale blue on dark ones. The header is dark
in both themes and sets the dark colours for the mark inside it. The favicon is
the star alone, without the ring, filling the icon: its day half amber and its
night half a bright blue, so both halves show on light and dark browser tabs. It
is an inline `data:` URL, so the browser makes no extra request.

### Dependencies

The runtime dependencies are `preact` and `@preact/signals`; the development
dependencies are `vite`, `typescript` and `@types/node`; and
`openapi-typescript` has a lockfile of its own. Everything else is written here:
routing, backoff, dialogs (the native `<dialog>`), formatting (`Intl`) and
diffing. Nothing needs a runtime schema validator, as the page and the API are
built together and the firmware ships the page.

npm is held to these rules:

- Versions are exact, and `package.json` pins the Node and npm of the dev shell,
  so an older npm that would ignore `min-release-age` refuses to run.
- Install scripts never run, and `.npmrc` also sets `allow-git=none`,
  `min-release-age=7` and `engine-strict`.
- Dependencies are installed only by Nix. A lockfile changes only through
  `--package-lock-only --ignore-scripts`, which resolves versions and runs no
  package code.
- `web-lint` checks that every lockfile entry is a tarball from the npm registry
  with a `sha512` integrity, that no package has an install script but the
  optional `fsevents`, and scans the sources for what the
  Content-Security-Policy and the dependency rules forbid.
- CI audits both lockfiles and verifies their signatures in a job with no
  secrets.

The client targets ES2022, sticks to web features that are widely available, and
works over plain HTTP and, later, HTTPS. Over HTTP the page isn't a secure
context, so it uses no secure-context API outside a check for one.

### Developing

The dev shell provides `web-dev`, `web-npm` and `web-types`. They run npm
packages in bubblewrap: the Nix store and the repository are read-only, `web/`
is the only writable part, the home directory is empty, the environment is
cleared, and the network is shared for the proxy to the device. `web-dev`
forwards `/api` to the device that `EVENINGSTAR_DEVICE` names, `web-npm`
changes only the manifest and lockfile of the tree it runs in, and `web-types`
builds the generated types again after the API changes. Without bubblewrap, as
on macOS, they refuse to run unless `EVENINGSTAR_UNSANDBOXED=1` is set.

Ubuntu 24.04 restricts unprivileged user namespaces through AppArmor, which
stops bubblewrap. Setting `kernel.apparmor_restrict_unprivileged_userns=0`, or
adding an AppArmor profile for `bwrap` that allows `userns`, lifts it.

A language server in an editor runs `typescript` from `node_modules`, a native
binary, on the host and unsandboxed. That risk is accepted.

## Storage

The ESP32-C6 module's internal flash (8 MiB) holds the bootloader, the
partition table, two app slots, and the data that outlives a boot. The
external NAND is separate and isn't covered here. The storage layer exists so
that code which needs a region can't also write the boot chain, the running
app or space the board gives to something else, and so that no byte leaves
the device unless a check covers it. The crate documentation has the details:
`chain.rs`, `nor.rs`, `roles.rs` and `state.rs` in `eveningstar-storage`, the
modules of `eveningstar-boot`, and `eveningstar-ota`.

### The crates

```
eveningstar-esp-formats   images, partition table, otadata: parsers and write planners
        ↑
eveningstar-storage       devices, roles, write state, handles, snapshots, publication,
        ↑                 layout, known bootloaders, verification chain, split
eveningstar-boot          content modules: bootloader, partition table, otadata, app slots
        ↑
eveningstar-ota           updates, activation, confirmation, rollback
        ↑
eveningstar               the internal flash's device, the app check task, the services
```

Each crate uses only the crates above it.

- `eveningstar-esp-formats` reads the formats from any `Source` and touches
  no flash. Its parsers read with buffers of at most 256 bytes on the stack
  and return an error rather than panic on any input. The otadata planners
  compute the erases and programs ESP-IDF's rules call for, and the caller
  carries them out. `ReadError::Stale` tells a read of a region written under
  it from a failed device read. It also defines the layout record an app image
  embeds (`LayoutRecord`) and reads one from an image (`app_layout`).
- `eveningstar-storage` owns the flash devices and splits them into region
  handles. It enforces bounds, protected spans, capabilities and write state,
  and reads no format beyond what the verification chain needs. It gives the
  compiled layout as a layout record (`LAYOUT_RECORD`). Its
  `test-support` feature gives the crates above it a NOR flash in RAM that
  takes power cuts and runs a hook on any call, and a simulated ESP-IDF
  bootloader run on it.
- `eveningstar-boot` turns readers into parsed models and published views. It
  holds no writer.
- `eveningstar-ota` holds the writers of the boot selection and reads the
  verification of the slots and the boot state from the content modules.

Nothing in these crates reboots.

### The layout

`nix/board.nix` declares every partition with its offset and size written out.
The layout can't change over OTA, as the partition table and bootloader aren't
updated from the firmware, so nothing is left for `gen_esp32part` to place. The
partitions are `factory` (4 KiB, read-only, reserved for durable factory data),
`otadata`, the app slots `ota_0` and `ota_1` (3 MiB each), a configuration
partition per slot (`config_0` and `config_1`, 128 KiB each), `diag` (256 KiB)
and `reserved`. A slot is sized for about twice the firmware expected once
WiFi and TLS are in, as slots can't grow later.

- Configuration is per slot, so after a rollback the old firmware finds its
  configuration as it left it, whatever the newer firmware wrote. The new
  firmware migrates from the alternate slot's partition after an update. A
  shared partition would undo that.
- `factory` and `reserved` are space that later firmware can claim only because
  the table already has it. `factory` has a role, so its span is known and
  protected, but no handle until a module owns it; `reserved` has no role and
  no handle.
- There is no `nvs` partition, as nothing uses that format.

`nix/_storage-layout.nix` turns the board into `layout.rs`, which is committed
so a plain `cargo build` needs no Nix. It rejects missing offsets, misaligned or
overlapping partitions, partitions outside the flash and slot pairs of unequal
size. The `lint` group fails when `layout.rs` is stale, and the `build` group
checks that the table ESP-IDF builds resolves to the same partitions, which
checks the numeric type, subtype and flag values against `gen_esp32part`.

Every app image embeds the layout it is built for, so that the firmware can
tell an image built for another layout from one built for this one. The layout
record sits right after the app descriptor, at offset 0x100 of the first
segment's data:

| Offset | Length       | Content                                                             |
| ------ | ------------ | ------------------------------------------------------------------- |
| 0x00   | 16           | the magic `EVENINGSTAR-LYT1`, which names version 1 of the format   |
| 0x10   | 4            | the partition table's offset, little-endian                         |
| 0x14   | 4            | the flash size in bytes, little-endian                              |
| 0x18   | 4            | the number of entries, little-endian                                |
| 0x1C   | 4            | zero                                                                |
| 0x20   | 32 per entry | the partitions in table order, each in the ESP-IDF table's encoding |

- The record is `eveningstar_storage::LAYOUT_RECORD`, built at compile time
  from `layout.rs`. `main.rs` links it into the section
  `.flash.appdesc.layout`. esp-hal 1.2.2's `rodata.x` collects `.flash.appdesc`
  and then `.flash.appdesc.*` into one output section at the start of the
  image, and the record's alignment of 1 leaves no gap after the descriptor's
  256 bytes. Because the record sits at a fixed offset, `app_layout` finds it
  from the image's header, the first segment's header and the descriptor's
  page size alone, with no symbol table, and reads only the record's header,
  as the entries are read on demand.
- `app_layout` gives no record when the first segment ends before the header
  or the entries do, the magic differs, the reserved bytes aren't zero, or the
  count exceeds the table's capacity (96 entries). An image compares equal
  to the compiled layout only when the table offset, the flash size, the entry
  count and every entry are equal byte for byte.
- `AppSlot::layout` (the `App` module) compares the record of a verified image
  with the compiled one, reading it again at the model's epoch like the
  descriptor, and says whether it is the compiled layout, another layout or
  missing.
- `Update::finish` and `Ota::verify_other` refuse an image whose record is
  missing or differs (`OtaError::Layout`). `roll_back` verifies the alternate
  slot with `verify_other`, so it refuses too. The layout can't change over
  OTA, and an image built for another layout would boot into the unverified
  tier, where the firmware runs on its default configuration without saving
  it, and could leave a remote board on that configuration. After an update
  from firmware that has no record, a `roll_back` to it is refused; the
  bootloader's own rollback doesn't read the record and isn't affected.

### Devices

A device is one flash chip. The storage crate has one trait and one module per
kind of flash; NOR (`nor`) is the only kind so far. A `NorDevice` reads at any
alignment, erases 4 KiB sectors and programs 4-byte words, and programming only
clears bits. Each call is atomic with respect to every other call on the same
device and is a compiler and hardware barrier. A power cut during an erase or
a program may leave any subset of that call's bits changed, and none of a later
call's. The trait has no associated constants, so the crate reaches the device
as a `&dyn NorDevice` and never names the firmware's type.

The crate calls a device only with ranges inside its capacity, aligned, and
erases at most one sector or programs at most 4 KiB per call, so no call keeps
interrupts off for longer than one sector erase. Long operations yield
between calls.

The firmware's `InternalFlash` (`storage.rs`) is the one
`esp_storage::FlashStorage`, behind a critical-section mutex: each call runs
with interrupts disabled, as a ROM flash call needs while code runs from the
flash it changes. It also reads the booted offset from MMU entry 0 of the
flash cache. `.clippy.toml` disallows `FlashStorage::new` everywhere else, so
the internal flash has no route around the handles.

### Regions, roles and handles

A region is any extent of a flash the firmware knows about; the bootloader and
the table are regions but not partitions. A handle is a `nor::Reader<'f, R>` or
a `nor::Writer<'f, R>`, where `R` is the region's role: an uninhabited marker
type that names its device and its class, publishable or sensitive. A role of
the compiled layout also has its size at compile time (`LayoutSize`), so a
store can assert its needs, and its offset when that doesn't depend on the slot
that booted (`Placed`). App slots and configurations are named by role,
current (the slot that booted) and alternate, with the slot (`ota_0`, `ota_1`)
reported beside them.

Each role below gives its span, its class and its handles.

- `Bootloader`: 0x0 up to the located table, or up to the compiled table
  offset when none is located. Publishable. Reader, every tier.
- `PartitionTable`: the compiled table's sector, 0x8000 to 0x9000.
  Publishable. Reader, every tier.
- `Otadata`: `otadata`. Publishable. Reader in the layout and boot tiers;
  writer in the boot tier.
- `CurrentApp`: the slot that booted. Publishable. Reader in the layout and
  boot tiers.
- `AlternateApp`: the other slot. Publishable. Reader in the layout and boot
  tiers; writer in the boot tier.
- `CurrentConfig`: the current slot's configuration. Sensitive. Writer in the
  layout and boot tiers.
- `AlternateConfig`: the alternate slot's configuration. Sensitive.
  Erase-only writer in the layout and boot tiers.
- `Diag`: `diag`. Sensitive. Writer in the layout and boot tiers.
- `Factory`: `factory`. Sensitive. No handle.
- `Chip`: the whole flash. Sensitive. No handle.
- `FoundTable`: the sector of the one table the search finds. Publishable.
  Reader in the unverified tier.
- `FoundApp`: the located table's app partition at the booted offset, or the
  image at the booted offset when no valid table is located. Publishable.
  Reader in the unverified tier.
- `FoundOtadata`: the located table's last `data`/`ota` partition. Sensitive.
  Reader in the unverified tier.

- Each role has at most one reader and one writer per boot. Neither is `Clone`
  or `Copy`, only the crate makes them, and `split` takes the device and its
  state table as `&'f mut`, so no handle outlives them or is made twice.
- A function takes the role it needs: a function of a `Writer<AlternateApp>`
  can't be given a configuration, the current slot or a region of another
  device. The `compile_fail` doctests in `nor.rs`, `roles.rs` and `chain.rs`
  show each rule.
- A writer reads its region at any time, as nothing else writes it. A
  sensitive role's owner therefore gets only the writer.
- No reader of a `Found*` role is made over a span that shares a byte with
  `factory`, a configuration or `diag` of the compiled layout; such a partition
  is reported only.

A writer erases and programs only inside a write session: `Writer::begin`
borrows the writer mutably and returns a `nor::Session`, so a region has one
session at a time. A session erases, and also programs for every role but
`AlternateConfig`, whose erase is the secure wipe, as NOR flash keeps no old
copies elsewhere. A session of a role that programs is an
`embedded-storage-async` `NorFlash` and `MultiwriteNorFlash` over its region,
for a store's own format. It erases one sector per device call and programs at
most 4 KiB per call, yielding after each.

Every erase and program checks its absolute range against the protected spans
before it reaches the device: everything below the first partition (the
bootloader and the table's sector), `factory`, `reserved` and the current slot,
from the compiled layout. A range that touches one is refused whole, so a
mistake in a caller's arithmetic still can't reach them.

### Write state and snapshots

Each physical span of the compiled layout (the bootloader's span up to the
table offset, the table's sector and each partition) has a write state: one
atomic word in the device's state table (`nor::State`, which the firmware
places in a `static` and `split` borrows). Only the `state` module encodes and
decodes the words. Everything else sees a `WriteState`: `Stable(epoch)` while
no write session is open, `Writing(epoch)`, the epoch it started from, while
one is.

The word is a sequence lock (seqlock), as used in the Linux kernel for data
read far more often than written. It is even while no session is open and odd
while one is, and its upper 31 bits count the sessions that issued a device
call. A session stores the odd word before its first device call. It counts as
written from just before its first device call: a call the guard refuses marks
nothing, and a call the device fails counts, as it may have changed bits.
Dropping the session, on any path (an error, a cancelled future, an abandoned
update), stores the next count when it was written, else the count it started
from. A session leaked rather than dropped leaves the region written to for the
rest of the boot.

A reader reads only through a snapshot. `Reader::snapshot` refuses (`Busy`)
while a session is open on the region, and (`OutOfRange`) a span outside it;
otherwise the snapshot holds the region's epoch. Every read of a snapshot loads
the state, reads the device and loads the state again, and fails `Stale`, with
its buffer zeroed, unless both loads see the region stable at the snapshot's
epoch. A snapshot is a `Source`, so the parsers read through it.

Reads and writes therefore run concurrently and safely. The loads and stores
are `SeqCst`, every device call is a critical section, which no load or store
moves across, and one core reaches the flash. A read whose two loads agree ran
outside every session that issued a device call on its spans, so its bytes are
those of one epoch, never a mix. Memory safety doesn't depend on this, as the
flash isn't Rust memory and the device serializes its calls. What the borrow
checker's shared-xor-mutable rule would guarantee at compile time, that bytes
don't change under a reader, is checked at runtime instead, and its failure is
an error, not wrong data.

The writer always wins. A session never waits for a reader; a reader of a
region being written fails and tries again later, and a busy writer can starve
readers. An update or a store's write is never held up by a download or a
check.

A region's epoch is the sum of its spans' counts together with which spans they
are, so epochs of regions over different spans never compare equal, and a
region over several spans (a `Found*` region across the layout's spans, or a
later export of the whole chip) has each of them checked. Counts only grow, so
two loads of the same spans that agree on the sum saw every span at the same
count. A counter that would wrap leaves its span written to for good: every
snapshot of it then fails rather than an epoch repeating. No boot runs that
many sessions.

`Reader::changed` completes once the region is stable at a state other than the
one it was in when `changed` was called: after a session on it ends at a new
epoch or, when a session was open at the call, after that session ends, even
without a write. Several tasks may wait on one device, the app check, OTA and
later the API among them. A device holds at most 16 waiters at once; with more,
they wake each other in turn until the region changes.

### Publication

Publication is an allow list. Bytes leave the device only through a published
view (`nor::Published`, behind the sealed `PublicRead` trait: its length, its
epoch, its sanitized spans and its reads). A byte is published only when three
rules hold. The storage crate enforces the first two, the content module the
third.

1. **Type.** Only readers of `Publishable` roles have `publish`. A located
   otadata, configuration, diag, factory or whole-chip handle has no such
   method, whatever the tier.
2. **Location.** Every publishable role is located from a known point: the
   bootloader at 0x0, where the ROM loads it; the table at the compiled offset,
   which a known bootloader reads, or the table the search finds; a partition
   of the located table; or the image at the booted offset, from MMU entry 0.
   The search is the one scan, and it is bounded: it looks only at sector
   boundaries below the booted offset, and a candidate counts only when it is a
   valid table with an app partition at the booted offset, and the only such
   table. Nothing else is located by scanning the flash for something that
   looks like a format.
3. **Verification.** A content module publishes only bytes its parser verified
   with a check that covers each of them:
   - An image, app or bootloader: its segments, its checksum and the SHA-256
     appended to it, which covers every byte before it. The view is the image,
     the digest included. An image without an appended SHA-256, which the ROM
     accepts on its checksum alone, isn't published, as an XOR checksum proves
     nothing about what the bytes are. The space after an image in its region
     is never published.
   - The partition table: the entries, the MD5 entry and the end entry, as the
     bootloader read them. Equality with the compiled layout covers the entries
     in the layout and boot tiers, and the table's MD5 entry covers them in the
     unverified tier. In the unverified tier, entries between the MD5 entry and
     the end entry, which the bootloader accepts but the MD5 doesn't cover, end
     the view after the MD5 entry. A table with no MD5 entry publishes no bytes
     and no labels; its partitions' types, subtypes, offsets, sizes and flags
     are still reported, as they say where regions lie, not what they hold.
     When the search finds several tables, only their offsets are reported.
   - The otadata, at its compiled offset: the 32-byte entry of each sector,
     when its parser pins down every byte of it. ESP-IDF's CRC covers the
     sequence number alone; the CRC is a function of it; the state has to be
     one of the values ESP-IDF defines; and the label, which the bootloader
     ignores and the firmware writes erased, is served as `0xFF` and listed as
     sanitized when it isn't erased. A published entry therefore reveals
     nothing beyond its parsed fields. A blank entry is published as such. An
     entry whose CRC doesn't match or whose state isn't a defined value has no
     view, and the rest of each 4 KiB sector is never published. The parsed
     fields, `OtadataFields`, give validity and, for a matching CRC, the
     sequence number, the slot it selects and a defined state; a located
     otadata gives those fields only.
   - Configuration, diag and factory data are never published raw. Their
     formats will give each record a strong integrity check, so a store can
     tell a record of its own from leftover data, and a store may publish
     sanitized fields of a record that verifies. A stored digest of secret data
     is never published, as it lets weak secrets be guessed offline; where a
     digest has to be visible outside the store, it is a MAC keyed by a device
     secret.

`publish` takes the epoch of the model the content module parsed, not a fresh
one, so a view never labels spans computed at one epoch with another, and it is
refused unless the region is stable at that epoch. A view's reads are checked
like a snapshot's and fail `Stale` once the region is written. Each content
module makes its views fresh from a model on each request.

As a second check, `publish` also refuses a span that overlaps a span the
compiled layout gives to sensitive data (`factory`, both configurations and
`diag`), in every tier. It catches a parser that verifies something it
shouldn't at the spans this firmware writes secrets to. It doesn't cover
secrets left elsewhere by another layout, which is why the allow rules are the
ones relied on.

**Sanitized bytes.** A view may serve bytes that lie between verified values,
which nothing reads or checks, as `0xFF`, the value of erased flash: in the
table, the MD5 entry's 14 bytes between its magic and its digest, and the end
entry's 28 bytes after its magic, type and subtype. `publish` reads those spans
once and records each whose flash bytes aren't all `0xFF` already, so a
download that differs from a dump of the flash says where and why; the list
reveals only that the bytes weren't erased, never their values. Unused space
that isn't between verified values, such as the space after an image, is cut,
not sanitized.

**Digests follow the bytes.** The integrity fields that formats store (an
image's SHA-256, the table's MD5, each otadata entry's CRC, the app
descriptor's ELF SHA-256) are parsed facts of a model, and a client verifies a
download with them. A transport digest is the SHA-256 of any range of a
published view as it is served, sanitized bytes as `0xFF`, computed on request
and returned with the epoch it was read at; it fails `Stale` like a read. No
digest the device publishes covers a sanitized byte's value on the flash, as a
digest over a few unknown bytes would give them away to anyone trying every
value, and no digest covers a range that couldn't be published. Sensitive
regions are hashed only inside the firmware.

A failure carries no value read from the flash: an image that doesn't verify
keeps its reason without the magic, chip ID, length or checksum it read
(`ImageFailure`), and a table the bootloader refuses keeps its reason without
its digests or entries (`TableFailure`).

Holding the reader is the permission to publish, so a content module never
hands out its reader or a snapshot; it hands out views and parsed models.

### Content modules

`eveningstar-boot` holds one content module per boot region. Each owns the
readers of its regions and offers parsed models, which only its parser makes
and which carry the epoch they were parsed at, and published views.

- `Bootloader` owns the bootloader's region.
  - Model: header (read whether or not the image verifies), length, stored
    digest, file SHA-256, known entry; the descriptor on request.
  - Published view: the image, once its SHA-256 verifies, known or not.
- `PartitionTable` owns the compiled table's sector, and the table the search
  found.
  - Model: the table as the bootloader judges it, the partitions it loads, the
    MD5 entry, the comparison with the layout, how it was located.
  - Published view: the covered bytes, the ignored ones sanitized.
- `Otadata` owns the otadata, the compiled layout's or the located table's.
  - Model: both entries, the active one, the boot state.
  - Published view: each entry its parser pins down (compiled only).
- `AppSlot` owns one app region: current, alternate or found.
  - Model: header, length, stored digest, file SHA-256, per epoch; the
    descriptor and the layout record's comparison on request.
  - Published view: the image, once verified at the region's epoch now.

- Every module is `Sync` and takes `&self`, so it lives in a `static` and is
  shared between tasks. Results that cost a hash are kept behind a
  critical-section mutex, held only to copy them in or out; the others are
  parsed again on each request.
- `PartitionTable` reports the compiled layout (`EXPECTED`) in the same shape as
  the found partitions, with the bootloader's and the table's regions, so
  clients compare them themselves. The comparison matches partitions by label:
  each expected one matches, differs in named fields or is missing, and any
  other is extra.
- `App` refers to the slots of the tier: current and alternate, or the found
  app. `App::run` verifies each slot (`AppSlot::check`), the current one first,
  then waits until a session on one of them ends at a new epoch, and verifies
  again. A check that a session interrupts keeps no result and runs again at
  the new epoch. `AppSlot::verdict` waits for a slot's result at a given epoch
  and fails `Stale` once the slot leaves it, so a slot is hashed once per epoch
  and OTA never hashes it itself.
- Long jobs take the caller's hasher (`Sha256Hasher`), pace (`Pace`) and
  progress hook (`Progress`): the hook is called and the pace run after each KiB
  hashed, and `Progress::idle` is called before each wait for a change only
  something else brings about.

### The verification chain and the tiers

`split` runs once per boot. It never fails: the worst outcome is the unverified
tier, in which the firmware runs on its default configuration without saving
it. Its `Report` records each step, the tier, and the step that kept the board
below the boot tier.

1. **Bootloader.** The header at 0x0 is read whether or not the image verifies:
   its flash size code is the chip size the bootloader checks the table against
   (an unknown code is 2 MiB, as in ESP-IDF). The image is verified (header,
   segments, checksum, appended SHA-256) at the length its headers describe,
   within the flash, and its stored digest is looked up in `known.rs`.
2. **Table offset.** A known bootloader's entry gives the offset it reads the
   table from. Otherwise, when the header reads and the booted offset lies
   inside the flash, the flash below the booted offset is searched, and only a
   single candidate counts. The bootloader's image has to end at or before the
   located offset, or the compiled one when none is located. One that doesn't
   is reported; its region ends at the table, so it doesn't verify within it
   and isn't published.
3. **Table.** The table at that offset is valid as the bootloader judges it
   (`esp_partition_table_verify` in ESP-IDF 5.5), and no more strictly:
   partition entries whose offset and end, in wrapping 32-bit arithmetic, lie
   within the chip size of step 1, at most one MD5 entry, whose digest matches
   the entries before it, and an end entry (magic, type and subtype `0xFF`)
   that isn't the first. The search judges each candidate the same way.
4. **Partitions.** The partitions are the ones the bootloader loads
   (`bootloader_utility_load_partition_table`): the entries up to the end
   entry, less one when there is an MD5 entry. With entries between the MD5
   entry and the end entry, they include the MD5 entry read as a partition and
   leave out the last entry, as the bootloader does, and such a table doesn't
   match the layout. Of several app partitions at the booted offset, or several
   `data`/`ota` partitions, the one the bootloader keeps is the one used.
5. **Layout.** The table is at the compiled offset, and its partitions equal
   the compiled layout's, field by field.
6. **Flash and booted slot.** The flash holds at least the layout, and the
   booted offset is a slot's. The current slot comes from the flash MMU, which
   says which page the CPU runs, not from the otadata, which says only what was
   selected.

- `Unverified`
  - Reached when: step 2 locates no single table, or any of steps 3 to 6
    fails.
  - Handles: readers of the bootloader, the compiled table's sector, and the
    `Found*` roles of what was located.
- `Layout`
  - Reached when: steps 2 to 6 pass, with the table found by the search or a
    known bootloader without rollback.
  - Handles: the unverified tier's, and readers of the slots and the otadata;
    writers of the configurations and `diag`.
- `Boot`
  - Reached when: steps 2 to 6 pass, with the table located by a known
    bootloader that supports rollback.
  - Handles: the layout tier's, and the boot writers: the alternate slot's and
    the otadata's.

- Matching the bootloader's judgement adds no failure from bit rot: a flipped
  bit the bootloader would reject stops the board before the firmware runs, and
  one it ignores is ignored here too. The table is never rewritten. The Nix
  table always has an MD5 entry, and the bootloader checks one whenever it is
  present.
- Image verification reads the app descriptor's MMU page size for the check of
  where mapped segments lie, as ESP-IDF 5.5 does on the ESP32-C6.
- `known.rs` lists the known bootloaders by the SHA-256 their image stores,
  each with the table offset it reads and whether it supports rollback. The
  offset is for the report and for refusing a table elsewhere: it never moves
  a region of the compiled layout. A known bootloader whose offset isn't the
  compiled one fails step 5, and its table has no reader. Only a bootloader
  known to support rollback is trusted with the boot selection, since an
  update under any other could leave a board starting a failing image for
  good. The list grows and doesn't shrink once boards have shipped, so no
  update takes a board out of the boot tier.
- A known bootloader is recognized by its exact digest, so the flash commands
  write it byte for byte. esptool rewrites the flash mode, frequency and size
  bytes of a bootloader's header when given values that differ from the
  image's, which is why the ESP-IDF bootloader's SHA-256 isn't checked at boot.
  `flash-boot` and `flash-all` pass no such options, so esptool keeps the
  header. A changed header shows as an unknown bootloader and the layout tier,
  not silently.
- The unverified tier has no writer: the layout the bootloader boots isn't
  known there, so the protected spans aren't either, and the firmware has
  nothing to save. Recovery is a reflash over USB (`flash-all`).
- No tier has a writer of the bootloader or the table, and there is no
  administrative write: a raw write would bypass the checks an update passes,
  and neither region can be rewritten safely across a power cut.
- The layout tier has no OTA, not even behind a warning: with a bootloader not
  known to roll back, an image that fails after an update keeps booting, and
  the board has no reset button.

A board flashed by `cargo run` gets the unverified tier: probe-rs writes
espflash's bootloader, which isn't known, and espflash's table, which the
search finds and which isn't the layout. The firmware then runs on its default
configuration and logs why.

### Updates and rollback

`Ota`, in `eveningstar-ota`, holds the boot writers, which only the boot tier
has, so it alone writes the alternate slot and the otadata. It reads the
verification of the slots from their `AppSlot`s and the boot state from the
`Otadata` module. The otadata format and selection rules are ESP-IDF's, so the
bootloader, built with app rollback, reads what the firmware writes. The
formats crate has its own otadata code because `esp-bootloader-esp-idf`'s fails
every operation when one entry is corrupt, can name an aborted image as
current, and activates in two writes, between which a power cut boots the new
image with no rollback.

- Each entry is judged on its own, so a corrupt sector never hides the other.
- An entry is written in a session of its own as an erase, then the label and
  state, then the CRC, then the sequence number last. A power cut during a NOR
  program can leave any subset of the bits programmed, so a single 32-byte
  program can tear into an entry that validates and boots the new image without
  arming the rollback. Until the sequence number is written the entry reads as
  blank, and a torn sequence number never matches the CRC, as the CRC is one to
  one on 32 bits.
- `Ota::new` waits for the current slot's check. OTA is ready once the running
  image verified and its descriptor reads, which gives the project name an
  update has to match; until then it refuses every write, `confirm` included,
  with the reason (`Unavailable`). A change of a slot's bytes that no session
  makes, such as flash decay, goes unseen until the next session on it or the
  next boot, at which the bootloader verifies the image it starts and falls
  back when it fails.
- `begin_update` first makes both entries select the running slot, cancelling
  a pending activation in place, so no valid entry selects the slot being
  rewritten. It then opens a session on the alternate slot, which the `Update`
  holds for its whole life, so no snapshot of the slot can be taken while an
  image is written; dropping the `Update` abandons it and ends the session.
  `Update::write` takes the image in parts of any size and erases each sector
  just before its first bytes are programmed.
- `Update::finish` programs the last word, ends the session at a new epoch of
  the slot, and waits for the slot's check to verify the slot at that epoch:
  chip, segments, checksum and the appended SHA-256. It then requires the image
  to be exactly the bytes written, its project name to be the running app's
  and its layout record to be the compiled layout's (`OtaError::Layout`), and
  returns a `VerifiedImage` of that epoch. `verify_other` makes the same
  checks of the image already in the alternate slot, apart from the length.
- `activate` takes only a `VerifiedImage` from the latest update, or from
  `verify_other` since; it borrows the flash, so it can't outlive its `split`.
  It refuses one whose slot's epoch has moved, and doesn't hash the slot again:
  the epoch proves no session wrote the slot since it verified, and the
  bootloader verifies the image at boot and falls back if it fails. Activation
  writes one `New` entry to the sector that isn't active, so a power cut leaves
  the old selection. A verified image is intact and meant for this board; it
  isn't known to be authentic.
- Any booted image that isn't `Valid` needs confirming
  (`BootState::needs_confirmation`): one booted `PendingVerify`, with no state,
  or as the bootloader's fallback when the selected slot failed to load.
  `Ota::confirm` makes it `Valid` in every case, writing a new entry for a
  fallback. `begin_update` and `activate` refuse (`Unconfirmed`) until the
  running image is `Valid`, and nothing writes `Valid` for an image that wasn't.
  `roll_back` marks the running image `Invalid`, once the alternate slot is a
  rollback target holding an image that `verify_other` accepts.
- `confirm` also waits for the self-checks (`SelfChecks`): a fixed set of
  checks, named by a type the firmware declares, each pending until its handle
  (`SelfCheck`) passes or fails it, and never changed after. A handle dropped
  without a verdict fails its check. While one is pending `confirm` refuses
  (`ChecksPending`), and once one has failed it refuses for the rest of the
  boot (`ChecksFailed`), writing nothing either way, so the confirmation
  window rolls the image back. A confirmation is never recorded to take effect
  later: a caller asks again once the checks have passed. Because a check that
  passed stays passed, the verdict `confirm` reads before its write still
  holds when the write is made. `roll_back` doesn't wait for the checks, and
  an image that is `Valid` already is confirmed whatever they say. `Ota`
  reads them through the sealed `ConfirmGate`, which only `SelfChecks`
  implements.
- A power cut at any point leaves a selection that boots the running image, or
  an activated image that verified, started once with its rollback armed.

The confirmation window (`storage/confirmation.rs`) is the firmware's, not
`Ota`'s, and runs in every tier. When the running image awaits a confirmation
(`eveningstar_boot::otadata::awaits_confirmation`), `storage::start` spawns a
task that requests a graceful reboot 10 minutes after boot
(`Reason::Unconfirmed`, which the API reports as `cannot_confirm`).

- An image awaits a confirmation when it has no boot state, or its boot state
  `needs_confirmation`: any state but `Valid`, a fallback, no otadata or one
  that doesn't read, the unverified tier without a located otadata (or with
  one but no known running slot), and a board started by `cargo run`.
- The task reads the boot state again when the window ends and reboots only
  if the image still awaits a confirmation, so a confirmation by any path
  cancels the reboot. A confirmed image at boot opens no window.
- A bootloader with app rollback turns a `PendingVerify` image into `Aborted`
  at the next boot and starts the previous image. With no rollback target, the
  same image starts again, and the board reboots every 10 minutes until the
  image is confirmed. That is accepted over a lockout.
- The task has its own watchdog handle, which sleeps through the window and
  bounds the read and the request after it; the handle retires when the task
  ends, and at boot when no window opens.
- `EVENINGSTAR_CONFIRM_WINDOW_S`, read when the firmware is built, shortens
  the window as a test aid; a value that isn't a decimal number above 0 fails
  the build.

The firmware builds `Ota` in a `static` last at boot, as it waits for the
running slot's check. Its self-checks are `storage::CHECKS`; the one check so
far, `Slots`, a placeholder that judges nothing about the running image, passes
once the alternate slot's check has kept a result, whatever the result, as a
blank slot is a board's state after `flash-all`, and fails when the slot is
written while it waits. The checks are logged once decided. Nothing takes `Ota`
yet, so nothing calls `Ota::confirm`: the API has no endpoint for it, and an
activated update rolls back after 10 minutes until one exists. The configuration
and diag writers are dropped at boot, as no store owns them yet. The OTA
service, not built yet, will decide how updates arrive and are authenticated.

### The app check task

`storage/check.rs` runs `App::run` over the tier's slots in the background of
everything else. It hashes with the SHA accelerator (`sha.rs`), whose
operations busy-wait on the peripheral, and pauses for 4 ms after every KiB
hashed: the Ethernet controller's driver yields after every SPI transaction, so
a job that holds the executor for milliseconds at a time takes it from the
network, and with the pause HTTP requests keep their pace while a slot is
checked. The task has its own watchdog handle, fed after every KiB hashed and
never on a timer, so a check that hangs while the rest of the system runs is
caught. The handle is paused while the checks wait for a slot to change, and
the results kept since the last wait are logged before each.

### Checks

- `check-images` (an example of the storage crate) runs the firmware's own code
  on files: the table judged as the bootloader judges it, with an MD5 entry and
  no entry after it, and compared with the layout by label; the bootloader
  verified and looked up in `known.rs`, with its entry's table offset and
  rollback; the app verified as an ESP32-C6 image of the `eveningstar` project,
  with the layout record right after its descriptor equal to the generated
  layout's in every byte; and the three, as `flash-all` writes them, taken
  through `split` to the boot tier. The `build` group runs it on the Nix
  bootloader and table and on the release app, and fails on a bootloader
  missing from `known.rs` (printing its entry), a table that isn't the layout,
  an app without the layout record or with one that isn't the generated
  layout's (printing which entries differ), or an app over 75% of its slot.
- The tests of the four storage crates read fixtures built by Nix and real
  captures in `crates/eveningstar-esp-formats/testdata/` (see its README). The
  `build` group fails when the Nix-built ones are stale.
- The snapshot and publication tests interleave tasks at device calls: the RAM
  device runs a hook on its n-th call that opens a session, writes and ends
  it. Every rule of the write state and of publication has a test, and the
  type rules have `compile_fail` doctests.
- The OTA tests cut the power after any erase or program, in each way a NOR
  program can be cut, and boot a simulated bootloader afterwards, from a first
  boot, a rollback target, a pending activation and a fallback. They also
  refuse images with no layout record, with one four bytes after the
  descriptor, with a first segment that ends inside it and with one of another
  layout, and take only the record right after the descriptor.

### On the board

On a dev board, after `flash-all`, the board reaches the boot tier: `split`
takes about 17 ms, loading the content modules 12 ms, and the checks of both
slots about 3.2 s in the background, with the SHA-256 digests computed on the
device equal to those computed on the host. The bootloader's first boot on
blank otadata writes entry 0 with sequence 1 and `Valid`
(`testdata/otadata-first-boot.bin`). Tables written at 0x8000 over USB give the
tier the bootloader's judgement and the layout call for, with readers of what
was located in the unverified tier; a table the bootloader refuses doesn't
boot, and rewriting the table over USB recovers the board. Bytes written after
an image in a slot over USB are never published.

A real update through `Ota`, copying the running image to the alternate slot,
was activated and then rolled back (booted `PendingVerify`, then its entry
`Aborted`) and, in a second run, confirmed, which lasts across reboots. A chip
reset in the middle of the copy leaves the board booting the running slot, and
the half-written slot fails its check on the checksum. An image in `ota_1` with
one byte flipped, selected as `New`, is refused by the bootloader, which starts
`ota_0`; the firmware logs that the bootloader fell back and that the boot
state needs confirming, and the alternate slot's check fails on the checksum.

The confirmation window was run with `EVENINGSTAR_CONFIRM_WINDOW_S=60`. On an
image confirmed `Valid`, the window's watchdog handle retires at boot. A `New`
entry booted as `PendingVerify` and rebooted after the 60 s; the bootloader
marked the entry `Aborted`. With no rollback target, the same image rebooted
once per window.

The bootloader's check of the image it starts, which activation and the
fallback above rely on, holds only without a debugger attached. The ESP-IDF
bootloader skips an image's checksum and SHA-256 while a debugger is attached
(`esp_cpu_dbgr_is_attached()` in `bootloader_support/src/esp_image_format.c`),
so with probe-rs attached during a boot it starts a corrupt image. The
firmware's own check still fails the image, and OTA refuses every write
(`Descriptor(Checksum)`). Board tests of image corruption attach the debugger
a few seconds after the reset; the RTT buffer keeps the early boot lines.

## Faults and reboots

The board has no reset button and runs unattended, so it must recover on its
own.

- **Faults reset the chip at once**, because the system's state can't be
  trusted. Faults are a panic, a supervised task missing its watchdog
  deadline, or a hung executor.
- **Everything else requests a reboot** through the reboot service, with a
  reason, such as the Ethernet link staying down or the running image staying
  unconfirmed. A graceful reboot waits for holds to be released, up to a
  deadline. An immediate one doesn't wait.

The reboot service and the watchdog supervisor live in `eveningstar-system`.
The firmware declares its reasons and holders (`services/reboot.rs`) and its
supervised tasks (`wd` in `main.rs`), and implements the chip's side: the
software reset (`reset.rs`) and the hardware watchdogs (`watchdog.rs`).

A crate with reboot reasons of its own requests graceful reboots through
`RebootRequest<S>`, an object-safe trait for its reason type `S`. The
firmware's reason type converts from `S` with `From`, and the reboot handles
implement the trait for every type their reason type converts from, so the
conversion is the firmware's `From` alone, and a `&dyn RebootRequest<S>`
names neither the firmware's reasons nor the service.

Only two places may reset the chip: the panic handler, and the firmware's
implementation of `eveningstar_system::Reset`, through which the reboot
service and the watchdog supervisor reset. `reset::take` hands that
implementation out once per boot, to those two services. Clippy's
`disallowed_methods`, configured in `.clippy.toml`, rejects the reset
functions everywhere else. Those two places `#[expect]` the lint, with a
reason.

The watchdogs come in three layers, each a backstop for the one above it: a
software supervisor over tasks, then the chip's main and RTC watchdogs. See
`eveningstar_system::watchdog` for what each layer catches, and the firmware's
`watchdog.rs` for the chip's watchdogs.

## Planned crates

- `morningstar`: the MeterBus and Modbus RTU protocol, the charge controllers'
  register maps and scaling. Pure `no_std`, tested on the host.
