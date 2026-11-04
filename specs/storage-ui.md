# Storage view: API and web UI

Status: proposal, under discussion. Nothing here is built yet.

A read-only view of the device's storage: the API that describes the internal
flash and lets a client download its public regions, whole or in part, and the
web UI page that shows it. How the storage layer works is in
`docs/architecture.md` ("Storage"); this builds on it and changes none of its
rules.

> **The split between sensitive and public handles is what keeps secrets on
> the device.** Every byte the API sends, and every digest it reports, comes
> from a handle that implements `Read`. Don't add a route from a
> `SensitiveRead`-only handle to a response, and don't relax a trust tier or a
> protected region, without asking the user.

## Decided with the user

- Read only. No endpoint writes, erases, confirms or rolls back anything.
  Wiping a region is wanted later, from the service that owns it and once
  authentication is enforced (see "Later: wiping").
- The web UI queues downloads and runs one at a time, waiting while another
  client downloads. Downloads are fetched into the page and saved when they
  finish, by an app-wide download service, so they carry on across the
  page's routes.
- Reading non-sensitive storage needs only the `read` scope, the lowest one.
  The device enforces no scope yet; authentication comes later and covers
  these endpoints like every other.
- `/storage` is an index of the board's storage. The internal flash and the
  external NAND are separate resources under it. The external NAND is on the
  board but not supported by the firmware, and the index says so. Later
  firmware may present the NAND as several resources (logs, files), which the
  index leaves room for.
- Apps and configurations are named by role, `current` and `alternate`, not
  by slot, so a client can name the running app when the slot is unknown.
  The slot (`ota_0`, `ota_1`) is reported beside the name.
- A public region downloads whole by default, or any byte range of it, so a
  client can download only an app's image and resume a broken download.
- `factory` is sensitive, though nothing reads it yet.
- Digests are computed at boot, in the background, never on request: each
  app image is verified and hashed, not the whole slot. No region is hashed
  whole. A write to a region makes its results stale through a write counter
  at the flash itself (see "Invalidation"). A re-check on request, or a
  periodic one to catch failing flash, is left for later.
- The page verifies downloads only in a secure context, with `crypto.subtle`,
  which it will have once it is served over HTTPS. There is no SHA-256 of our
  own in the client.
- Error codes: `403 sensitive`, `409 unavailable`, `503 busy`, plus `416`
  for a range outside the region.

## API

Every endpoint is a `GET` with the `read` scope.

| Path                                             | Answers with                                                        |
|--------------------------------------------------|---------------------------------------------------------------------|
| `/storage`                                       | The board's storage devices and whether this firmware supports each |
| `/storage/internal`                              | The internal flash: tier, bootloader, boot state, both app slots     |
| `/storage/internal/regions`                      | Every region: layout, what the board holds, contents, roles, digests |
| `/storage/internal/regions/{region}/content`     | The region's bytes, or a range of them                              |

A NAND holding bulk logs shares nothing with a NOR flash holding a partition
table but the word "storage", so the external NAND will get its own paths and
types under `/storage/external`.

### Region names

`{region}` is a closed enum, one name per thing a handle can cover:

| Region             | Covers                                              | Contents  |
|--------------------|-----------------------------------------------------|-----------|
| `bootloader`       | 0x0 up to the partition table                       | public    |
| `partition_table`  | The table the bootloader read                       | public    |
| `otadata`          | `otadata`                                           | public    |
| `app_current`      | The running app: its slot, or its image when the app didn't boot from a slot | public |
| `app_alternate`    | The slot that isn't running                         | public    |
| `config_current`   | The running slot's `config_*`                       | sensitive |
| `config_alternate` | The other slot's `config_*`                         | sensitive |
| `diag`             | `diag`                                              | sensitive |
| `factory`          | `factory`                                           | sensitive |
| `reserved`         | `reserved`                                          | reserved  |

The current and alternate names swap meaning when the device boots the other
slot. A download resumed across a reboot is caught by its `If-Range` (below).

### `GET /storage`

