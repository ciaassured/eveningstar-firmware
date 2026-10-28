# Web UI: implementation spec

Status: agreed design, not yet implemented. Written for the session that
implements it, in a worktree off the `web-ui` branch. When work lands, move
the decisions that stay true into `docs/architecture.md` ("The web UI") and
delete what this spec only needed for the build-out.

> **Security is a primary requirement, at runtime and during development.**
> The device runs unattended for years, often without updates, on networks
> its owner doesn't fully control. Every choice below that trades convenience
> for isolation, fewer dependencies or a stricter browser policy was made on
> purpose. Don't relax one without asking.

## 0. Before you start

- Read `CLAUDE.md` and `docs/architecture.md`. The web UI is a static client
  of the public API: no routes, form handlers or server-side rendering of its
  own. It depends on the generated OpenAPI document, never on the Rust crates.
- Repository rules that apply to every file you touch:
  - Comments describe the code. They never instruct or advise the reader.
  - A crate's or module's docs state what it guarantees, never what its
    users do with it.
  - Commit subjects: `<area>: <lowercase imperative summary>`. Areas in use:
    `firmware`, `bsp`, `docs`, `tooling`, `workspace`, `style`. Use `web` for
    the UI and `tooling` for Nix/CI. Bodies explain why, wrapped at 72
    columns, and end with the co-author trailer your attribution
    instructions give.
  - Lefthook runs `nix run .#checks -- lint` on pre-commit and `build test`
    on pre-push. Never bypass hooks with `--no-verify`.
  - Committing a subset of the working tree: the hook lints the real working
    tree, so wholly unstaged and untracked files are linted beside the staged
    ones. Back up changed and untracked files, run
    `git stash push --keep-index --include-untracked`, commit, restore, and
    compare against the backup.
- Out of scope: the storage endpoint (`storage` branch, unmerged), DNS
  rebinding protection, server-side auth, permissions and config locking.
  The client provides for the last three (sections 6–8) without inventing
  their API.

### Stop and ask the user when

1. Milestone 0 fails (Vite's native addons won't run in the Nix sandbox).
   Report what failed. Don't silently switch tools.
2. You want any npm package outside the allowlist (section 2.1).
3. You need any firmware or API change beyond serving the UI (section 4).
4. A security rule here blocks something and the obvious fix is to relax it.

## 1. Constraints the design follows

| # | Fact | Consequence |
|---|------|-------------|
| C1 | The HTTP server has 4 workers (`http.rs: WORKERS`), and the IP stack has 8 sockets in total, shared with everything else (`ethernet.rs: SOCKETS`). | Each tab keeps at most one API request in flight. A UI load costs as few requests as possible. |
| C2 | Keep-alive idle timeout is 1 s (`persistent_start_read_request`). | Polling slower than 1 s opens a new TCP connection per poll. A POST can race the server closing an idle connection. |
| C3 | Server read timeouts are 5 s (first request) and 3 s (request body). | Client timeouts sit above them: 8 s for GET, 10 s for anything with a body. |
| C4 | Browsers open up to 6 connections per host and request `/favicon.ico` on their own. | The client serialises its requests. The page declares its icon inline, as a `data:` SVG of the mark (12.2). |
| C5 | The device serves plain HTTP today. HTTPS is likely later, and the user will add it when a security feature needs it. | Over HTTP the page is not a secure context: no `crypto.subtle` (so no PKCE S256), no `crypto.randomUUID`, no service workers, no clipboard writes, no `Secure` cookies, and browsers don't offer brotli. The client must work unchanged on both: same-origin relative URLs only, no hard-coded scheme, and secure-context APIs used only behind `window.isSecureContext`. |
| C6 | Devices run for years without updates. Tabs may stay open for days. | Use only web features that are Baseline "widely available". Poll only while the tab is visible and in use. |
| C7 | The UI ships inside the firmware image and updates only with it, over OTA. | UI and API always match at load time. A firmware update under an open tab is the only skew case (section 5.6). |
| C8 | Networks may be flaky. | Every failure has a visible, specific reason, a retry path, and a freshness indicator on live data. |

## 2. Stack and dependency policy

- **Preact** with **@preact/signals**. Vite's built-in JSX transform,
  configured for Preact. No `@preact/preset-vite`: it adds about 72 packages
  (Babel, prefresh) for state-preserving hot reload, which we don't need.
- **TypeScript 7** (`tsc`, native) for type checking only. Vite strips types.
- **Typed API client**: our own small client over the types
  `openapi-typescript` generates. No `openapi-fetch`, because we need full
  control of queueing, timeouts, retries, auth and error mapping (section 5).
