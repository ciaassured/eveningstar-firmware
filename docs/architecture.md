# Architecture

How the firmware is structured, and why. This covers the decisions that span
crates and modules. The details of each part are in its crate and module
documentation (`//!` comments), which this document links to by name.

## Crates

The workspace's crates live flat under `crates/`.

| Crate                 | Holds                                                     | Builds for        |
|-----------------------|-----------------------------------------------------------|-------------------|
| `eveningstar-board`   | Board support: the chip's pins and flash, named by role   | ESP32-C6          |
| `eveningstar-api`     | The API contract: endpoints, request and response types   | ESP32-C6 and host |
| `eveningstar-storage` | The internal flash: layout, formats, handles and OTA      | ESP32-C6 and host |
| `eveningstar`         | The firmware: services, transports, drivers and watchdogs | ESP32-C6          |

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

### `eveningstar-storage`

Everything about the internal flash that doesn't need the chip: the partition
layout, the formats on the flash, the checks of what the board holds, the typed
handles to the partitions and the OTA mechanism. It has no Espressif
dependencies and reaches the flash through its `Flash` trait, so its tests run
on the host against a RAM flash that can lose power at any write. It is listed
in `host_crates` in `nix/checks.nix`. See [Storage](#storage).

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
  path, success status, summary and description, the parameters of its path,
  request and response bodies (`Empty`, `Json<T>`, or `Bytes` for a response),
  and the errors the operation itself can answer with.
- A path segment written `{name}` is a path parameter. The endpoint's `Params`
  type parses it, usually a closed enum deserialised from the segment as sent;
  a segment that doesn't parse, or that names a value the crate doesn't know
  (an enum's `Unknown`), is a `404`. Endpoints without parameters have `()`.
- Error sets are types too: `Infallible` for none, or an enum made by the
  `errors!` macro, each of whose variants is an `ErrorCode`. A `Json` request
  body adds `BodyError` (400, 413 and 415) on its own.
- The `operations!` table in `v1.rs` lists every endpoint and builds
  `v1::Operation`, an enum with one variant per endpoint, and `v1::Call`,
  which holds each endpoint with its parsed path parameters.
  `Operation::find(method, path)` gives the call a request is for, or a
  `RouteError` (404 or 405).

### Serving the API

The firmware serves everything under `/api/v1` with one service, `http/api.rs`.
It calls `Operation::find`, then hands the call to its handler in an
exhaustive `match`. The generic `serve` function reads the request body, calls
the handler with the path's parameters and the body, and writes the response
with the endpoint's status code, or a JSON `v1::Error`.

The `match` is split by group (`dispatch_system`, `dispatch_storage`), as every
arm puts the request and the writer in the frame again, and one `match` over
every operation would exceed the stack frame limit. Each group's `match` still
names every operation, the other groups' as one arm, so an endpoint added to
the table without a handler fails to build.

A response too large for a frame, such as `/storage/internal`, is built in a
static behind an async mutex (`Shared<T>`), filled in place and serialised by
reference. The mutex is held while the response is written, so a client that
stops reading delays the next request for the same endpoint until the server's
write timeout ends the connection.

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
`http/api.rs`: `X-Content-Type-Options: nosniff` so a browser can't read the data
as HTML or script, `Cache-Control: no-store`, `Cross-Origin-Resource-Policy:
same-origin`, and a Content-Security-Policy of `default-src 'none';
frame-ancestors 'none'` with its older form `X-Frame-Options: DENY`, so
nothing in an answer runs or loads if someone opens one and it can't be framed.
These are the headers of OWASP's REST security cheat sheet that matter for JSON;
the headers that only concern pages that load and run things are left to the page
(see [The web UI](#serving)). A path that no route takes is answered with the
API's `404` in JSON, with the same headers, not the router's own plain text,
which repeats the path it was asked for.

### Binary bodies

A `Bytes` response is written from a `Source`: a length, an optional strong
entity tag, the filename it is saved under, and a read at an offset. The
handler returns the source and a buffer (`Download`), and the body is written a
buffer at a time, each chunk followed by a yield, so the headers can borrow
the source while the body streams. A read that fails ends the body before its
length and closes the connection, so the client sees a short body rather than
wrong bytes.

`Range` and `If-Range` are handled once, in `v1::Selection` (RFC 9110, pure
and tested on the host), and `http/api.rs` answers `200`, `206` or `416` for
every binary endpoint from it. One range is served; several, or one past the
end, is `416`; an `If-Range` that isn't the source's current strong tag gets
the whole body. Every binary answer carries `Accept-Ranges`, `Content-Length`,
the tag, and `Content-Disposition` with the filename limited to ASCII letters,
digits, `.`, `-` and `_`, so it can't end the header. A binary answer closes
its connection once written: a body cut short leaves nothing on the
connection that could be read as the rest of it.

### Adding an endpoint

1. Add its request and response types to `v1.rs`, or to the module of its
   group such as `v1/storage.rs`, deriving `JsonSchema` under the `schema`
   feature. Their doc comments become the schema descriptions.
2. Add a unit struct for the endpoint and implement `Endpoint` for it, including
   the `SCOPE` a session needs to call it and its path's `Params`.
3. Add the struct to the `operations!` table.
4. Write the handler in the module for its group, and add its arm to the
   group's `match` in `http/api.rs`. The firmware doesn't build until you do.

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
`device`, `session`, `config`, `live`, `storage` and `downloads`, which use
`api`, which uses its `transport`. `ui` holds presentational components and imports only `lib` and
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
8. A download of a binary body runs in a lane of its own beside the queue, one
   at a time, so polls keep flowing while it runs. It has a stall timeout (no
   bytes for 10 s) instead of a total one, is never retried by the transport,
   and streams its chunks to the caller. A body shorter than its
   `Content-Length` is a failure.

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

### Downloads

Downloads belong to an app-wide service (`downloads/`), not to the page that
started them, so they carry on across routes; the header's tray shows them.
The page fetches each download into memory and hands it to the browser when it
is complete, through an object URL and a `download` link. A plain link would
be simpler, but it can't carry credentials held in memory, and the page
couldn't check the bytes before saving them. The largest region is 3 MiB.

- The queue runs one download at a time, in order. A `503 busy`, which means
  another client holds the device's one download, waits for its `Retry-After`
  for as long as the entry stays queued, and is never a failure.
- A broken download resumes with `Range` and `If-Range` a few times, with
  backoff. A `200` in answer means the region changed, and the download starts
  again from a fresh plan. Each download is planned when it starts, from a
  fresh read of the storage view, as the regions' names follow the running
  slot.
- In a secure context the page hashes a download with `crypto.subtle` against
  the device's digest before saving it, and keeps a mismatch from being saved
  as good. Over plain HTTP, which isn't a secure context, it shows the digest
  to check by hand. The page has no SHA-256 of its own.
- Closing the tab loses the queue, so the page asks before unloading while it
  isn't empty.

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
sun and moon colours for solar and battery data and never for status, and a
3 px bar marking the selected item and what needs attention. Light and dark
follow the browser. The tokens are in `web/src/styles/tokens.css`, and a test
checks every text and graphic pairing in both themes against WCAG AA. Status is
always a shape and a word: off, standby, normal, caution, serious and critical
each have their own shape. There are no font files and no image files; the mark
and wordmark are inline SVG redrawn from the artwork in `web/brand`. The mark's
star is split by the day-night curve: its day half takes `--mark-day`, and its
night half, the orbit ring and its dots take `--mark-night`, so the mark is amber
and navy on light surfaces and amber and pale blue on dark ones. The header is
dark in both themes and sets the dark colours for the mark inside it.
The favicon is the star alone, without the ring, filling the icon: its day half
amber and its night half a bright blue, so both halves show on light and dark
browser tabs. It is an inline `data:` URL, so the browser makes no extra request.

### Dependencies

The runtime dependencies are `preact` and `@preact/signals`; the development
dependencies are `vite`, `typescript` and `@types/node`; and `openapi-typescript`
has a lockfile of its own. Everything else is written here: routing, backoff,
dialogs (the native `<dialog>`), formatting (`Intl`) and diffing. Nothing needs
a runtime schema validator, as the page and the API are built together and the
firmware ships the page.

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
  optional `fsevents`, and scans the sources for what the Content-Security-Policy
  and the dependency rules forbid.
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
partition table, two copies of the app, and the data that outlives a boot. The
external NAND is separate and isn't covered here. The storage layer exists so
that code which needs a partition can't also write the boot chain, the running
app or space the board gives to something else, and can't send secrets off the
device by passing the wrong handle. The crate's documentation (`lib.rs`,
`partitions.rs`, `otadata.rs`) has the details.

### The layout

`nix/board.nix` declares every partition with its offset and size written out.
The layout can't change over OTA, as the partition table and bootloader aren't
updated from the firmware, so nothing is left for `gen_esp32part` to place. The
partitions are `factory` (4 KiB, read-only, reserved for durable factory data),
`otadata`, the app slots `ota_0` and `ota_1` (3 MiB each), a `config` partition
per slot, `diag` and `reserved`. A slot is sized for about twice the firmware
expected once WiFi and TLS are in, as slots can't grow later.

- Configuration is per slot, so after a rollback the old firmware finds its
  configuration as it left it, whatever the newer firmware wrote. The new
  firmware migrates from the old slot's partition after an update. A shared
  partition would undo that.
- `factory` and `reserved` are space that later firmware can claim only because
  the table already has it. No handle exists for them.
- There is no `nvs` partition, as nothing uses that format.

`nix/_storage-layout.nix` turns the board into `layout.rs`, which is committed
so a plain `cargo build` needs no Nix. It rejects missing offsets, misaligned or
overlapping partitions, partitions outside the flash and slot pairs of unequal
size. The `lint` group fails when `layout.rs` is stale, and the `build` group
checks that the table ESP-IDF builds resolves to the same partitions, which
checks the numeric type, subtype and flag values against `gen_esp32part`.

### Trust tiers

`split` runs once per boot and returns the handles of the tier the board
reaches, with a `Report` of what it found. It never fails: the worst outcome is
the untrusted tier, so storage never stops the device from booting.

| Tier      | Reached when                                                                                                                                                          | Handles                                    |
|-----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------------|
| Untrusted | the table at 0x8000 isn't the layout, the flash is smaller, the app didn't boot from a slot, or the bootloader isn't known and another table could be the one it read | the boot chain, read only                  |
| Data      | otherwise                                                                                                                                                             | + config, diag, otadata and the other slot |
| Full      | + the bootloader is known and the running app's descriptor can be read                                                                                                | + `Ota`                                    |

The table is compared entry by entry with the layout. It is slightly more
lenient than the bootloader, which also requires the entry after the MD5 entry
to be erased, and the MD5 isn't checked. When the table at 0x8000 isn't the
layout, `split` looks for the one the bootloader used, so the boot-chain
handles describe what is on the board: a well-formed table below the running
app with an app entry at the booted offset. A table left over from an earlier
layout looks the same, so when two qualify the board's table is unknown and
there is no table handle. The same leftover can make 0x8000 look right while
the bootloader reads another table, so with a bootloader that isn't known,
`split` also searches, and any table besides the one at 0x8000 drops the board
to the untrusted tier. A known bootloader reads 0x8000, so with one nothing
more is searched. In the untrusted tier the running app's handle is its image,
cut to its entry in the table, so a wrong table can't widen it. The running
slot comes from the flash MMU, which says which page the CPU runs, not from the
otadata, which says only what was selected.

The full tier needs a bootloader in `known.rs`, identified by the SHA-256 its
image stores. Only a bootloader known to support rollback is trusted with the
boot selection, since an update under any other could leave a board that boots
nothing. The list grows and doesn't shrink once boards have shipped, so no
update takes a board out of the full tier.

A board flashed by `cargo run` gets the untrusted tier: probe-rs writes its own
bootloader and table. The firmware then runs on default configuration that it
doesn't save, and logs why.

### Handles

A handle is a `Partition<'f, R>`, with `R` an uninhabited role. What it can do
is a set of sealed traits it implements, so a misuse fails to compile (the
`compile_fail` doctests in `partitions.rs` show each).

- `SensitiveRead` is the read every handle has. `Read` adds permission for the
  contents to leave the device, and only public handles (the boot chain, the
  app slots, the otadata) have it. A function that exports data takes
  `impl Read` and so can't be given a configuration or diag handle; one that
  handles secrets takes `impl SensitiveRead` and accepts both.
- `Erasable` and `Writable` are separate. The other slot's configuration can be
  erased but not programmed: erasing is the secure wipe, as NOR flash keeps no
  old copies elsewhere.
- Public handles are `Copy`, so a download and the OTA code can read the same
  slot. Sensitive and writable handles are made once, by `split`, and holding
  one is the permission. `split` takes the flash as `&mut dyn Flash`, so a
  second call can't make a second copy.
- A role's size (`FixedSize`) comes from the layout and is a constant, so a
  store can assert its needs at compile time. The boot-chain spans are found at
  runtime and have none.
- Sensitive handles don't implement the NOR flash traits. `into_nor_flash`
  consumes a handle and returns an adapter for the store that owns the
  partition. Erase sets 4 KiB sectors to `0xFF`, and program only clears bits,
  with no read-modify-write that would erase what the caller didn't ask for.
- Every erase and program also checks its absolute range against the protected
  regions: the bootloader, the table's sector, `factory`, `reserved` and the
  running app. A mistake in the crate's own arithmetic still can't reach them.

A flash operation runs with interrupts disabled for up to a sector erase, so
no call erases more than a sector or programs more than 4 KiB, and long
operations yield between them. `.clippy.toml` disallows `FlashStorage::new`
everywhere but `storage.rs`, so the internal flash has no route around the
handles.

### Checks and the storage service

`services/storage.rs` holds what `split` found and the public handles, those
that implement `Read`; the sensitive handles never reach it. Its handle,
`StorageView`, is what the storage endpoints get.

After `split`, a task of its own verifies each app slot the firmware holds a
handle for (`verify_app_with`): the checksum and the digest the image stores,
and from the same pass the SHA-256 of the whole image, the one `sha256sum`
prints. The second digest costs nothing: the hasher is cloned before it
finishes, and the clone is fed the stored digest. Whole slots and other
regions aren't hashed. A slot that fails its check is the first sign of flash
going bad or of an aborted write.

- The check is a background job. It hashes with the chip's SHA accelerator and
  pauses for 4 ms after every 1 KiB, through the `Pace` the storage crate takes
  between steps. The Ethernet controller's driver yields after every SPI
  transaction, so a job that holds the executor for milliseconds at a time
  takes it from the network: checks in 4 KiB software steps made HTTP requests
  take about 650 ms instead of 10 ms; with the pause and the accelerator they
  take about 12 ms, and a slot takes about 1.2 s.
- The SHA accelerator (`sha.rs`) is shared through esp-hal's backend: each
  hash keeps its own context, and the backend runs one operation at a time.
  `split`'s bootloader verification stays in software; it runs once, over
  22 KiB.
- `InternalFlash` counts writes: a generation per partition, bumped before
  every erase and program. No writer has to say what it changed, and there is
  no other route to the flash. A check stores its result only if the slot's
  generation didn't move while it ran, and a result is current while its
  generation is the live one. After a write, the task checks the stale slots
  again once the flash has had no write for 3 s.
- The task has its own watchdog handle, fed on every read of a check, never on
  a timer, and idle while it waits for writes.

### Downloads of public regions

A public region leaves the device through the content endpoint, whole or by
range. Whether it may is decided from the region's name before any handle is
looked up: configuration, diagnostics and factory data never leave, as bytes or
as a digest, and an unknown region fails closed. The body reads through an enum
of the public handles whose every access takes `impl Read`, so a sensitive
handle can't be added to it.

- One download at a time. The lock holds the only download buffer (1 KiB, one
  TCP buffer) and the download's watchdog handle: taking it feeds the handle,
  every chunk feeds it, and releasing it, also when the client goes away, sets
  it idle. A client that stops reading is ended by the server's write timeout,
  well within the watchdog's. A whole 3 MiB slot takes about 9 s.
- Each chunk checks the region's generation. A write while it streams cuts the
  body short.
- The entity tag is a random value drawn at boot and the region's generation,
  so it changes whenever the bytes may have, and on every boot, as the regions'
  names then follow the other slot.

### Updates and rollback

`Ota`, in the full tier, is the only code that writes the other app slot and
the otadata, so the typestate API is the only public way to change the boot
selection. The otadata format and selection rules are ESP-IDF's, so the
bootloader, built with app rollback, reads what the firmware writes. The crate
has its own otadata code because `esp-bootloader-esp-idf`'s fails every
operation when one entry is corrupt, can name an aborted image as current, and
activates in two writes, between which a power cut boots the new image with no
rollback.

- Each entry is judged on its own, so a corrupt sector never hides the other.
- An entry is written as an erase, then label and state, then the CRC, then the
  sequence number last. A power cut during a NOR program can leave any subset
  of the bits programmed, so a single 32-byte program can tear into an entry
  that validates and boots the new image without arming the rollback. Until the
  sequence number is written the entry reads as blank, and a torn sequence
  number never matches the CRC, as the CRC is one to one on 32 bits.
- `begin_update` first makes both entries select the running slot, cancelling a
  pending activation in place, so no valid entry selects the slot being
  rewritten. `Update::write` takes the image in parts of any size, and
  `finish` verifies it as the bootloader will: chip, segments, checksum, the
  required SHA-256, a project name equal to the running app's, and exactly the
  image's length.
- `finish` and `verify_other` return a `VerifiedImage`, the only thing
  `activate` takes. It comes from the latest update, borrows the flash so it
  can't outlive its `split`, and `activate` hashes the slot again. Activation
  writes one `New` entry to the sector that isn't active, so a power cut leaves
  the old selection. It means the image is intact and meant for this board; it
  doesn't mean authentic.
- Any booted image that isn't `Valid` needs confirming
  (`BootState::needs_confirmation`): one booted `PendingVerify`, with no state,
  or as the bootloader's fallback when the selected slot failed to load.
  `Ota::confirm` makes it `Valid` in every case, writing a new entry for a
  fallback. `begin_update` and `activate` refuse (`Unconfirmed`) until the
  running image is `Valid`, and never write `Valid` for an image that wasn't.
- Nothing in storage reboots. Below the full tier, `storage.rs` requests a
  graceful reboot for a `PendingVerify` image, which can't be confirmed there,
  so the bootloader rolls it back.

The OTA service, not built yet, will give a user 10 minutes after boot to
confirm an image that needs it. On timeout it requests a graceful reboot in
every case, including when no rollback target exists. A board then reboots every
10 minutes until someone confirms, which is accepted so that a user can confirm
whichever image comes up.

### Checks

- `check-images` (an example of the crate) runs the firmware's own table
  comparison, image verification and `split` on files. The `build` group runs it
  on the Nix bootloader and table and on the release app, and fails on a
  bootloader missing from `known.rs` (printing its entry), a table that isn't
  the layout, or an app over 75% of its slot.
- The crate's tests read fixtures built by Nix and real captures in
  `crates/eveningstar-storage/testdata/` (see its README). The `build` group
  fails when the Nix-built ones are stale.
- The OTA tests cut the power after any erase or program, in each way a NOR
  program can be cut, and boot a simulated bootloader afterwards, from a first
  boot, a rollback target, a pending activation and a fallback.

At boot, `storage.rs` reads the booted offset, wraps `esp_storage::FlashStorage`
behind a critical-section mutex as the `Flash`, and logs the tier, the table,
the bootloader, the running slot and the boot state. The API reports the same
under `/storage/internal`.

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

A supervised task feeds its handle on progress, never on a timer, so a task
that hangs while the rest of the system runs is still caught. A task that
waits for work for a long time, such as the storage checks between writes or
a download between requests, sets its handle idle: its deadline isn't checked
until its next feed. The HTTP workers aren't supervised; their connections are
left to the server's timeouts.

## Planned crates

- `morningstar`: the MeterBus and Modbus RTU protocol, the charge controllers'
  register maps and scaling. Pure `no_std`, tested on the host.