```json
{
  "devices": [
    { "id": "internal", "kind": "nor_flash", "status": "available", "capacity": 8388608 },
    { "id": "external", "kind": "nand_flash", "status": "not_supported", "capacity": null }
  ]
}
```

`status` is `available` or `not_supported` (on the board, no driver in this
firmware). Later values could be `absent` or `faulty`.

### `GET /storage/internal`

What `split` and the boot-time checks found, and what it means for the
device.

```json
{
  "capacity": 8388608,
  "layout_size": 8388608,
  "tier": "full",
  "limit": null,
  "persistent_config": true,
  "updates": true,
  "booted_offset": 65536,
  "table": { "offset": 32768, "matches": true, "md5_checked": false },
  "bootloader": {
    "status": "known",
    "name": "esp-idf-v5.5.1-rollback",
    "digest": "3f1c…",
    "file_sha256": "8d02…",
    "size": 22528,
    "version": 1,
    "idf_version": "v5.5.1",
    "built": "Oct 30 2026 12:00:00",
    "flash": { "mode": "dio", "speed_mhz": 80, "size": 8388608 }
  },
  "boot": {
    "selected": "ota_0",
    "state": "valid",
    "rollback_target": false,
    "needs_confirmation": false
  },
  "otadata": [
    { "sector": 0, "validity": "valid", "seq": 3, "state": "valid", "selects": "ota_0" },
    { "sector": 1, "validity": "blank" }
  ],
  "apps": [
    {
      "region": "app_current",
      "slot": "ota_0",
      "offset": 65536,
      "size": 3145728,
      "image": {
        "status": "valid",
        "checked_uptime_s": 3,
        "size": 1203456,
        "digest": "c41f…",
        "file_sha256": "07aa…",
        "project_name": "eveningstar",
        "version": "0.1.0",
        "date": "Oct 31 2026",
        "time": "10:12:44",
        "idf_version": "esp-hal",
        "secure_version": 0,
        "elf_sha256": "9a0b…"
      }
    },
    {
      "region": "app_alternate",
      "slot": "ota_1",
      "offset": 3211264,
      "size": 3145728,
      "image": { "status": "empty" }
    }
  ],
  "checks": { "status": "done", "finished_uptime_s": 4 }
}
```

- `tier` is `untrusted`, `data` or `full`, and `limit` is the `Limit` that
  stopped `split` below full (`table`, `flash_size`, `not_a_slot`,
  `other_table`, `bootloader_invalid`, `bootloader_unknown`, `app_descriptor`),
  or `null`. `persistent_config` and `updates` say what the tier allows, so a
  client doesn't derive behaviour from tier names.
- `bootloader.status` is `known`, `unknown` (verifies, digest not in
  `known.rs`) or `invalid` (with an `error` code). The descriptor fields are
  `null` for a bootloader without a descriptor (ESP-IDF before 5.3).
- `boot` is `null` in the untrusted tier, where the otadata isn't read.
  `state` and `selected` are `null` when the otadata has no valid entry.
- `apps` lists an app only when the firmware holds a handle for it: both in
  the data and full tiers, only `app_current` in the untrusted tier, where
  its `slot` is `null` when it didn't boot from one.
- `image.status` is `checking` until the check of that slot has run, and
  again after a write to the slot makes it stale; then `valid` (with the
  digests), `empty` (erased) or `invalid` (with an `error` code).
  `checked_uptime_s` says when the result was taken. The descriptor fields
  are read without hashing, so they are present while `checking` too.
- Two digests per image, from one pass over it:
  - `digest`: the SHA-256 the image stores, of every byte before it. The
    "Validation Hash" of `esptool image_info`.
  - `file_sha256`: the SHA-256 of the whole image, the stored digest
    included. What `sha256sum` prints for a download of the image alone.
- `checks` is the background check: `running` or `done`.
- Digests are lowercase hex SHA-256. Text the flash holds (labels, versions)
  is a string when it is UTF-8 and printable, else `null` with the bytes in a
  `*_hex` field beside it.