- **Tests**: Node's built-in runner (`node --test`) on `.ts` files with
  Node's type stripping. No Vitest, jsdom or testing libraries. Logic lives in
  plain modules so it can be tested without a DOM. Components stay thin.
- **No libraries for small things.** Dates and numbers use `Intl`. Diffing,
  formatting, backoff, routing and dialogs (native `<dialog>`) are written
  here. Icons are a handful of inline SVGs. There are no web fonts.

### 2.1 Allowlist

| Lockfile | Packages | Kind |
|----------|----------|------|
| `web/package-lock.json` | `preact`, `@preact/signals` | runtime (shipped) |
| `web/package-lock.json` | `vite`, `typescript` | dev |
| `tools/openapi-typescript/package-lock.json` | `openapi-typescript` | dev, its own tree |

Measured on 2026-10-28: the `web/` tree installs about 20 packages on
Linux x64 (Vite pulls rolldown, lightningcss and postcss). `openapi-typescript`
pulls about 33, which is why it lives in its own lockfile and derivation.
Adding anything else needs the user's approval, with the package's purpose,
transitive count, maintainers, install scripts and shipped size.

### 2.2 npm hardening

`web/.npmrc` and `tools/openapi-typescript/.npmrc`:

```
ignore-scripts=true
save-exact=true
allow-git=none
min-release-age=7
engine-strict=true
audit=true
fund=false
```

(`min-release-age` is in days. npm ≥ 11.19 supports it.)

- `package.json`: exact versions only. `engines.node` and `engines.npm` pin
  the dev shell's versions, so an older system npm that would ignore
  `min-release-age` refuses to run. No `preinstall`, `install`,
  `postinstall`, `prepare` or other lifecycle scripts. `"private": true`.
- Dependencies are installed only by Nix (`importNpmLock`). The host never
  runs `npm install` or `npm ci`. Lockfiles change only through
  `--package-lock-only --ignore-scripts`, which runs no package code
  (section 9).
- Until M6 adds `web-npm`, create and update lockfiles by running
  `npm install --package-lock-only --ignore-scripts` directly, with the
  dev shell's npm, from the directory holding the `.npmrc` (so
  `min-release-age` and `allow-git` apply). It only resolves versions and
  writes `package-lock.json`; it downloads package metadata, not package
  code, and runs nothing.
- Lockfile lint (a Nix check): lockfile version 3; every non-root entry has
  `resolved` starting with `https://registry.npmjs.org/` and a `sha512-`
  `integrity`; no `git+`, `file:`, `link:` or tarball URLs; no
  `hasInstallScript` except allowlisted optional platform packages
  (`fsevents`).
- CI (section 10): `npm audit --audit-level=low` and `npm audit signatures`.
  The latter needs an installed tree, so that job runs
  `npm ci --ignore-scripts` in an isolated job with no secrets.

## 3. Build chain (Nix)

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

### 3.1 `nix/openapi.nix`: narrow its source

Today its source is all of `crates/`, so every firmware edit rebuilds the
document and everything downstream. Narrow it to the root manifests,
`Cargo.lock`, `.cargo`, `crates/eveningstar-api` and stub copies of the other
members, using the same stub generation `nix/app.nix` uses (factor it into a
shared file instead of duplicating it). Give it a dependency derivation like
`app-deps`.

Acceptance: changing a file under `crates/eveningstar/` leaves
`nix eval --raw .#packages.x86_64-linux.openapi.drvPath` unchanged.

### 3.2 `nix/web.nix` (new)

Packages:

- `web-types`: `tools/openapi-typescript` installed with `importNpmLock`,
  then `openapi-typescript ${openapi}/v1.json -o $out/v1.d.ts`. No options
  that emit runtime code (`--enum` etc.). The output is declarations only, so
  a compromised generator can at worst produce wrong types.
- `web-node-modules`: `importNpmLock.buildNodeModules` for `web/`, with
  install scripts disabled. Prebuilt native binaries (rolldown, lightningcss,
  the TypeScript 7 binary) may need `autoPatchelfHook` to run in the sandbox;
  see Milestone 0.
- `web-ui`: source is `web/` only (a fileset). Links `web-node-modules`,
  copies `${web-types}/v1.d.ts` to `src/api/generated/v1.d.ts`, runs
  `vite build`. Output: `dist/`.
