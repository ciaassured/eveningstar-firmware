# Storage API

The read-only API of the device's storage, under `/api/v1/storage`, built on
the second storage design (`specs/storage-v2.md`, `docs/architecture.md`
"Storage"). It replaces the API of `specs/storage-ui.md`, whose PRs (#65–#69)
were closed.

This work builds the storage API only. The APIs of the services that own what
regions hold (firmware images and boot state, configuration, diagnostics) are
sketched under "Later APIs" so the storage API leaves room for them, and are
built later.

## Decided with the user (2026-11-06)

- The storage API describes the flash as a medium: where the regions are,
  whether the layout checks out, and which bytes may leave the device. What
  the bytes mean belongs to the API of the service that owns them. Image
  identity, slot roles, boot state and confirmation go to `/firmware`;
  settings to `/config`; crash records and events to `/diag`.
- One document per region holds everything the storage API says about it.
  Nothing about a region is spread over several endpoints.
- Region names are one flat namespace across devices. The external NAND will
  hold different things under names of its own, and each region names its
  device in a field.
- The region documents describe what is on the flash this boot: the regions
  the chain located, at the located spans. The compiled layout appears only
  in `/storage/layout`, beside what was found.
- The unverified tier reuses the region names: the table the search found is
  `partition_table`, the app at the booted offset is `app_current`.
- A region holds named views, the unit of download. A format with
  independent parts has one view per part (the otadata's two entries).
- A view is the format's canonical form: the bytes it defines, the bytes
  inside it that nothing checks served as `0xFF` (sanitized) and listed.
  Sanitization is kept rather than splitting views around those bytes, so a
  table or otadata download stays a file the ESP-IDF tools read and that
  writes back as the bootloader reads it.
- A view that isn't published says why, in its `status`.
- The table comparison is its own endpoint, `/storage/layout`.
- No digest of the contents in the region document. A download carries the
  view's digest in `Repr-Digest` (RFC 9530), and the digest of a range in
  `Content-Digest` on request. A `HEAD` gives the view's digest without the
  body.
- No epoch in any document. A view's entity tag stands for it, and the
  region document carries the tag so a client can make a download
  conditional on it with `If-Match`.
- No transport digest endpoint, no whole-region downloads, no per-region
  `layout` field, no field naming the API that describes a region.
- Every span is `offset` and `size` (or `length` for a view), end exclusive.
- Fields that don't apply are `null`, never absent.

## Endpoints

Every endpoint has the `read` scope.

| Method      | Path                                     | Answers with                      |
| ----------- | ---------------------------------------- | --------------------------------- |
| `GET`       | `/storage`                               | Devices, the chain's result, tier |
| `GET`       | `/storage/layout`                        | Expected and found partitions     |
| `GET`       | `/storage/regions`                       | Every region's document           |
| `GET`       | `/storage/regions/{region}`              | One region's document             |
| `GET, HEAD` | `/storage/regions/{region}/views/{view}` | A published view's bytes          |

## `GET /storage`

```json
{
  "devices": [
    { "device": "internal", "kind": "nor_flash", "status": "available", "capacity": 8388608 },
    { "device": "external", "kind": "nand_flash", "status": "not_supported", "capacity": null }
  ],
  "chain": {
    "tier": "boot",
    "failed": null,
    "steps": {
      "bootloader": true,
      "table_offset": true,
      "table": true,
      "layout": true,
      "flash_size": true,
      "booted_slot": true
    },
    "bootloader": {
      "status": "known",
      "name": "esp-idf-v5.5.1-rollback",
      "rollback": true,
      "fits": true
    },
    "booted_offset": 65536,
    "booted_label": "ota_0"
  },
  "allows": { "persistent_config": true, "updates": true }
}
```

- `devices`: `status` is `available` or `not_supported` (on the board, no
  driver in this firmware). Later values could be `absent` or `faulty`.
  `capacity` is `null` when the device doesn't give it (an unknown JEDEC ID,
  which `Report.capacity` reports as 0).
- `chain` is the internal flash's verification chain, from `chain::Report`:
  - `tier` is `unverified`, `layout` or `boot`.
  - `failed` is the step that kept the tier below the boot tier, or `null`.
    `steps` gives every step's result (`Report::passed`).
  - `bootloader.status` is `known` (verifies and listed in `known.rs`),
    `unlisted` (verifies, not listed) or `invalid`. `name` and `rollback` are
    the known entry's, `null` otherwise. `fits` says whether the image ends at
    or before the end of the bootloader's region, `null` when it doesn't
    verify. On a working board the bootloader always ends before its own
    table, so the region, which ends at the located table, holds it. When no
    table is located, the region ends at the compiled offset (0x8000), which
    is an assumption about the board: a board flashed by a project with a
    larger bootloader and its table at a higher offset has a bootloader
    longer than that region. The chain verifies the image over the whole
    flash, but its view is bounded by the region, so `fits` is `false` and
    the `image` view `unverified`. The bootloader's own identity (version,
    descriptor, digests) belongs to `/firmware`.
  - `booted_offset` is the offset of the app the bootloader started, from
    MMU entry 0, `null` when entry 0 isn't valid (`Report.booted` is
    `u32::MAX`). `booted_label` is the label of the located table's partition
    there, `null` when there is none or its label isn't covered.