### `GET /storage/internal/regions`

Every region in offset order: the bootloader, the table, then the layout's
partitions.

```json
{
  "regions": [
    {
      "region": "bootloader",
      "label": null,
      "offset": 0,
      "size": 32768,
      "contents": "public",
      "available": true,
      "roles": []
    },
    {
      "region": "app_current",
      "label": "ota_0",
      "type": 0,
      "subtype": 16,
      "type_name": "app",
      "subtype_name": "ota_0",
      "offset": 65536,
      "size": 3145728,
      "read_only": false,
      "contents": "public",
      "available": true,
      "layout": { "status": "matches" },
      "roles": ["running", "selected"]
    },
    {
      "region": "config_current",
      "label": "config_0",
      "contents": "sensitive",
      "available": true,
      "layout": { "status": "differs", "fields": ["offset", "size"] },
      "roles": []
    }
  ],
  "board_table": null
}
```

- `contents` is a property of the region, not of the tier:
  - `public`: may leave the device. The handles that implement `Read`.
  - `sensitive`: never leaves the device, as bytes or as a digest.
  - `reserved`: no handle exists, and nothing is in it yet.
- `available` is whether the firmware holds a handle for it this boot. A
  region is downloadable when it is `public` and `available`.
- No region is hashed whole. The digests of what a region holds are in
  `/storage/internal`: the bootloader's and each app image's.
- `layout` is the entry-by-entry comparison of the table at 0x8000 with the
  layout (`matches`, `differs` with the fields, `missing`). The bootloader and
  table have none.
- `roles`: `running`, `selected` (for the next boot), `rollback_target`.
- `board_table` is `null` when the table at 0x8000 matches the layout.
  Otherwise it holds what the board has, so the client can show expected
  against found: `{ "offset", "status": "found" | "several" | "none",
  "entries": [...], "extra": n, "end": "md5" | "erased" | <error> }`, the
  entries bounded by a constant (`BOARD_ENTRIES_MAX`, e.g. 24) with
  `truncated`.

Gaps between regions (such as 0xC000 to 0x10000, left by app alignment) are
computed by the client from offsets and sizes.

### `GET /storage/internal/regions/{region}/content`

The region's bytes. Without a `Range` header, the whole region: an app slot
is its full 3 MiB, erased tail included, so it writes back identically with
`esptool write_flash`. With one, the bytes it asks for.

