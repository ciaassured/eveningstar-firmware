# Storage view: what is left

Status: implemented. How it is built and why is in `docs/architecture.md`
("Binary bodies", "Downloads", "Checks and the storage service", "Downloads of
public regions"), and the endpoints are documented in the OpenAPI document
(`nix build .#openapi`). This file keeps what those don't: the decisions made
with the user, the work that builds on the view, and what hasn't been verified.

> **The split between sensitive and public handles is what keeps secrets on
> the device.** Every byte the API sends, and every digest it reports, comes
> from a handle that implements `Read`. Don't add a route from a
> `SensitiveRead`-only handle to a response, and don't relax a trust tier or a
> protected region, without asking the user.

## Decided with the user

- The storage API only reads, and needs the `read` scope. Wiping comes later
  (below).
- `/storage` is an index of the board's storage. The external NAND is listed
  as `not_supported`; later firmware may present it as several resources
  under `/storage/external`.
- Regions are named by their role this boot (`app_current`, `app_alternate`,
  `config_current`, `config_alternate`), with the slot reported beside them.
  `factory` is sensitive.
- A public region downloads whole or by HTTP range, and the web UI resumes
  broken downloads.
- App images are verified and hashed in the background after boot, never on
  request; no region is hashed whole. A write makes results stale through the
  write generations in `InternalFlash`.
- Long jobs (checks, downloads, later uploads and wipes) each have a watchdog
  handle fed on progress, and yield often. The HTTP workers are left to
  picoserve's timeouts.
- The web UI verifies downloads only in a secure context, with
  `crypto.subtle`; there is no SHA-256 of its own. It queues downloads in an
  app-wide service and fetches each into memory before saving it.

## Later: wiping

Wiping is wanted, but each wipe belongs to the service that owns the region,
none of which exists yet, and a wipe can't be undone, so it waits for
enforced authentication and needs the `configure` scope:

| Region             | What a wipe means                                     | Owner          | Conditions and effects                                                                                                                                                  |
|--------------------|-------------------------------------------------------|----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `config_alternate` | A secure wipe of the previous firmware's settings     | Config store   | Only after the store has migrated from it. The handle already allows erasing and not programming (`OtherConfig`). A rollback then boots on defaults.                      |
| `app_alternate`    | Removing the rollback target, e.g. a vulnerable image | OTA service    | Through `Ota`, the only writer of the other slot: first both otadata entries select the running slot, so no pending activation points at an erased slot. Full tier, running image `Valid`. No rollback until the next update. |
| `config_current`   | A factory reset of the settings                       | Config service | Erase and reboot, as erasing under a running store leaves its state wrong. The same action as the board's button. Loses the Wi-Fi credentials, so the device may leave the network. |

Erasing a 3 MiB slot is 768 sector erases, about 35 s with interrupts off for
tens of milliseconds (up to about 400 ms) at a time. Each wipe is a long job
under its own watchdog handle, fed per sector and yielding between them.

## Left to build

- **Re-checks** on request, or periodically to catch flash going bad. Both
  would reuse the check task.
- **The OTA service's writes.** It is the first writer of a public region. The
  generations, the re-check after a write and the download cut were exercised
  only with temporary hooks; a real update should show `checking`, then the
  new image's digests, and cut a download of the slot in progress.
- **A boot identifier in `/storage/internal`**, or the entity tag of each
  region in `/storage/internal/regions`, would let the web UI plan a download
  without reading the storage view again first.

## Not verified yet

Verified on the dev board (full tier after `flash-all`, untrusted tier after
`cargo run`): every endpoint; both digests of the bootloader and both slots
against `esptool image-info` and `sha256sum`; downloads of every public region
byte for byte against `esptool read-flash`; ranges, `416`, resume with
`If-Range`, the tag changing across a reboot; `403`, `404`, `405`, `409` and
`503` with `Retry-After`; a stalled and a killed client; a temporary write
during a download cutting it short; injected hangs naming `storage_check` and
`storage_download`; HTTP latency during checks (about 12 ms) and downloads
(about 21 ms); and the web page served by the device in headless Chromium,
over HTTP and in a secure context through a local proxy, including a busy wait
across a route change.

Not verified:

- The contended case of the large-response mutex: the JSON responses fit the
  TCP buffers, so a stalled client never held it on the board.
- Stack use measured on the board; the frames are bounded by clippy's limit
  at compile time only.
- The web UI over HTTPS on the device, and Chrome's handling of downloads from
  a plain-HTTP page on a real desktop.
- The `Invalid` image path and the data tier on the board (host tests only).