- `allows` says what the tier allows, so a client doesn't derive behaviour
  from tier names: `persistent_config` is the layout tier or above, `updates`
  the boot tier. Whether an update can start now is `/firmware`'s.

## `GET /storage/layout`

The table the chain located, the compiled layout, and their comparison.

```json
{
  "table": {
    "status": "valid",
    "expected_offset": 32768,
    "offset": 32768,
    "source": "known_bootloader",
    "search": "not_run",
    "candidates": [],
    "failure": null,
    "coverage": "layout",
    "md5_read_as_partition": false
  },
  "expected": [
    {
      "label": "ota_0", "type": 0, "subtype": 16, "offset": 65536, "size": 3145728, "flags": 0,
      "comparison": { "status": "matches", "fields": [] }
    },
    {
      "label": "config_0", "type": 1, "subtype": 64, "offset": 6356992, "size": 131072, "flags": 0,
      "comparison": { "status": "differs", "fields": ["offset", "size"] }
    }
  ],
  "found": [
    {
      "kind": "partition",
      "label": "ota_0", "type": 0, "subtype": 16, "offset": 65536, "size": 3145728, "flags": 0,
      "extra": false
    }
  ]
}
```

- `table.status`:
  - `valid`: the bootloader accepts it, and its partitions read.
  - `refused`: the bootloader refuses it. `failure` is
    `{ "code": "<TableFailure>", "index": n }`, the index `null` for the
    failures that have none. A read error is the code `read`; no value read
    from the table is reported.
  - `unread`: located at `offset`, but the firmware holds no reader for it,
    as when a known bootloader's table offset isn't the compiled one.
  - `not_located`.
- `expected_offset` is the compiled table offset (`layout::TABLE_OFFSET`),
  `offset` the located one or `null`.
- `source` is `known_bootloader`, `searched` or `null` when not located.
  `search` is `not_run`, `none`, `one` or `several`; the search runs only for
  a bootloader that isn't known, whose header reads, and a booted offset
  inside the flash. `candidates` gives the first two offsets the search found
  when it found several (`Found::Several`), and is otherwise empty.
- `coverage` is `layout`, `md5` or `uncovered` (`table::Coverage`),
  and `md5_read_as_partition` whether the bootloader loads the MD5 entry as a
  partition. Both are `null` and `found` is empty unless `status` is
  `valid`.
- `expected` is the compiled layout (`EXPECTED`), each partition with its
  comparison by label: `matches`, `differs` with the fields, or `missing`
  (`fields` empty for the last). Unless the table is `valid`, nothing is
  compared, and every partition is `missing`.