- Ranges are HTTP's (RFC 9110): `Range: bytes=0-1203455` answers `206
  Partial Content` with `Content-Range`. One range per request; a request for
  several, or one past the region, answers `416` with `Content-Range: bytes
  */<size>`. Offsets are relative to the region.
- Every answer carries `Accept-Ranges: bytes` and a strong
  `ETag: "<boot>-<generation>"`: a random value drawn once per boot and the
  region's write generation (see "Invalidation"). The tag changes whenever
  the bytes may have, and on every reboot, as `app_current` may then be the
  other slot. `If-Range` with a tag that no longer matches answers the whole
  region with `200`, which tells a resuming client to start again. A resume
  across a reboot therefore always starts again; that is rare and harmless.
- A write to the region while a download streams it closes the connection
  before the last byte, so the client sees a short body rather than torn
  contents, and its resume finds the tag changed.
- `Content-Type: application/octet-stream`, `Content-Length`, and
  `Content-Disposition: attachment` with a filename (below), plus the API's
  usual headers (`nosniff`, `no-store`, CSP, CORP, XFO).
- Errors:
  - `404 not_found`: no region with that name.
  - `403 sensitive`: the region is `sensitive` or `reserved`. It never
    leaves the device, whatever the session's scopes.
  - `409 unavailable`: public, but the firmware holds no handle this boot
    (the untrusted tier, or a board table that isn't known).
  - `416 range_not_satisfiable`: as above.
  - `503 busy` with `Retry-After`: another download is running.
- One download at a time. A download holds one of the four HTTP workers for
  its length; the other three keep serving.
- The handler reads at most 1 KiB per flash read (one TCP buffer), so no
  read holds the flash's critical section for long, and yields between
  reads.

### Filenames

`<device-name>-<region>[-<slot>]-<what>.bin`, for example:

```
eveningstar-0kd7-9c6-app_current-ota_0-image-07aa31c2.bin   the image alone
eveningstar-0kd7-9c6-app_current-ota_0-slot-of-07aa31c2.bin whole slot
eveningstar-0kd7-9c6-app_current-ota_0-0x0-0xfff.bin        any other range
eveningstar-0kd7-9c6-bootloader-image-8d02b7e4.bin          the bootloader image alone
eveningstar-0kd7-9c6-bootloader.bin                         the whole region
```

- The slot appears for apps when it is known. Hyphens rather than brackets,
  so the name needs no quoting in a shell.
- `image-<hash>` is a download of exactly the image, and the hash is the
  first 8 hex digits of its `file_sha256`, so `sha256sum` of the file starts
  with it. `slot-of-<hash>` is a whole slot holding that image: the hash
  names the firmware inside, not the file, and the wording says so. Other
  ranges carry their bounds. Two downloads of different contents can't then
  share a name.
- A hash appears only while the image's check is current; otherwise the
  name carries the range alone.
- The device names the file in `Content-Disposition` for `curl -OJ` and other
  clients. The web UI builds the same name itself, as it saves through a
  `download` link.

### The boot-time checks

`split` already hashes the bootloader. It reads only the running app's
descriptor, so that the boot isn't held up (about 17 ms today).

After `split`, a task of its own checks each app slot the firmware holds a
handle for, one after the other, yielding every 4 KiB: `verify_app` (the
checksum and the stored digest), giving the image's status, `digest` and
`file_sha256`. The second digest comes from the same pass: the hasher is
cloned before it finishes, and the clone is fed the 32 stored digest bytes.
The bootloader's two digests come from `split`'s verification the same way.
Both need a small addition to `eveningstar-storage`.

Results land in the storage service as they come, so the API reports
`checking` until then. The cost on the board is to be measured: each image,
about 1.2 MiB today, read and hashed in software. A slot that fails its
check is a finding the page shows as serious, the first sign of failing
flash or an aborted write.

### Invalidation

Only the firmware writes the flash while it runs, and only through
`InternalFlash`, the one `Flash` (`FlashStorage::new` is disallowed
anywhere else). So `InternalFlash` counts writes, and no writer has to
remember to say what it changed:

- It keeps a generation per layout partition, an `AtomicU32`. Each erase and
  program bumps the generation of the partition it touches, before the
  operation. Protected regions (the bootloader, the table, the running app)
  never take a write, so their generations stay 0 for the whole boot.
- A check notes the generation before it reads, and stores its result only
  if the generation is unchanged after; otherwise it discards it and checks
  again. A result is current while the stored generation equals the live
  one, and the API reports `checking` once it isn't.
- After a write, the task checks the slot again once the slot has gone
  quiet (no write for a few seconds), so an OTA write of a minute costs one
  check, not hundreds.
- A download streaming a region compares the generation after each chunk
  and closes the connection when it moves.
- The `ETag` is the boot's random value and the generation.

Today nothing writes a public region while the firmware runs; the first
writer will be the OTA service, through `Ota`. Configuration and diag writes
bump their own partitions' generations, which nothing reads yet.

Writes from outside, over the USB debug link, aren't counted. Flashing
through probe-rs or esptool resets the chip, which starts a new boot with a
new random value.

Not now: re-checking on request, or periodically to catch flash going bad.
Both would reuse the same task.

### Watchdogs and yielding

The executor is cooperative and has one priority, so a long job is only as
polite as its yields, and a job that stops yielding starves the watchdog
supervisor, whose MWDT resets the chip after 5 s. Both long jobs here, the
check and a download, are supervised, and both feed on progress, never on a
timer, so a job that hangs while the rest of the system runs is still
caught.

- **The watchdog gains an idle state.** A handle can say `idle()`: its
  deadline isn't checked until its next `feed()`. Today a handle can only
  feed or retire for good, which doesn't fit a job that waits for work for
  hours and then runs. A task that hangs while idle isn't caught; idle means
  waiting for an outside event, which is where that is acceptable. A small
  change to `watchdog.rs`.
- **The check task** has its own handle, `storage_check`, with a 10 s
  timeout. It feeds on each flash read of a check, through a `Source`
  wrapper around the slot's handle, so every 4 KiB step feeds and the
  storage crate needn't know about watchdogs. It is `idle` while it waits for
  a write to settle or for work.
- **Downloads** use one handle, `storage_download`, 10 s, held by the
  download lock: there is one download at a time, so one handle covers it.
  Taking the lock feeds, each chunk read from the flash and written to the
  socket feeds, and releasing the lock (on completion, error or the client
  going away) sets it `idle`. A client that stops reading is ended by
  picoserve's 1 s write timeout well before the deadline, so only a hang in
  the firmware itself resets the chip.
- **Uploads**, which come with the OTA service, take the same pattern: a
  handle held for the length of the upload, fed per chunk written.
- **Yielding.** The check yields every 4 KiB hashed (the storage crate's
  `yield_now`, about 1 to 2 ms of hashing; to be measured). A download
  yields at every socket write and once per chunk, which also bounds how
  long a fast client on a local network can hold the executor.
- **Interrupts.** Each flash operation runs with interrupts disabled. A
  download reads at most 1 KiB per read, and the storage crate reads at most
  256 bytes at a time, so no read holds them off for long; erases, which do,
  belong to the writers and are already bounded to a sector.
- The HTTP workers themselves aren't supervised: their connections are left
  to picoserve's timeouts. The download's handle covers the one
  long-running path a worker gets here.

## Changes to `eveningstar-api`

Capabilities the API doesn't have yet, in their own PR before the storage
endpoints:

1. **Path parameters.** `Endpoint` gains `type Params: PathParams`, `()` for
   every existing endpoint. A path template such as
   `/storage/internal/regions/{region}/content` matches one segment per
   parameter, and `Operation::find` returns the operation with its parsed
   parameters, or `404` when a segment doesn't parse. The OpenAPI document
   declares each parameter with its schema (the region enum). The handler
   receives the parameters beside the body.
2. **Binary, ranged response bodies.** A `ResponseBody` beside `Empty` and
   `Json<T>`, such as `Bytes`, for `application/octet-stream`. Its value in
   the firmware is a source with a length and an optional strong tag, never a
   buffer of the whole body. Range handling lives in one place: the API
   crate parses `Range` and `If-Range` against a length and tag (pure,
   tested on the host), and `http/api.rs` answers `200`, `206` or `416` from
   that for every binary endpoint, so a handler only provides the source.
   The OpenAPI document declares the `Range` and `If-Range` header
   parameters, `{ "type": "string", "format": "binary" }` bodies, and the
   `206` and `416` responses.

New `ErrorCode`s: `sensitive` (403), `unavailable` (409),
`range_not_satisfiable` (416), `busy` (503). They are additive; existing
clients read them as `unknown`.

## Firmware

- A storage service (`services/storage.rs`) holds the `Report`, the public
  handles only (the `BootChain` and the data tier's `other_app` and
  `otadata`, all `Copy`), and the results of the boot-time checks.
  `storage::start` keeps handing the sensitive handles to their future owners
  (dropped today). Its handle, `StorageView`, is the only thing the HTTP
  handlers get.
- The check task gets public handles only, so it can't hash a sensitive
  region.
- `InternalFlash` keeps the write generations ("Invalidation") and hands the
  storage service a read-only view of them. The boot's random value comes
  from the chip's RNG.
- `StorageView::content(region)` returns a source over `impl Read` only, so
  passing a configuration or diag handle fails to compile. The `403` comes
  from the region's `contents`, decided by name before any handle is looked
  up.
- The download lock is a flag in the service, taken for the length of a
  response and released on drop, including when the client goes away.
- Handlers live in `http/storage.rs`. They map the storage crate's types to
  the API's; the mapping holds no decisions beyond that.

## Web UI

A page at `#/storage`, named "Storage" in the navigation after "System". It
loads `/storage`, `/storage/internal` and `/storage/internal/regions` through
`LoadGate`, once on entry, again when the device restarts (uptime going
down), and on a refresh button. While `checks.status` is `running` it
reloads `/storage/internal` and the regions every few seconds until `done`.
Otherwise nothing is polled: the data changes only across reboots, and
later on OTA actions.

