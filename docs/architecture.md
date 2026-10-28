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