- `found` is every row the bootloader loads from the located table, in
  table order, so a row's position in the array is its entry's index in the
  table (`Rows` yields every entry from 0, the MD5 entry included when it is
  read as a partition). A table sector holds at most 95 entries before its
  end entry, so `found` isn't capped.
  - `kind` is `partition`, or `md5` for the MD5 entry read as a partition
    (`PartitionRow::Md5`), whose other fields are `null`.
  - `label` is `null` when nothing covers it (`Valid::label_covered`): every
    row of an `uncovered` table, and in an `md5` table every row at or after
    the MD5 entry. Text the flash holds is printable ASCII, any other byte as
    `?`.
  - `extra` is `true` when the layout has no partition of its label, `null`
    when the label is `null`. Labels compare as all 16 bytes, so a label with
    bytes after its NUL is `extra` and its layout partition `missing`.
- For an `uncovered` table, `expected[].comparison` still says whether a
  layout label was found. That reveals only that a label equals a compiled
  constant, which the pin-down rule allows (`specs/storage-v2.md`,
  "Publication"), and the tier already reveals whether the whole table
  matches.
- The bootloader's and the table's own regions are in `/storage/regions`.
  Gaps (such as 0xC000 to 0x10000) are computed by the client from offsets
  and sizes.

## `GET /storage/regions` and `GET /storage/regions/{region}`

```json
{
  "region": "app_current",
  "device": "internal",
  "class": "publishable",
  "span": { "offset": 65536, "size": 3145728 },
  "partition": { "label": "ota_0", "type": 0, "subtype": 16, "flags": 0 },
  "views": [
    {
      "view": "image",
      "status": "published",
      "offset": 0,
      "length": 1203456,
      "sanitized": [],
      "etag": "\"5c1e9a0f3b2d7781-3\""
    }
  ]
}
```

`/storage/regions` answers `{ "regions": [ ... ] }`, every listed region's
document in offset order.

### Region names

`{region}` is a closed enum.

| Region             | Covers                                         | Class       |
| ------------------ | ---------------------------------------------- | ----------- |
| `bootloader`       | 0x0 up to the located or compiled table offset | publishable |
| `partition_table`  | The located table's sector                     | publishable |
| `factory`          | `factory`                                      | sensitive   |
| `otadata`          | The located otadata                            | see below   |
| `app_current`      | The slot that booted, or the found app         | publishable |
| `app_alternate`    | The slot that didn't boot                      | publishable |
| `config_current`   | The current slot's configuration               | sensitive   |
| `config_alternate` | The alternate slot's configuration             | sensitive   |
| `diag`             | `diag`                                         | sensitive   |
| `reserved`         | `reserved`                                     | reserved    |

- A publishable region is listed when the firmware holds its reader this
  boot, at the reader's span. A sensitive or reserved region is listed when
  its span is known this boot.
  - In the layout and boot tiers every region above is listed. Their spans
    are the located table's, which in these tiers equal the compiled
    layout's.
  - In the unverified tier the regions are the ones the chain located:
    `bootloader`, `partition_table` when the firmware holds a reader of the
    located table (the compiled sector, which a known bootloader reads, or
    the `FoundTable`), and `app_current` and `otadata` when it made their
    `Found*` readers. The located table's other partitions are in
    `/storage/layout` only. A table located without a reader isn't listed
    (`/storage/layout` reports it as `unread`), nor is a found app or otadata
    over a span shared with a sensitive partition of the layout.
- `otadata` is `publishable` at its compiled offset and `sensitive` when a
  located table gives it (`FoundOtadata`), as its parser can't pin down an
  entry of a layout it doesn't know.
- `class`: `publishable` regions have views; `sensitive` regions never leave
  the device, as bytes or as a digest; `reserved` regions have no handle and
  hold nothing yet. A sensitive or reserved region has an empty `views`, the
  found otadata included, and its class is the reason.
- `partition` is the located table's entry, `null` for `bootloader`,
  `partition_table`, and an `app_current` the chain located from MMU entry 0
  without a valid table, which spans the image its headers describe. Its
  `label` is `null` when the label isn't covered.
- `current` and `alternate` swap meaning when the device boots the other
  slot; `partition.label` names the slot. A download across a reboot is
  caught by its tag.

### Views

A view is a published view (`nor::Published`): the bytes of a region a
content module verified, in the format's canonical form. A publishable
region lists the views its format defines, published or not, in this order.