### Layout

```
Storage                                                        [Refresh]

┌ Internal flash ──────────────────────────────────────────────────────┐
│ ◆ Normal   Full tier · 8 MiB · configuration saved · updates ready   │
│                                                                      │
│ ▕▒▏▕▒▏▕▒▏▕▒▏░▕ ota_0 ▲current ███▕ ota_1 ░░░░░░ ▕c0▕c1▕dg▕reserved ▏  │
│  boot table factory otadata                                          │
├ Regions ─────────────────────────────────────────────────────────────┤
│ Region           Partition  Offset     Size     Contents  Layout     │
│ bootloader       —          0x000000   32 KiB   Public    —       ↓  │
│ partition_table  —          0x008000   4 KiB    Public    —       ↓  │
│ factory          factory    0x009000   4 KiB    Sensitive ✓       🔒 │
│ otadata          otadata    0x00a000   8 KiB    Public    ✓       ↓  │
│ (unallocated)               0x00c000   16 KiB                        │
│ app_current ▌    ota_0      0x010000   3 MiB    Public    ✓       ↓▾ │
│ app_alternate    ota_1      0x310000   3 MiB    Public    ✓       ↓  │
│ config_current   config_0   0x610000   128 KiB  Sensitive ✓       🔒 │
│ …                                                                    │
└──────────────────────────────────────────────────────────────────────┘

┌ Firmware ─────────────────────┐ ┌──────────────────────────────────────┐
│ Current (ota_0)               │ │ Alternate (ota_1)   Empty            │
│ Running · Selected            │ │                                      │
│ eveningstar 0.1.0             │ │                                      │
│ Built Oct 31 2026 10:12       │ │                                      │
│ Image  valid · c41f…07aa      │ │                                      │
│ ELF SHA-256 9a0b…c41f  [copy] │ │                                      │
│ 1.15 MiB of 3 MiB  ▓▓░░░░     │ │                                      │
│ [Download image] [Whole slot] │ │                                      │
└───────────────────────────────┘ └──────────────────────────────────────┘

┌ Boot chain ──────────────────────────────────────────────────────────┐
│ Bootloader  esp-idf-v5.5.1-rollback (known, supports rollback)       │
│             SHA-256 3f1c…  · ESP-IDF v5.5.1 · built Oct 30 2026      │
│ Table       0x8000, matches the layout (MD5 not checked)             │
│ Booted      0x010000 = ota_0, as the otadata selects                 │
│ Checked     at boot, 4 s after start                                 │
│ ▸ Otadata entries   (expandable: sector, seq, state, selects, valid) │
└──────────────────────────────────────────────────────────────────────┘

┌ External flash ──────────────────────────────────────────────────────┐
│ ○ Off   On the board, not supported by this firmware yet             │
└──────────────────────────────────────────────────────────────────────┘
```

