# Web UI: what is left

Status: implemented. How it is built and why is in `docs/architecture.md`
("The web UI"), and the rules for working on it are in `CLAUDE.md` ("Web UI").
This file keeps what those don't: the API changes the client is waiting for,
and what hasn't been verified.

> **Security is a primary requirement, at runtime and during development.**
> Don't relax a choice that trades convenience for isolation, fewer
> dependencies or a stricter browser policy without asking the user.

## Stop and ask the user when

1. You want any npm package outside the allowlist in `CLAUDE.md`.
2. You need any firmware or API change beyond serving the UI.
3. A security rule blocks something and the obvious fix is to relax it.

## Out of scope until the user asks

DNS rebinding protection, server-side authentication, permissions and
configuration locking. The client has seams for the last three (`AuthProvider`,
`Requires`, the edit session) and doesn't invent their API.

## API proposals (non-binding, for the user)

Recorded so the client's seams line up with them:

- `System` gains a firmware build identifier. The client could then reload its
  own page when the build it started with differs from the device's, as a
  firmware update under an open tab leaves the page older than the device.
- 503 with `Retry-After` when the server is out of workers, instead of stalled
  connections. The client already honours `Retry-After`.
- An aggregate read-only status endpoint for the dashboard, so a view costs one
  request instead of several.
- Session endpoints returning the user and capabilities.
- Configuration lock, check, commit, confirm and rollback endpoints with
  revisions, which the `ConfigService` interface in `web/src/config/service.ts`
  is shaped for.
- The OpenAPI document's enums are closed unions, though the API's docs say
  later versions may add values. The client reads unknown values as they came;
  an `unknown` member in the schemas would let the compiler check that.

## Not verified yet

- On a device: `curl -sI http://<device>/` shows the headers in `http/ui.rs`, a
  repeat load answers `304`, and the browser console logs no Content-Security-
  Policy or Trusted Types violations. What was tested is the same page served
  by a script with those headers and a fake of the API, in headless Chromium:
  no violations, a reboot followed from waiting through restarting (the first
  request that hung timing out after 8 s) to restarted, light and dark themes,
  and a 360 px width. The dev server (`web-dev`) also loaded it there.
- Flash use on the board. The build grows `.rodata` by about the size of the
  bundle.
- CI itself: the workflow passes `actionlint` and its commands ran in the
  sandbox, but it hasn't run on GitHub.
- The gauges and charts of the theme, which wait for the solar data.