| Region            | Views                | Each view is                          |
| ----------------- | -------------------- | ------------------------------------- |
| `bootloader`      | `image`              | The bootloader image, digest included |
| `partition_table` | `table`              | The entries the table's check covers  |
| `otadata`         | `entry_0`, `entry_1` | The 32-byte entry of each sector      |
| `app_current`     | `image`              | The app image, digest included        |
| `app_alternate`   | `image`              | The app image, digest included        |

`status` says whether the view is published, and why not:

- `published`: `offset` and `length` place the view within the region's
  span; `sanitized` lists the spans within the view served as `0xFF` that
  aren't erased on the flash (`offset` and `size`, at most `MAX_SANITIZED`,
  4); `etag` is the download's entity tag.
- `writing`: a write session is open on the region.
- `checking`: the region is stable, but no model exists at its current epoch
  yet. In practice the app slots, whose check hashes the image in the
  background. The bootloader's model is loaded at boot and stays: its region
  has no writer, so the load can't fail (`Busy` and `Stale` need one), and a
  flash read error is kept in the model as `unreadable`.
- `unverified`: the contents don't pass the format's check: no image or an
  image that doesn't verify, a table whose check covers nothing (no MD5
  entry below the layout tier), an otadata entry whose CRC doesn't match or
  whose state isn't defined. Whether a slot is empty or corrupt, and why, is
  answered by `/firmware`; for the table, by `/storage/layout`.
- `unreadable`: the flash failed to read (`ReadError::Device` in the model,
  `PublishError::Device`), or the storage crate refused the view for a reason
  that is an invariant violation (`Sensitive`, `OutOfRange`,
  `SanitizedOutOfRange`, `TooManySanitized`), which the firmware also logs.

`offset`, `length`, `sanitized` and `etag` are `null` unless published.

The handler parses a model and publishes from it within one request. When
either fails `Busy` or `Stale` (the region was written meanwhile), it reads
the write state again and reports `writing` while a session is open, or
`checking` when the region is stable at a newer epoch. A status other than
`published` can therefore be transient.

## `GET /storage/regions/{region}/views/{view}`

The view's bytes, whole or by range.

- **Ranges** are HTTP's (RFC 9110 §14), relative to the view:
  `Range: bytes=0-4095` answers `206 Partial Content` with `Content-Range`.
  One range per request; several, or one past the view, answer `416` with
  `Content-Range: bytes */<length>`. `200` and `206` carry
  `Accept-Ranges: bytes`.
- **Entity tag**: a strong `ETag`, the view's `etag`: the boot's random value
  and the region's epoch (`specs/storage-v2.md`, "What the API gets"). It is
  never derived from the contents. It changes once per write session that
  reached the flash, and at every boot. The views of one region share its
  epoch, so the otadata's two entries change tags together; tags are per
  resource, so sharing a value is allowed (RFC 9110 §8.8.3).
- **Conditions** (RFC 9110 §13):
  - `If-Match`: strong comparison. `*` matches any published view; a list
    matches when one member does. No match answers `412`, so a client that
    read a region document downloads exactly the view it described.
  - `If-Range` with a tag that no longer matches answers the whole view with
    `200`.
  - `If-None-Match` is ignored, as every answer is `Cache-Control:
    no-store`.
- **Digests** (RFC 9530, `sha-256` only, base64 as a structured-field byte
  sequence):
  - `Repr-Digest` on `200`, `206` and `HEAD`: the SHA-256 of the whole view
    as served. For an image it is the file SHA-256 the check computed. The
    table and otadata views are hashed on request. Error answers carry none,
    as their representation is the error's (RFC 9530 Appendix B.10).
  - `Content-Digest` on a `GET` whose `Want-Content-Digest` dictionary
    (RFC 9530 §4) has the key `sha-256` with an integer weight from 1 to 10:
    the SHA-256 of this answer's bytes. Any other value of that header, and
    `Want-Repr-Digest`, are ignored, which the RFC allows. When the answer is
    the whole view, `Content-Digest` equals `Repr-Digest` and costs nothing
    more. For a range, the device hashes it in a first pass and sends it in a
    second, both at the tag's epoch.
  - Digests cover the view as served, sanitized bytes as `0xFF`.