- **Health banner.** Full is `normal`. Data is `caution`: "Updates are
  unavailable", with the limit in words (the bootloader isn't known to
  support rollback, it doesn't verify, the app's descriptor doesn't read).
  Untrusted is `serious`: "Running without saved configuration", with the
  limit in words, and the likely cause when it is one (flashed with
  `cargo run`, whose bootloader and table aren't the board's). A slot whose
  image is `invalid` is `serious`. A `needs_confirmation` boot adds a
  `caution` banner: the running firmware rolls back at the next reboot
  unless confirmed, which this firmware can't do yet. The firmware requests
  that reboot itself 10 minutes after boot while the image awaits a
  confirmation (reason `cannot_confirm`), and a bootloader with rollback then
  starts the previous image.
- **Flash map.** One bar for the whole flash, segments proportional to size
  with a minimum width so a 4 KiB partition stays visible, labelled in a
  legend when too narrow. Unallocated space is hatched. The current slot
  carries the 3 px selection bar; an app slot shows how much of it the image
  fills. Sensitive regions carry a lock shape, reserved ones are dimmed.
  Segments are links to their table rows; the table is the accessible source
  of truth, and the bar is `aria-hidden`.
- **Region table.** Offsets in hex, sizes in binary units with exact bytes
  and hex on hover and in the row's details. Each row expands to show type
  and subtype numbers, end offset, flags, the image digests for app regions and, when the
  board's table differs, expected against found field by field. The layout
  column is a status shape and word (matches, differs in offset and size,
  missing). The action is a download button, with a menu on app regions
  (image only, whole slot, a custom range), or a lock with the reason:
  "Holds configuration, secrets or factory data; never leaves the device",
  "Reserved for later firmware", "Not readable in the untrusted tier". At
  360 px the table becomes one card per region.