- `web-ui-bundle`: runs `nix/web/inline.mjs` (plain Node, no dependencies)
  on `dist/`, then:
  1. Produce one `index.html` with exactly one inline
     `<script type="module">` and one inline `<style>`, and no other
     `<script>`, `<link rel="stylesheet">`, `<link rel="modulepreload">` or
     external URL. Fail otherwise.
  2. Compute `sha256` (base64) of the exact bytes between the script tags,
     and between the style tags. Write the policy (section 4.2) to
     `$out/content-security-policy`, one line.
  3. Compress `index.html` to `$out/index.html.gz` with `zopfli`
     (deterministic, no timestamp or name).
  4. Write `$out/etag`: `"` + first 16 hex digits of sha256 of
     `index.html.gz` + `"`.
  5. Fail if `index.html.gz` is larger than 64 KiB. Write the sizes to
     `$out/report.txt`.

Checks (derivations, sandboxed):

- `web-typecheck`: `tsc --noEmit -p web`.
- `web-test`: `node --test` over `web/src/**/*.test.ts`.
- `web-lint`: the lockfile lint (section 2.2); `.npmrc` contents;
  `package.json` has no lifecycle scripts and exact versions; and a source
  scan of `web/src` (excluding `generated/`) rejecting `innerHTML`,
  `outerHTML`, `insertAdjacentHTML`, `document.write`,
  `dangerouslySetInnerHTML`, `eval(`, `new Function`, string arguments to
  `setTimeout`/`setInterval`, `javascript:`, `: any`, `as any`,
  `@ts-ignore`, `localStorage`/`sessionStorage` outside
  `src/lib/storage.ts` (if one ever exists), and imports from `src/api/`
  inside `src/ui/` (section 5.1). Scanning stays plain `grep`/Node: ESLint
  alone would add about 100 packages.

### 3.3 Vite configuration

- `build.modulePreload: false` (no polyfill inline script),
  `build.cssCodeSplit: false`, one JS chunk (no dynamic imports),
  `build.assetsInlineLimit` large enough that no asset file is emitted,
  `build.target: "es2022"`, no source maps in the bundle.
- `base: "/"`. `server.host: "127.0.0.1"`. `server.proxy` sends `/api` to
  `EVENINGSTAR_DEVICE` (for example `http://192.168.1.50`).

### 3.4 Wiring into the checks script and app