- **`HEAD`** answers `200` with the headers of a `GET` of the whole view:
  `Content-Length` of the view, `ETag`, `Repr-Digest`, `Accept-Ranges` and
  `Content-Disposition`. It ignores `Range` and `If-Range` (RFC 9110 §14.2)
  and never carries `Content-Digest`, which on a `HEAD` would describe empty
  content (RFC 9530 Appendix B.2). `If-Match` applies as for `GET`.
- **A write while an answer streams** fails the read `Stale`, and the body
  ends before its length; a chunk the client takes longer than 5 s to drain
  ends it the same way. A client sees a short body, never torn contents. A
  write or a failed read during the digest pass, before the headers, is
  answered with `409`.
- **Headers**: `Content-Type: application/octet-stream`, `Content-Length`,
  `Content-Disposition: attachment` with a filename (below), and
  `Connection: close` on every answer with the body or its headers, plus the
  API's usual headers. picoserve keeps the connection open until the next
  request's wait times out (1 s), so a client that ignores the header sees a
  short body when the connection closes then.
- **Errors**, checked in this order:
  1. `403 sensitive`: `factory`, `config_current`, `config_alternate`,
     `diag` and `reserved` in every tier, by name, before any reader is used,
     listed or not, whatever the view's name.
  2. `404 not_found`: no region or view of that name, a view the region
     doesn't define, or a publishable region that isn't listed this boot.
  3. `403 sensitive`: `otadata` when the found otadata is listed.
  4. `409 unpublished`: the view's status isn't `published`. The region
     document says why, and the status can be transient.
  5. `412 precondition_failed`: `If-Match` doesn't match. Checked from the
     model before the download lock is taken, so a client with a stale tag
     never gets `503`.
  6. `416 range_not_satisfiable`.
  7. `503 busy` with `Retry-After`: another download or digest pass holds
     the lock. A `HEAD` takes the lock too, so a `HEAD` during a download is
     `503`, even of an image, whose digest is known.
- **One download at a time**, through a static 1 KiB buffer and its own
  watchdog handle, fed per 1 KiB chunk read through that buffer, in the
  digest pass and while the body streams. The watchdog is paused between
  downloads.
  Each chunk is at most 1 KiB, so no read holds the flash's critical section
  for long, and the handler yields between chunks. A download holds one HTTP
  worker for its length.

### Filenames

`<device-name>-<region>[-<label>]-<view>-<hash>[-at<offset>-len<length>].bin`:

```
eveningstar-0kd7-9c6-app_current-ota_0-image-07aa31c2.bin                the whole view
eveningstar-0kd7-9c6-app_current-ota_0-image-07aa31c2-at0x0-len0x1000.bin a range of it
eveningstar-0kd7-9c6-bootloader-image-8d02b7e4.bin
eveningstar-0kd7-9c6-otadata-otadata-entry_0-1b9c4f20.bin
```

- `<device-name>` is the system's name (`System.name`). `<label>` is the
  partition's label, when it has a covered one. Characters outside
  `[A-Za-z0-9._-]` become `_`, as the `Filename` writer does.
- `<hash>` is the first 8 hex digits of the view's SHA-256 (the
  `Repr-Digest`), so `sha256sum` of a whole download starts with it. A range
  keeps the view's hash and adds its offset and length in hex, so two
  downloads of different bytes never share a name.
- The web UI builds the same name itself.

## Changes to `eveningstar-api`

- `ResponseBody::HEAD`, which `Bytes` sets: a `GET` endpoint with a binary
  body also answers `HEAD`. `HEAD` on the JSON endpoints stays `405`.
- `If-Match` beside `If-Range` in `range.rs`; `Bytes`'s error set gains
  `PreconditionFailed` (412).
- `Repr-Digest` and `Content-Digest` formatting, and `Want-Content-Digest`
  parsing as above.
- Error codes `sensitive` (403), `unpublished` (409),
  `precondition_failed` (412), `busy` (503), and `Retry-After` on `busy`.
- `{region}` and `{view}` as closed-enum path parameters; an unknown value
  doesn't parse, so it is a 404. `GetView` takes the pair.