- **Board table differs.** A switch over the table: Layout / On the board.
  "On the board" lists the found entries with each mismatch marked, the
  extra entries past the layout's, and how the table ends.
- **Firmware cards.** One per app the API lists, titled by role with the
  slot: name, version, build date, ESP-IDF or HAL version, secure version,
  image status and digest, ELF SHA-256 shortened with a copy button, image
  size against the slot. Badges: Running, Selected for next boot, Rollback
  target, Needs confirmation. A slot that is selected but not running says
  the bootloader fell back from it.
- **Download.** A download button adds the download to the queue (below)
  rather than starting it. The page fetches the content into memory through
  the API client and hands it to the browser only when it is complete, so
  the browser's own download list shows nothing until then and the page
  shows the progress. It is done this way, rather than with a plain link
  the browser would download itself, because:
  - the request carries the session's credentials once authentication
    exists, which a plain link can't when they are held in memory;
  - the page can verify the bytes before saving them (below), and resume,
    queue and retry on its own terms;
  - the largest region is 3 MiB, so holding it in memory costs nothing that
    matters.

  Streaming to disk (the File System Access API, or a service worker) is
  left out: it is Chromium-only or a lot of machinery, needs a secure
  context, and buys nothing at these sizes. On completion the blob is saved
  through an object URL and a `download` link with the filename above, and
  the URL is revoked.
- **Download queue.** One app-wide download service, not the page, owns the
  downloads, so they carry on while the person moves to another route. It
  runs one at a time, in order:
  - Each entry is `queued`, `waiting` (another client is downloading),
    `downloading` (bytes of total), `verifying`, `done`, `failed` or
    `cancelled`. Entries can be cancelled and reordered while queued.
  - A `503 busy`, which means some other client holds the device's one
    download, waits for its `Retry-After` and asks again, for as long as the
    entry stays queued, showing "Waiting: another client is downloading" and
    for how long. It is never counted as a failure.
  - A broken download keeps what arrived and resumes, asking for the rest
    with `Range` and `If-Range`, a few times with backoff, then fails with a
    resume button. A `200` in answer means the region changed: the download
    starts again and says why.
  - A device restart (uptime going down) doesn't cancel the queue; the
    `If-Range` of a resumed entry then always answers `200`, and the entry
    starts again.
  - A download tray in the header shows the active entry's progress and the
    number queued, and opens the list. The Storage page shows the same state
    beside each region's button.
  - Closing or reloading the tab loses the queue, so while it isn't empty
    the page asks before unloading (`beforeunload`). Moving between routes
    doesn't.
- **Verifying a download.** When `isSecureContext` (the page served over
  HTTPS), the page hashes the download with `crypto.subtle` before saving
  it: an image download against the image's `file_sha256`, and the image
  part of a whole-slot or whole-bootloader download against its `digest`.
  A mismatch keeps the file from being saved as good and says so. Over HTTP
  the page shows the expected `file_sha256` for checking with `sha256sum`.
  Arbitrary ranges aren't verified.