- `nix/checks.nix`: the shell script runs on the host, so it never runs npm
  code itself. Its `lint` group runs
  `nix build --no-link .#web-lint .#web-typecheck`. Its `test` group adds
  `.#web-test`. Its `build` group gets the real bundle with
  `nix build --no-link --print-out-paths .#web-ui-bundle` at run time (not
  interpolated into the script, so `lint` doesn't build the whole chain) and
  exports `EVENINGSTAR_WEB_UI` for `cargo build --release`.
- `nix/app.nix`: `packages.app` sets
  `EVENINGSTAR_WEB_UI = "${web-ui-bundle}"`. `app-deps` doesn't depend on it.

## 4. Firmware: serving the UI

This is the only firmware change in scope.

### 4.1 `crates/eveningstar/build.rs`

Std only, no new build dependencies.

- With `EVENINGSTAR_WEB_UI` set: read `content-security-policy` and `etag`,
  and generate `$OUT_DIR/web_ui.rs` holding `include_bytes!` of the absolute
  `index.html.gz` path, plus `&str` constants for the policy and the ETag,
  and `GZIP: bool = true`. Panic with a clear message if a file is missing.
  Emit `cargo:rerun-if-env-changed=EVENINGSTAR_WEB_UI` and
  `rerun-if-changed` for the three files.
- Without it: generate a placeholder (uncompressed HTML saying the UI wasn't
  built and how to build it, with `GZIP = false` and the policy
  `default-src 'none'`), and print a `cargo:warning`. Plain `cargo run` and
  clippy keep working without Node.

### 4.2 `crates/eveningstar/src/http/ui.rs`

Replace the `"/"` route in `http.rs` with a handler serving the bundle.
Only `GET /` (and `HEAD` if picoserve makes it free) is served. There are no
other UI paths: the UI routes with the URL fragment.

Response headers:

```
Content-Type: text/html; charset=utf-8
Content-Encoding: gzip                (when GZIP)
ETag: <etag>
Cache-Control: no-cache
Content-Security-Policy: default-src 'none'; script-src 'sha256-…';
  style-src 'sha256-…'; connect-src 'self'; img-src 'self' data:;
  base-uri 'none'; form-action 'self'; frame-ancestors 'none';
  require-trusted-types-for 'script'; trusted-types 'none'
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
X-Frame-Options: DENY
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Permissions-Policy: camera=(), microphone=(), geolocation=(), usb=(), serial=()
```

- `If-None-Match` matching the ETag answers `304` with no body. Use our ETag
  rather than picoserve's `File`, which hashes the body with SHA-1 at compile
  time, too slow for a body this size.
- gzip is served regardless of `Accept-Encoding`. Every browser in scope
  supports it.
- The UI never sends credentials in URLs, and the CSP blocks every
  cross-origin fetch, frame, form target and asset.

Acceptance: `curl -sI http://<device>/` shows the headers above. In the
browser, loading the page on the device logs no CSP or Trusted Types
violations (DevTools console). A repeat load answers `304`. Flash use grows
by roughly the bundle size.

## 5. Client architecture

### 5.1 Layout and dependency direction

```
web/
  .npmrc  package.json  package-lock.json  tsconfig.json  vite.config.ts  index.html
  src/
    main.tsx                 bootstrap: mount App, start services
    app/                     App shell, hash router, route table
    api/
      generated/v1.d.ts      (gitignored; from web-types)
      client.ts              typed request(operationId, …) over `paths`
      transport.ts           queue, timeouts, retry policy, abort
      failure.ts             ApiFailure union + user messages
    session/                 auth provider, session state, capabilities
    config/                  edit-session state machine, ConfigService interface, diff
    live/                    poller, connection state, freshness, activity tracking
    device/                  per-resource stores (system, reboot) built on api/
    pages/                   one file per route
    ui/                      presentational components: props in, events out
    lib/                     assertNever, backoff, Intl formatting, small helpers
    styles/                  tokens.css (custom properties), base.css
  **/*.test.ts               beside the module under test
```

Dependency direction: `pages → device/session/config/live → api → transport`.
`ui/` imports only `lib/` and its own siblings, never `api/`, `device/` or
`session/` (enforced by `web-lint`). Side effects (fetching, timers) live
only in services and stores, never in components.

TypeScript settings: `strict`, `noUncheckedIndexedAccess`,
`exactOptionalPropertyTypes`, `noImplicitReturns`,
`noFallthroughCasesInSwitch`, `verbatimModuleSyntax`, `erasableSyntaxOnly`
(no enums, namespaces or parameter properties, so `node --test` can strip
types), relative imports with `.ts` extensions (`allowImportingTsExtensions`
with `noEmit`), `jsx: "react-jsx"`, `jsxImportSource: "preact"`. Named
exports only. Switches over API enums are exhaustive with `assertNever`, and
handle the `unknown` variant the API reserves for values it adds later.

### 5.2 Typed client

- `request(op, params)` takes an `operationId` key of the generated
  `operations` (endpoint type names: `GetSystem`, `GetRebootStatus`,
  `Reboot`). It derives method, path, request body and response types from
  the generated `paths`, and resolves paths against `/api/v1`. Each
  operation has a static descriptor table typed
  `satisfies Record<keyof operations, OperationMeta>` with method, path,
  `idempotent`, timeout and required capability. A new endpoint in the API
  fails to type-check until it gets a descriptor, mirroring the exhaustive
  `match` in the firmware.
- JSON bodies always send `Content-Type: application/json`. Responses must
  be `application/json` (or empty for empty responses) and parse, or they
  become a `decode` failure. No runtime schema validation: the UI and API
  are built together (C7).

### 5.3 Transport rules (numbered for tests)

1. **One request in flight per tab.** A single queue with three priorities:
   user action > view load > background poll.
2. **Polls coalesce.** A poll for a resource that already has one queued or
   in flight is dropped, not queued.
3. **Timeouts** use `AbortController`: 8 s for requests without a body,
   10 s with one, overridable per operation.
4. **Only idempotent requests are retried automatically.** Retry on network
   error, timeout, 503, or 429 (honouring `Retry-After`). Up to 4 attempts
   with exponential backoff (0.5, 1, 2, 4 s, ±25 % jitter). Other 4xx and
   5xx are not retried.
5. **A failed non-idempotent request has an unknown outcome.** It is never
   retried automatically. The caller reconciles by reading state (5.7 for
   reboot, section 8 for commits) and tells the user what is known.
6. **Navigation aborts** queued and in-flight view loads of the page left.
7. **Every result is an `ApiFailure` or a typed value**, never a thrown
   error past the transport.

### 5.4 Failures and messages

```ts
type ApiFailure =
  | { kind: "network" }                         // fetch rejected
  | { kind: "timeout"; afterMs: number }
  | { kind: "busy"; retryAfterS?: number }      // 503 / 429
  | { kind: "unauthorized" }                    // 401
  | { kind: "forbidden" }                       // 403
  | { kind: "locked"; holder?: string; since?: string } // 409/423, future
  | { kind: "api"; status: number; code: ErrorCode }    // v1::Error body
  | { kind: "decode"; status: number }
  | { kind: "unknown-outcome"; cause: ApiFailure }      // non-idempotent
```

Each kind maps, in one exhaustive function, to a short title, a plain
explanation of the likely cause, and the available actions (retry, reload,
sign in). For example, `network`: "Can't reach the device. It may be
restarting, its network may be down, or it may be serving too many clients."
The `ErrorCode` values map to messages the same way. An expandable
"Details" section shows the method, path, status, code, attempt count and
time, for support and auditing. Failure text is rendered as text only.

### 5.5 Loading views all-or-nothing

- A `LoadGate` component takes a view's loaders. It runs them sequentially
  through the queue, renders its children only when all succeed, and
  otherwise shows the failure panel with the reason and a Retry button.
  Transient failures (rule 4) retry automatically, with a visible countdown.
  A view never renders with part of its data missing.
- Views keep to one request where the API allows. Section 11 proposes an
  aggregate status endpoint for the dashboard.

### 5.6 Live data, connection state and freshness

- `LiveSource<T>` interface: start, stop, a `value` signal, an `updatedAt`
  signal and an `error` signal. The first implementation polls through the
  queue. A server-sent events implementation can replace it later without
  touching pages; it would hold a socket per tab, so it is not the default
  (C1).
- Each source has an interval (the dashboard's: 5 s). After failures the
  interval backs off up to 30 s, and success restores it.
- Polling stops while `document.visibilityState` is `hidden`, and resumes
  with an immediate refresh. After 30 minutes without input, polling pauses
  behind a "Paused to save device connections" notice with a Resume button.
- A global connection state signal holds `online`, `degraded` (latest
  request failed, retrying) or `offline` (failing for over 15 s, or 3
  consecutive failures). It is shown in the header and announced through an
  `aria-live="polite"` region.
- Freshness: every live value shows its age. After 3 intervals without an
  update it is marked stale (dimmed, "updated 2 min ago"). Stale data is
  never presented as current.
- Device restart detection: `uptime_s` going down means the device
  restarted. Show a notice and reload every store. Section 11 proposes a
  firmware build identifier in `System`. Once it exists, a change from the
  build seen at page load prompts a page reload, because the UI in the tab
  is then older than the firmware.

### 5.7 Worked example: reboot

1. A confirmation dialog explains graceful vs immediate (`RebootMode`).
2. `POST /system/reboot`. A `202`, or a network failure (unknown outcome),
   both move on to step 3. Other failures show their message.
3. Poll `GET /system/reboot`: a pending reboot shows its reason, the
   `remaining_s` countdown and the holds.
4. Requests start failing: show "Restarting".
5. Poll `GET /system` until it answers with `uptime_s` lower than before the
   request: "Restarted". If nothing answers within 120 s: "The device hasn't
   come back after 2 minutes", with the likely causes.

A state machine in `device/reboot.ts` implements this. Tests drive it with
a fake transport.

## 6. Authentication (provision only)

The API has no auth yet. Build the seams without guessing the protocol.

- `AuthProvider` interface, used only by the transport:
  `decorate(request)` adds credentials; `onUnauthorized()` resolves to
  `"retry"` or `"login-required"`; `state` signal (`anonymous`,
  `authenticated { user, expiresAt? }`, `expired`); `login(input)` and
  `logout()`. The one implementation now is `NoAuth`.
- On `401` the transport pauses the queue and raises login-required. The
  router shows `#/login` and remembers the route. After login, queued
  idempotent requests resume. Non-idempotent ones fail with "Sign in again,
  then retry".
- Logout or a user change clears every device store, so no data from one
  session is shown in the next.
- Credentials live in memory only, never in `localStorage` or
  `sessionStorage`. Whether a session survives reloads gets decided with the
  server design. An `HttpOnly; SameSite=Strict` cookie is the likely choice;
  `Secure` is impossible over plain HTTP (C5).
- HTTPS is likely to come later (C5), and the user will add it when a
  security feature needs it. OAuth in the browser (PKCE needs
  `crypto.subtle`), `Secure` cookies, HSTS and keeping tokens off the LAN in
  clear all wait for it. Flag any feature that needs HTTPS to the user
  rather than working around its absence. Until then, the design assumes a
  device-local login issuing an opaque session token, and the auth seams
  must not need restructuring when HTTPS arrives.

## 7. Permissions (provision only)

- `Capability` is a string union defined in one place. It starts as
  `"read" | "operate" | "configure"`: read state; operational actions such as
  reboot or clearing live caches; editing the persistent configuration.
- The session holds the granted capabilities. With `NoAuth`, all are
  granted.
- Each operation's descriptor (5.2) names its required capability, so the
  UI gates every action from one table.
- `can(cap)` and a `<Requires cap="operate" mode="disable">` wrapper. A
  disabled control carries its reason ("Needs operator access"). Gating is
  UX only; the server enforces.
- Section 11 proposes declaring the capability on each `Endpoint` in the API
  crate and exporting it in the OpenAPI document, so the client table is
  generated rather than written.

## 8. Configuration edit mode (provision only)

Modelled on JunOS: operational mode vs configuration mode, an exclusive
lock (`configure exclusive`), a candidate configuration, a diff
(`show | compare`), validation (`commit check`), `commit confirmed` with
automatic rollback, and rollback.

- `ConfigService` interface (no implementation until the API exists):
  `running()`, `lock()`, `renew(lock)`, `unlock(lock)`,
  `check(candidate)`, `commit(lock, baseRevision, candidate, opts)`,
  `confirm(lock)`, `status()` (who holds the lock and since when).
- `EditSession`: a pure reducer `(state, event) → state`, plus a service
  running its effects. States: `operational`, `acquiring`,
  `locked-by-other { holder, since }`,
  `editing { lock, expiresAt, base, candidate, dirty }`, `checking`,
  `committing`, `awaiting-confirm { deadline }`, `lock-lost`,
  `unknown-outcome`.
- The lock is a lease renewed while editing. The banner shows the time
  left. A failed renewal warns. Losing the lock keeps the candidate in
  memory, shows that it can't be committed, and offers to re-lock if the
  running revision hasn't moved.
- A commit is one atomic request carrying the lock, the base revision and
  the full candidate. A network failure during commit is an unknown outcome:
  read the running revision to find out what happened.
- For commit confirmed, show the countdown and Confirm. If the change can
  move the device's address, show where to reconnect.
- The candidate is kept in memory only, since configuration may hold
  secrets. Leaving with unsaved edits asks first (route guard and
  `beforeunload`). Secret fields are write-only: the UI never displays a
  stored secret, only whether one is set.
- Operational actions stay available outside edit mode, subject to
  capability.
- Deliver the reducer, interfaces, diff helper and tests. Don't wire any of
  it into visible UI until the API exists.

## 9. Development environment

- The dev shell adds `nodejs` (the version `engines` pins) and three
  commands:
  - `web-dev`: runs `vite` inside bubblewrap. The Nix store and repository
    are read-only, `web/` is writable, `$HOME` is replaced by a tmpfs, the
    environment is cleared except an allowlist (`PATH`,
    `EVENINGSTAR_DEVICE`), and the network is kept for the device proxy.
    Before starting, it links `web/node_modules` to `web-node-modules` and
    `web/src/api/generated` to `web-types`. Both links are gitignored.
  - `web-npm <args>`: runs npm inside the same sandbox, with network, and
    always with `--package-lock-only --ignore-scripts`. Used to add, remove
    or update packages in `web/` or `tools/openapi-typescript/`.
  - `web-types`: rebuilds and relinks the generated types after an API
    change.
- On platforms without bubblewrap (macOS), `web-dev` and `web-npm` refuse to
  run unless `EVENINGSTAR_UNSANDBOXED=1` is set. Ubuntu 24.04 restricts
  unprivileged user namespaces through AppArmor; document the fix for
  developers there.
- Accepted risk, to document: an editor's TypeScript language server runs
  `node_modules/typescript` (a native binary) on the host, unsandboxed.

## 10. CI (`.github/workflows/rust_ci.yml`)

Fix these as part of this work:

- Top-level `permissions: contents: read`.
- Pin every action to a full commit SHA (with the version in a comment).
  Add Dependabot for `github-actions` and `npm` (both directories), with
  `cooldown` at least 7 days.
- `CACHIX_AUTH_TOKEN` only as the cachix step's `authToken` input, not in
  job `env`. Today `nix run .#checks` exposes it to every `build.rs` and
  proc macro. Skip pushing to the cache on `pull_request` events.
- Keep `pull_request` and never use `pull_request_target`.
- A separate `npm-audit` job with no secrets: `npm ci --ignore-scripts`,
  `npm audit --audit-level=low` and `npm audit signatures`, for both
  lockfiles.
- Reproducibility: after building `web-ui-bundle`, run
  `nix build --rebuild .#web-ui-bundle`.
- Any future self-hosted runner (hardware tests) never runs fork PRs.

## 11. API proposals (non-binding, for the user)

Not part of this work. Recorded so the client's seams line up with them:

- `System` gains a firmware build identifier (5.6).
- 503 with `Retry-After` when the server is out of workers, instead of
  stalled connections.
- An aggregate read-only status endpoint for the dashboard (5.5).
- JSON bodies require `Content-Type: application/json`, which blocks
  cross-site form posts.
- Each `Endpoint` declares its required capability, exported to OpenAPI as
  an extension (`x-eveningstar-capability`) (7).
- Session endpoints returning the user and capabilities (6).
- Configuration lock, check, commit, confirm and rollback endpoints with
  revisions (8).

## 12. UI shell and theme

### 12.1 Shell

- Status bar (top, dark in both themes): mark and wordmark, device name,
  connection state, mode badge (`Operational`, or `Editing · m:ss` once edit
  mode exists), account slot (empty until auth exists).
- Navigation rail on the left, turning into a row under the status bar
  when the viewport is narrower than 640 px: Dashboard, System. Routes `#/`,
  `#/system`, `#/login` (reserved), plus a not-found view.
- Dashboard: uptime (live, as a stand-in for solar data), pending reboot
  banner (live).
- System: identity (name, ID), uptime, reboot (5.7).
- Styling: CSS custom properties in `src/styles/tokens.css`, CSS modules for
  components (Vite built-in), layouts that work from 360 px wide.
- Accessibility: semantic elements, native `<button>` and `<dialog>`,
  labelled controls, visible focus, keyboard-only use, status never shown
  by colour alone, and `prefers-reduced-motion` respected.

### 12.2 Theme: Eventide

Chosen by the user from a demo of four directions. It blends two of them:
layered neutral greys, a single interactive blue and left-bar highlights,
with rounded corners, bold key figures and the brand's sun and moon data
colours. The colours are original picks; nothing is copied from another
design system. Carbon's layering and left-bar ideas were a guide only.

**Principles**

- Neutral, layered surfaces carry the layout. Panels have no borders; they
  stand off the page by surface colour.
- One blue (`--accent`) for everything interactive, and nothing else.
- Data colours: `--sun` for solar input and energy coming in, `--moon` for
  the battery and stored energy. Neither is ever used for status.
- A 3 px bar on the inside left edge marks the selected navigation item and
  anything that needs attention: alerts (in their status colour) and the
  edit-mode bar (in `--sun-fill`). Under 640 px the navigation bar moves to
  the bottom edge of the selected item.
- Key figures are bold (600) with tabular numerals, so live values don't
  shift as digits change.
- The one decoration is a 2 px "terminator" line under the status bar: a
  horizontal gradient `transparent → --moon (30 %) → --sun (72 %) →
  transparent` at 80 % opacity, from the logo's day–night curve.
- Light and dark follow `prefers-color-scheme`. Both are first-class.

**Colour tokens**

| Token | Light | Dark | Use |
|-------|-------|------|-----|
| `--bg` | `#f4f4f4` | `#161616` | page |
| `--surface` | `#ffffff` | `#262626` | panels, rail |
| `--surface-2` | `#ececec` | `#303030` | edit bar, nested blocks |
| `--border` | `#e0e0e0` | `#393939` | dividers, gauge tracks, button outlines |
| `--fg` | `#161616` | `#f4f4f4` | text |
| `--muted` | `#525252` | `#c6c6c6` | secondary text, labels |
| `--accent` | `#0f62fe` | `#78a9ff` | links, primary buttons, focus, selected nav bar |
| `--accent-fg` | `#ffffff` | `#161616` | text on `--accent` |
| `--sun` | `#a65a00` | `#f7ae55` | solar text and lines |
| `--sun-fill` | `#f59b2f` | `#f5a742` | solar gauges, area fills, edit bar |
| `--moon` | `#0072c3` | `#33b1ff` | battery gauges and text |
| `--header-bg` | `#161616` | `#101010` | status bar |
| `--header-fg` | `#f4f4f4` | `#f4f4f4` | status bar text |
| `--header-muted` | `#a8a8a8` | `#a8a8a8` | status bar secondary text |
| `--header-line` | `#393939` | `#393939` | status bar dividers |
| `--nav-active` | `#e8e8e8` | `#393939` | selected nav background |
| `--st-off` | `#8d8d8d` | `#8d8d8d` | status: off |
| `--st-standby` | `#0072c3` | `#4589ff` | status: standby |
| `--st-normal` | `#198038` | `#42be65` | status: normal |
| `--st-caution` | `#8e6a00` | `#f1c21b` | status: caution |
| `--st-serious` | `#ba4e00` | `#ff832b` | status: serious |
| `--st-critical` | `#da1e28` | `#fa4d56` | status: critical |

Check every text and status pairing for WCAG AA contrast against the
surface it sits on, in both themes, and adjust the value (not the role) if
one fails.

**Status scale.** Six levels, each a shape and a colour, always with a word
nearby: off (hollow circle), standby (half-filled circle), normal (filled
circle), caution (triangle), serious (diamond), critical (octagon). About
11 px, drawn with CSS `clip-path` or inline SVG. The idea of a fixed status
scale comes from Astro UXDS; nothing of Astro's is copied.

**Type.** No font files.

- UI: `system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue",
  Arial, sans-serif`.
- Monospace (IDs, technical details): `ui-monospace, SFMono-Regular, Menlo,
  Consolas, "Liberation Mono", monospace`.
- Base 14 px. Panel titles 0.875 rem / 600. Labels 0.8 rem / 400 in
  `--muted`. Key figures 2.3 rem / 600. Secondary figures 1.25 rem / 600.
  `font-variant-numeric: tabular-nums` on every figure.

**Shape and space.**

- Radii: panels and alerts 10 px; buttons, inputs and nav items 7 px; chips
  and the mode badge fully rounded.
- Spacing on a 4 px grid: panel padding 18 px, gaps between panels 14 px.
- Panel grid: `repeat(auto-fit, minmax(240px, 1fr))`; wide panels (charts)
  span two columns from 760 px.

**Components.**

- Connection indicator: a dot and a word. Online: filled `--st-normal`.
  Retrying: `--st-caution`, blinking (static under reduced motion).
  Offline: hollow `--st-critical`. Announced through `aria-live="polite"`.
- Gauges (inline SVG, drawn at runtime): solar power as a 270° arc, battery
  charge as a full ring, 6 px stroke, round caps, track in `--border`.
- Charts: line in `--sun` (1.8 px), area fill `--sun-fill` at 16 % opacity,
  grid lines in `--border`, labels in `--muted`, the latest point marked.
- Buttons: primary filled `--accent`; secondary `--surface` with a
  `--border` outline; disabled at 50 % opacity with the reason as text
  beside the label.
- Stale data (5.6): dimmed to 45 % opacity, its age turning
  `--st-caution`.

**Brand assets.** Inline SVG only, no image files.

- Mark: the four-point star with the day–night curve and broken orbit
  ring, redrawn as a few paths from `web/brand/mark.svg`, with the gradient
  from `--moon` to `--sun-fill`.
- Wordmark: Exo 2 (SIL OFL 1.1) as outlined paths, cleaned up from
  `web/brand/wordmark.svg`. No Exo 2 font file is shipped.
- `web/brand/` holds the source artwork, copied unchanged from the
  `eveningstar-hardware` repository (`assets/logo/`, AGPL-3.0 like this
  one). It is not shipped as files; the inline SVG in the UI is derived from
  it.
- Favicon: the mark as an inline `data:image/svg+xml` link (C4), so the
  browser makes no extra request.
- Mark and wordmark together stay under about 5 KB.

## 13. Milestones

Each milestone ends with commits that pass every check (`nix run .#checks`),
plus the acceptance criteria listed.

- **M0, sandbox spike (go/no-go).** A minimal Preact + Vite project builds
  with `importNpmLock` in the Nix sandbox, with install scripts off, and
  rolldown, lightningcss and `tsc` 7 run. Confirm that TypeScript 7 accepts
  the settings in 5.1 and that `node --test` runs a `.ts` test. If any of
  this fails, stop and report (section 0).
- **M1, Nix chain.** Narrowed `openapi` (3.1 acceptance), `web-types`,
  `web-node-modules`, `web-ui`, `web-ui-bundle` (3.2 steps and budget),
  `web-lint`, `web-typecheck`, `web-test`, the `.npmrc` files, and the
  checks script groups (3.4). Acceptance: `nix build .#web-ui-bundle` twice
  gives the same output; the CSP hashes match the inline bytes.
- **M2, firmware serving.** `build.rs`, `http/ui.rs`, `app.nix` wiring.
  Acceptance: section 4.
- **M3, client core.** Transport (5.3 rules each with a test), failures
  (5.4), `LoadGate`, live sources, connection state, freshness, visibility
  and idle pause, `NoAuth` and the auth seams, capabilities, router, shell.
- **M4, pages.** Dashboard, System, the reboot flow with its state machine
  tests.
- **M5, edit-mode groundwork.** Reducer, interfaces, diff, tests. No
  visible UI.
- **M6, development and CI.** `web-dev`, `web-npm`, `web-types` commands;
  CI changes (section 10).
- **M7, docs.** Update `docs/architecture.md` ("The web UI": chain,
  serving, transport rules, security model, dependency policy). Add to
  `CLAUDE.md`: security is a primary requirement for web work, the npm rules,
  the `web-*` commands, and that npm code never runs on the host outside the
  sandbox. Then delete the parts of this spec the docs now cover.