- The OpenAPI document: the `HEAD` operations, the `Range`, `If-Range`,
  `If-Match` and `Want-Content-Digest` header parameters, the `206`, `412`
  and `416` answers, and the digest and `Retry-After` headers.

## Firmware

- A storage service holds the content modules' statics and the chain's
  `Report`, and answers the documents from fresh models on each request.
- Handlers in `http/storage.rs` map the storage and boot crates' types to the
  API's. The body reads through the content modules' views only, so no raw
  reader or snapshot reaches a response.
- The documents are bounded: the region names, the table sector's 95
  entries, `MAX_SANITIZED`. `/storage/regions` and `/storage/layout` are
  written in parts with chunked transfer coding (`JsonParts` in
  `http/api.rs`): the literal JSON between their elements (`Regions::START`
  and the other constants of the API crate), and each element, built when it
  is written and serialized when its chunk is formatted. picoserve formats a
  JSON body once to measure it and again for each TCP buffer it fills, and
  needs the same output each time, which a list read from the flash as it is
  written can't promise. Each element is checked to fit 768 bytes before its
  chunk is written, as a chunk whose formatting fails is left out silently.
- A streamed document is sent after its `200` and headers, so a flash read
  that fails partway through `found` (a device error; the table has no
  writer, so it can't go stale) can't change the status. The device then
  ends the chunked body before the JSON does, and the client sees a body
  that doesn't parse and retries the request. The connection stays usable.
  A region document doesn't fail this way: a region written while it is
  built reports `writing` or `checking`. The web UI's client retries a
  document request whose body doesn't parse.
- Clippy's stack-frame limit (1024 bytes) sums every MIR temporary and
  awaited future it can size, without overlap. The documents and views are
  therefore built by synchronous, non-inlined functions; each streamed
  document has one await site; the storage routes are dispatched through one
  future; and the storage crate's `PublicView` (`Published::erase`) gives
  one read and digest path for every view.

## Delivery

PRs, #88 and #90 on `main` and #91 to #93 each on the one before:

1. #87 `storage-api-spec`: this spec.
2. #88 `api-http-conditions` (`eveningstar-api` only): `HEAD`, `If-Match`
   and `412`, the digest headers and `Want-Content-Digest`, the new error
   codes, `Retry-After`, and their OpenAPI description.
3. #90 `storage-api-types`: the documents' API types, their parts, and the
   web client's path parameters.
4. #91 `storage-api-documents`: the storage service and the documents'
   handlers.
5. #92 `storage-api-views`: the view downloads, the lock, buffer and
   watchdog, the digests and the filenames.
6. `storage-api-docs`: `docs/architecture.md`, the pointer in
   `specs/storage-v2.md`, and this spec brought in line with what was built.

The firmware PRs are tested on the board in every tier the bench can reach.

## Later APIs

Designs noted so the storage API leaves room for them. Not built in this
work.

### `/firmware`

- `GET /firmware`:
  - `booted`: the region (`app_current`) and its label.
  - `slots`: `current` and `alternate`, each with its region name, label and
    image: `checking`, `empty`, `invalid` with the failure code, or
    `verified` with size, the validation hash (the stored SHA-256), the file
    SHA-256, the app descriptor (project, version, date, time, IDF version,
    secure version, ELF SHA-256) and the layout record's comparison.
  - `boot`: from the otadata: the slot selected for the next boot, its OTA
    state, whether the alternate slot is a rollback target.
  - `confirmation`: whether the running image awaits it, the reboot
    deadline of the confirmation window, and the self-checks with their
    results.
  - `bootloader`: the known entry, size, digests and descriptor.
  - A download of an image is the storage view; the client links to
    `/storage/regions/<region>/views/image`.
- `POST /firmware/confirm` (`Ota::confirm`): `409` with the checks while one
  is pending or failed.
- Later: an update upload, activate and roll back, with the `configure` or
  `operate` scope once authentication exists.

### `/config`

Settings read and written through the configuration store, with the
`configure` scope. Backup and restore are protected paths for later.

### `/diag`

Crash records and events from the diag store, read-only.