- **Copying.** `navigator.clipboard` exists only in a secure context, and
  the page is served over HTTP. The copy button uses it when
  `isSecureContext`, and otherwise selects the text for the person to copy.

### Client changes

- The typed client learns path parameters (filled from the generated types),
  request headers, and binary responses with their `206` and `416`.
- The transport gets a download lane beside its queue: at most one download,
  user priority, no timeout while bytes keep arriving (a stall timeout
  instead, e.g. 10 s without a chunk), never retried on its own, aborted on
  cancel. Retrying, resuming and waiting on `busy` belong to the download
  service. Polls keep flowing through the queue meanwhile.
- New modules: `downloads/` (the app-wide download service: the queue, the
  per-entry state machine with resume and busy waits, verification, saving),
  `storage/` (service, types, labels for tiers, limits, roles, states, the
  map and gap computation, filenames), the page in `pages/Storage.tsx`, the
  tray in the shell, and presentational pieces in `ui/` (flash map bar,
  region table, copyable digest, progress, download list). Logic stays out
  of components and is tested without a DOM, as the rest of the client is.

## Verifying

- API crate: serde and schema tests for every new type, path parameter
  parsing, `Range` and `If-Range` parsing against lengths and tags (empty,
  open-ended, suffix, several, past the end, stale tag), and the binary body
  in the OpenAPI document.
- On the board: `curl` each endpoint in each tier (full after `flash-all`,
  untrusted after `cargo run`); download each public region and compare it
  byte for byte with `esptool read_flash` of the same range, and the
  `sha256sum` of each image download with its `file_sha256`; check `digest`
  against `esptool image_info`; download a range and resume a cut download;
  check that `config_current`, `diag` and `factory` answer `403`, a second
  concurrent download `503`, and an `If-Range` from before a reboot `200`.
  Measure how long the boot-time checks take, the longest gap between their
  yields, and HTTP latency while a check and a download run. With a
  temporary hang injected into the check and into a download's chunk loop,
  see the supervisor name the task and reset; with a client that stops
  reading, see the download end without a reset. The generations and the
  re-check after a write wait for the first writer (the OTA service); until
  then they are covered by review and by a test hook that bumps a
  generation.
- Web: unit tests with fakes for each tier, a differing table, an empty and
  an invalid slot, checks still running, and downloads that are cancelled,
  stall, resume, find the region changed or are busy; the queue running one
  at a time, reordering, waiting on `busy` without failing, surviving a
  route change and a device restart, and asking before unload while not
  empty; the page in headless Chromium in both themes and at 360 px, with no
  CSP or Trusted Types violations during a download.

## Delivery

A GitHub stack on `main`, one PR per piece:

1. `storage-ui-spec`: this file.
2. `api-path-params`: path parameters and binary, ranged bodies in
   `eveningstar-api`, `http/api.rs` and the OpenAPI document.
3. `storage-checks`: the image's second digest in `eveningstar-storage`, the
   write generations in `InternalFlash`, the watchdog's idle state, the
   boot-time checks under their own watchdog, and the storage service
   holding their results.
4. `storage-api`: the storage types and endpoints in `eveningstar-api`,
   then the handlers in the firmware.
5. `storage-web`: the client changes and the page.
6. `storage-ui-docs`: `docs/architecture.md` (path parameters, ranged
   bodies, the storage endpoints, the boot-time checks and why the content
   endpoint takes only `Read` handles), `specs/storage-v2.md` ("Left to build")
   and `specs/web-ui.md`.

2 and 3 are independent and can be built at the same time. Once the contract
commit of 4 exists, the firmware half of 4 and all of 5 are built at the same
time, the web side against fakes and the generated types.

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

## Out of scope

Writes of any kind (wiping is above), OTA uploads, confirming or rolling back
an image, sanitized dumps of sensitive partitions (`specs/storage-v2.md`), the
external NAND's contents, and checks on request or on a schedule.
