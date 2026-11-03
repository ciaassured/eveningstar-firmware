# Storage, second design: regions, snapshots and content modules

Status: proposal, for review. Nothing here is built yet. It replaces the
storage layer described in `docs/architecture.md` ("Storage") and
`specs/storage.md`; once it is implemented, both are rewritten to match and
`CLAUDE.md` ("Storage") is updated with it.

> **Two rules keep secrets on the device.** A raw region read is never
> exported: bytes leave the device only through a published view, which a
> content module makes over bytes its parser verified. And a published view
> never covers a span that the compiled layout gives to sensitive data. Don't
> relax either rule, add a route from a raw reader to a response, or hand out a
> writer outside the tiers below without asking the user.

## Why

The first design is sound in its layout gate, its role types and its otadata
writes, and those carry over. Four things change:

1. **Public meant the whole region.** Every public handle read, hashed and
   downloaded its whole span: the bootloader's 0x0–0x8000 around an image of
   about 22 KiB, the table's 0xC00 bytes, the otadata's 8 KiB around two
   32-byte entries, and each 3 MiB app slot. Neither an update nor esptool
   erases past the image, so those spans hold whatever was there before.
2. **OTA lived in the storage crate:** the update typestate, the otadata
   planners, image verification, the full tier (which existed to gate `Ota`),
   the descriptor and project checks, and the T7 reboot.
3. **The writer bypassed the types.** `Ota` held raw spans of the other slot
   and the otadata beside `Copy` read handles over the same spans.
4. **The device layer was internal NOR only.** The `Flash` trait, the
   sector and word sizes and the protected-region guard assume the internal
   flash and its layout, so the external NAND can't reuse them.

## Decided with the user

- Vocabulary: **region** for any extent of a flash the firmware knows about
  (the bootloader and the table aren't partitions). App slots and
  configurations are named by role, **current** and **alternate**, with the
  slot (`ota_0`, `ota_1`) reported beside them.
- The storage crate owns the flash devices and splits them into singleton
  region handles. It enforces bounds, protected spans, capabilities and write
  state. It knows no file format beyond what the verification chain needs.
- Content modules own region handles and decide what leaves the device.
  Parsed models are constructed only by their parser.
- Reading and writing run concurrently. Readers read through snapshots, which
  fail once the region is written; the writer always wins. While a write
  session is open, no snapshot of the region can be taken.
- The write state is a type, not an even or odd number checked in several
  places.
- A table's raw bytes are public up to the end its parser found. The
  discovered table is always reported, beside the expected layout in the same
  shape, with how it was located.
- The boot writers (alternate slot and otadata) are handed out only at the
  end of the verification chain: a known bootloader, so the table's offset is
  certain, then the table, its MD5 and the layout.
- No whole-slot downloads. No zeroize capability: wiping is an erase by the
  region's owner.
- Hashes are two separate things: integrity fields that formats store (app and
  bootloader SHA-256, table MD5, otadata CRC), and transport digests over
  any public range a client asks for, computed on request.
- Secrets may leave the device later through protected paths: a configuration
  backup, and a clone export of the whole chip. Both need authentication,
  which isn't built.
- Separate crates. The storage, OTA and other system components are meant to
  be reusable by other firmware, so they don't depend on the application.

## Crates

```
eveningstar-esp-formats   pure: Source, ReadError, Text, image, table, otadata (model and planners), md5, hash, pace
        ↑
eveningstar-storage       devices, roles, write state, Reader/Writer/WriteSession/Snapshot, publication,
        ↑                 layout, known bootloaders, verification chain, split
eveningstar-boot          content modules: bootloader, partition table, otadata, current and alternate app
        ↑
eveningstar-ota           updates, activation, confirmation, rollback, T7
        ↑
eveningstar               the firmware: the device implementations, the services, the API
```

- **`eveningstar-esp-formats`** reads the ESP formats from any `Source` and
  touches no flash. The otadata planners stay here: they are pure, and they
  describe ESP-IDF's rules. It takes `image.rs`, `table.rs`, `otadata.rs`,
  `source.rs`, `text.rs`, `md5.rs`, `hash.rs` and `pace.rs` with their tests.
- **`eveningstar-storage`** depends on the formats crate for the chain only:
  the bootloader's image (its length and stored digest) and the table's
  entries and MD5. It exposes `test-support` (the RAM flash, the simulated
  bootloader, the power-cut harness) behind a feature, for the crates above.
- **`eveningstar-boot`** turns readers into views. It holds no writer.
- **`eveningstar-ota`** holds the boot writers and reads through them; it asks
  `eveningstar-boot` for views of the slots and the otadata.
- The configuration store, diag store and factory data module own their
  regions later. Each may be its own crate or a firmware module.
- Crate names keep the `eveningstar-` prefix for now; system crates never
  depend on application crates (`morningstar`, the charge-controller service),
  so they can be renamed and moved out later.
- Every crate stays `no_std`, allocation-free and buildable on the host, and
  is added to `host_crates` in `nix/checks.nix`.

## Devices

A device is one flash chip. The storage crate defines one trait per kind of
flash and one module per device: `internal` now, `external` later.

```rust
/// A NOR flash: reads at any alignment, erases sectors, programs words, and
/// programming only clears bits. Each call is atomic with respect to every
/// other call on the same device.
pub trait NorDevice: Sync {
    const SECTOR_SIZE: u32;
    const WORD_SIZE: u32;
    fn capacity(&self) -> u32;
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError>;
    fn erase_sector(&self, address: u32) -> Result<(), DeviceError>;
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError>;
}
```

- The internal flash is a `NorDevice`. Its implementation (`esp-storage`
  behind a critical-section mutex, as now) stays in the firmware; it no longer
  counts writes, as the write state moves into the storage crate.
- The crate calls a device with ranges inside its capacity, aligned, and at
  most one sector erased or 4 KiB programmed per call, so no call keeps
  interrupts off for longer than one sector erase. Long operations yield
  between calls.
- `.clippy.toml` keeps disallowing `FlashStorage::new` outside the firmware's
  storage glue.

The external NAND (GD5F2GM7UE, on SPI2 with the W5500) is out of scope, but
nothing here may assume NOR outside `internal`. Its device trait will be
async; it will have 2 KiB pages with a spare area, 128 KiB block erases, bad
blocks, ECC status on reads, and one program per page per erase. Its
operations will lock the shared bus, not disable interrupts, and release the
bus between status polls. Its layout check will need an on-flash label, since
it has no ESP partition table. Roles, write state, snapshots, publication and
capabilities are device-independent and are shared with it.

## Regions and roles

A handle is a `Reader<'f, R>`, a `Writer<'f, R>` or an `Exporter<'f, R>` (later), where
`R` is a role: an uninhabited marker type. A role knows its device and its
class. A role of the compiled layout also knows its span at compile time.

```rust
pub trait Role: Sealed {
    type Device;
    const NAME: &'static str;
}
/// A role whose span comes from the compiled layout.
pub trait Fixed: Role {
    const OFFSET: u32;
    const SIZE: u32;
}
/// A role whose verified contents may be published.
pub trait Publishable: Role {}
```

The internal flash's roles:

| Role              | Span (compiled)                         | Class       | Reader       | Writer                   | Owner                     |
|-------------------|-----------------------------------------|-------------|--------------|--------------------------|---------------------------|
| `Bootloader`      | 0x0 up to the table offset              | publishable | every tier   | never                    | `boot::Bootloader`        |
| `PartitionTable`  | the table's sector, 0x8000–0x9000       | publishable | every tier   | never                    | `boot::PartitionTable`    |
| `Factory`         | `factory`                               | sensitive   | layout, boot | never                    | factory module (later)    |
| `Otadata`         | `otadata`                               | publishable | layout, boot | boot, in `BootWriters`   | `boot::Otadata`; `ota`    |
| `CurrentApp`      | the slot that booted                    | publishable | layout, boot | never                    | `boot::App`               |
| `AlternateApp`    | the other slot                          | publishable | layout, boot | boot, in `BootWriters`   | `boot::App`; `ota`        |
| `CurrentConfig`   | the current slot's configuration        | sensitive   | layout, boot | layout, boot             | configuration store       |
| `AlternateConfig` | the alternate slot's configuration      | sensitive   | layout, boot | layout, boot, erase only | configuration store       |
| `Diag`            | `diag`                                  | sensitive   | layout, boot | layout, boot             | diag store                |
| `Chip`            | the whole flash                         | sensitive   | every tier   | never                    | export service (later)    |

The roles of the unverified tier have their spans set at runtime, from what
was found, and have no `Fixed`:

| Role              | Span                                                            | Class       |
|-------------------|-----------------------------------------------------------------|-------------|
| `FoundTable`      | the sector of the one table found by the search                 | publishable |
| `FoundApp`        | an app entry of the found table, or the image at the booted offset when no table was found | publishable |
| `FoundOtadata`    | the found table's `data`/`ota` entry                            | publishable |

- `reserved` has no role. Space without a role has no handle.
- Each role has at most one reader and one writer per boot. Neither is
  `Clone` or `Copy`. `Reader` and `Writer` constructors are crate-private, and
  `split` takes the device as `&'f mut`, so no handle outlives it or is made
  twice.
- Sensitive roles give their reader and writer to the same owner; storage
  hands them out as one `Store<'f, R>` holding both.
- Functions take the role they need: `fn activate(slot: &Writer<AlternateApp>)`
  can't be given a configuration, the current slot, or a region of another
  device, and `R::SIZE` is a constant a store can assert on.
- The protected spans keep their guard: every erase and program checks its
  absolute range against the bootloader, the table's sector, `factory`,
  `reserved` and the current slot, from the compiled layout, before it reaches
  the device.

## Write state and snapshots

Each region has a write state in the storage crate, one atomic word that
only the `state` module encodes and decodes.

```rust
/// A region's write state. `Epoch` values are opaque: they compare equal,
/// format for an entity tag, and are made only by the storage crate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WriteState {
    /// No write session is open. The region's bytes are those of this epoch.
    Stable(Epoch),
    /// A write session is open. It started from this epoch.
    Writing(Epoch),
}
```

- Inside the `state` module the word is even for `Stable` and odd for
  `Writing`; no other module sees the number. This is a sequence lock
  (seqlock), as used in the Linux kernel for data read far more often than
  written: the writer marks the data busy before it writes and moves it to a
  new value after, and a reader checks the value before and after reading and
  discards what it read if the value changed.
- **Writes happen only inside a session.** `Writer::begin(&mut self)` returns
  a `WriteSession<'_, R>`, which carries `erase` and `program` (and `erase`
  only, for `AlternateConfig`). `Writer` itself only reads. `begin` stores
  `Writing(e)` with release ordering before the session's first operation.
  Dropping the session, on any path (an error, a cancelled future, an
  abandoned update), stores `Stable(e')` with `e' != e` if anything was
  erased or programmed, else `Stable(e)` again. A session borrows the writer
  mutably, so there is one session per region at a time.
- **The writer reads at any time**, inside or outside a session: it is the
  region's only writer.
- **A reader reads only through a snapshot.** `Reader::snapshot(&self, range)`
  returns `Snapshot<'_, R>` holding `Stable(e)`, or `Busy` while the state is
  `Writing`. Every `Snapshot::read`:
  1. loads the state (acquire) and requires `Stable(e)`;
  2. reads the device;
  3. loads the state again and requires `Stable(e)`.

  Otherwise it returns `Stale` and the bytes are discarded.
- A snapshot's reads therefore return bytes of epoch `e` or an error, never a
  mix. The argument: device calls are serialized and the ESP32-C6 has one core
  touching the flash; `begin` stores `Writing` before the first device call of
  a session, and a session that writes anything ends with a new epoch. A read
  whose two loads both see `Stable(e)` either ran entirely outside every
  session or overlapped only sessions that wrote nothing, so its bytes are
  those of epoch `e`. Epochs never return to an earlier value. Epochs grow by one per session in a 31-bit counter, which no
  boot exhausts, and the entity tag adds the random value drawn at boot.
- Compared with the borrow checker's shared-xor-mutable rule: memory safety
  doesn't depend on this, as the flash isn't Rust memory and every device call
  is serialized. What the borrow checker would guarantee at compile time
  (bytes don't change under a reader) is checked at runtime instead, and its
  failure is an error, not wrong data. Readers can be starved by a busy
  writer, which is intended: the writer always wins.
- A content module's parsed model is valid for the epoch it was parsed at. It
  carries that epoch and is invalid once a snapshot at that epoch would fail.
- A content module rereads after a session ends; a module that needs to know
  waits on the region's state through `Reader::changed(&self).await`, which
  wakes after a session ends with a new epoch (replacing the firmware's
  `Writes` signal).

## Publication

Bytes leave the device only through a `Published<'_, R>` view.

```rust
impl<'r, R: Publishable> Snapshot<'r, R> {
    /// The bytes `range` of the snapshot, for export. Refused when the range
    /// overlaps a span the compiled layout gives to sensitive data.
    pub fn publish(self, range: Range<u32>) -> Result<Published<'r, R>, PublishError>;
}

/// A published view: bytes that may leave the device, as of one epoch.
pub trait PublicRead {
    fn len(&self) -> u32;
    fn epoch(&self) -> Epoch;
    fn read(&self, offset: u32, buf: &mut [u8]) -> impl Future<Output = Result<(), ViewError>>;
}
```

Three rules decide what is published; the first two are the storage crate's,
the third the content module's:

1. **Type.** Only `Publishable` roles have `publish`. A configuration, diag,
   factory or `Chip` snapshot has no such method, whatever the tier.
2. **Deny list.** `publish` refuses a range overlapping a span the compiled
   layout gives to sensitive data (`factory`, both configurations, `diag`),
   with absolute offsets, in every tier. The list is compiled in, so nothing
   read from the flash can move it. This is what stops a corrupt or forged
   table from turning a configuration's span into a "found app" and
   publishing it.
3. **Proof.** A content module publishes only bytes its parser verified, and
   in the unverified tier only bytes a cryptographic check covers:
   - an app or bootloader image: its checksum and the SHA-256 it stores,
     which covers every byte before it;
   - a partition table: its MD5, which covers its entries, and the end entry,
     whose bytes the bootloader checks;
   - otadata entries, whose CRC covers only the sequence number: published as
     raw bytes only for the `Otadata` role, at the compiled span; a
     `FoundOtadata` is reported as parsed fields only.

Holding the reader is the permission to publish, so a content module never
hands its reader or its snapshots to anything else; it hands out views and
parsed models.

## Content modules (`eveningstar-boot`)

Each module owns one or more readers and offers parsed models and published
views. A model's constructor is its parser.

| Module           | Owns                                      | Parsed model                                                                                     | Published view                                       |
|------------------|-------------------------------------------|--------------------------------------------------------------------------------------------------|------------------------------------------------------|
| `Bootloader`     | `Reader<Bootloader>`                      | header, descriptor, length, stored digest, file SHA-256, known entry                              | the image                                            |
| `PartitionTable` | `Reader<PartitionTable>`, `Option<Reader<FoundTable>>` | entries, MD5 stored and computed, end, how it was located                           | entries, MD5 entry and end entry                     |
| `Otadata`        | `Reader<Otadata>` or `Reader<FoundOtadata>` | both entries, validity, the active entry, boot state                                           | both 32-byte entries (`Otadata` only)                |
| `App`            | `Reader<CurrentApp>`, `Reader<AlternateApp>`, or `Reader<FoundApp>`s | header, descriptor, length, stored digest, file SHA-256, verification result | the image, once verified                             |

- The app checks run in `App`, as the storage service's check task does now:
  background, paced, with their own watchdog handle fed per chunk, and run
  again after a session on the slot ends.
- `PartitionTable` also reports the expected layout from `layout.rs` in the
  same shape as the found entries (`Entry`: label, type, subtype, offset,
  size, flags), plus the bootloader's and table's regions, so clients compare
  them themselves. The expected rows are plain values, not parsed models.
- The comparison matches entries by label, not by position: each expected
  entry is `matches`, `differs(fields)` or `missing`, and entries with no
  expected label are `extra`. The positional comparison, which reported every
  entry after a missing one as differing, goes.

## Hashes

- **Integrity fields** belong to their format and are parsed facts of a
  model: the SHA-256 an app or bootloader image stores, the table's MD5, each
  otadata entry's CRC, the app descriptor's ELF SHA-256. A client verifies a
  downloaded image or table with them, and the boot chain uses them.
- **Transport digests** are the SHA-256 of a range of a published view,
  computed on request with the SHA accelerator and paced like the checks. A
  digest is returned with the epoch it was computed at; it fails `Stale` like
  a read. One generic function serves every `PublicRead`.
- A digest is published under the same rules as the bytes: no digest of a
  range that couldn't be published. Sensitive regions are hashed only inside
  the firmware (a store's own checksums).
- The file SHA-256 of an app or bootloader image (the one `sha256sum`
  prints) is kept from the verification pass, as now; it costs nothing.

## The verification chain and the tiers

`split` runs once per boot. It never fails: the worst outcome is the
unverified tier, in which the firmware runs on its default configuration
without saving it.

1. **Bootloader.** The ROM loads it from 0x0. Its image is verified (header,
   segments, checksum, stored SHA-256) up to the compiled table offset, and
   its stored digest is looked up in `known.rs`.
2. **Table offset.** A known bootloader's entry gives the offset it reads the
   table from: `TableSource::KnownBootloader`. Otherwise the board is
   searched (`table::find`, as now), and the table counts only as the single
   candidate: `TableSource::Searched`.
3. **Table.** The table at that offset parses as the bootloader parses it
   (`esp_partition_table_verify` in ESP-IDF 5.5): entries, then an MD5 entry
   whose digest matches the entries, then the end entry. The current crate
   ignores what follows the MD5 entry; this tightens it to match the
   bootloader.
4. **Layout.** The entries equal the compiled layout's, field by field.
5. **Flash and booted slot.** The flash is at least the layout's size, and
   the booted offset (MMU entry 0, read by the firmware) is a slot's offset.

| Tier         | Reached when                                                                                    | Hands out                                                                                   |
|--------------|-------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------|
| `Unverified` | step 2 finds no single table, or any of steps 3–5 fails                                          | readers: `Bootloader`, `PartitionTable`, `Chip`, and the `Found*` roles of what was found     |
| `Layout`     | steps 2–5 pass, with the table found by search or a known bootloader without rollback            | + readers of every layout role; `Store`s for the configurations and diag                    |
| `Boot`       | steps 2–5 pass with `TableSource::KnownBootloader`, and the known entry supports rollback        | + `BootWriters { alternate_app: Writer<AlternateApp>, otadata: Writer<Otadata> }`            |

A bootloader that doesn't verify in step 1 is unknown, so step 2 searches.

- `known.rs` entries gain two facts: the table offset the bootloader reads
  and whether it supports rollback. The list still only grows once boards
  have shipped.
- The MD5 adds no new failure from bit rot: the bootloader already refuses a
  table whose MD5 entry doesn't match, whatever the build's configuration
  (`flash_partitions.c` checks it whenever the entry is present), and the
  table is never rewritten. A table without an MD5 entry, which the
  bootloader accepts, drops to the unverified tier; the Nix table always has
  one (`PARTITION_TABLE_MD5` defaults to on).
- The `Found*` readers are cut so that none overlaps a compiled sensitive
  span: a found entry that does gets no reader, and is reported only.
- No writer in the unverified tier. The layout the bootloader boots isn't
  known there, so the protected spans aren't either, and the firmware has
  nothing to save. Recovery is a reflash over USB (`flash-all`).
- No writer for the bootloader or the table in any tier, and no
  administrative write: a raw write would bypass the image checks an update
  passes, and neither region can be rewritten safely across a power cut.
  Updating them remotely is out of scope.
- OTA in the `Layout` tier is refused, not offered behind a warning: with a
  bootloader not known to roll back, an image that fails after an update
  keeps booting, and the board has no reset button. A deliberate override
  (for example confirmed with the user button) is a later decision.

The `Report` records each step: the bootloader's identity and known entry,
the table source and offset, the found tables, the table's parse result and
MD5, the label-matched comparison, the booted offset and slot, the tier, and
the first step that failed. It holds nothing about OTA readiness, which
`eveningstar-ota` reports.

## OTA (`eveningstar-ota`)

`Ota` takes `BootWriters` and the `App` and `Otadata` modules. Its behaviour
is today's (`docs/architecture.md`, "Updates and rollback"); what changes is
where the checks live.

- The descriptor check and the project name, today `Limit::AppDescriptor` and
  the full tier, are `Ota`'s own readiness: it reports `Unavailable` with a
  reason instead of not existing.
- Each `begin_update`, `activate`, `confirm` and `roll_back` writes through a
  session on the writer it needs. An `Update` holds the alternate slot's
  session for its whole life, so no snapshot of the slot can be taken while
  an image is written, and dropping it ends the session and starts the slot's
  check again.
- `VerifiedImage` holds the epoch it was verified at; `activate` refuses it
  once the slot's epoch has moved, and hashes the slot again, as now.
- **T7** is the check, from the first storage spec, that an image booted
  `PendingVerify` can be confirmed. Where it can't (no boot writers), the
  firmware requests a graceful reboot so the bootloader rolls the image back.
  It moves to `eveningstar-ota`, which reads the boot state from the
  `Otadata` module in every tier that has one and tells the firmware to
  reboot.

## Protected paths (later)

Not built here; the shape leaves room for them.

- **Clone export:** the whole chip (`Chip`), for flashing onto another board
  with esptool. It carries the secrets and the factory identity. Writing a
  clone back stays a USB operation, since it rewrites the bootloader and the
  table.
- **Configuration backup and restore:** structured records exported by the
  configuration store, not raw bytes, so a restore goes through the store's
  validation.
- Both need an authorization token from the authentication service, which
  isn't built: `Exporter<Chip>` and the store's export take one. A press of
  the user button as proof of physical presence is an option to decide then.

## What the API gets (later, not part of this work)

The storage view's PRs (#65–#69) are rebuilt on this after the storage
crates land. In outline:

- Each region lists its storage span and, separately, its content: the
  published view's offset, length and epoch, and the parsed model.
- A download serves a published view, whole or by range, with an entity tag
  from the boot's random value, the region and the epoch. `SlotOf` and
  whole-region downloads go.
- A transport digest endpoint hashes any range of a published view.
- `/storage/internal/table` gives the expected and found entries and the
  table source; the positional `layout` field per region goes.

## Order of work

Each step is a PR on a stack, each passing `nix run .#checks`:

1. **Formats crate:** move the pure modules out of `eveningstar-storage`,
   with their tests and fixtures. No behaviour change.
2. **Storage core:** roles, `Reader`, `Writer`, `WriteSession`, `Snapshot`,
   `WriteState`, publication and the deny list, the `NorDevice` trait, and
   `test-support`. Tests for each rule here, including the `compile_fail`
   doctests for the type rules.
3. **Chain and split:** the five steps, the three tiers, the `Found*` roles,
   `known.rs` with table offset and rollback, the label-matched comparison.
4. **`eveningstar-boot`:** the four content modules and the transport digest.
5. **`eveningstar-ota`:** `Ota` on `BootWriters`, with the OTA and otadata
   power-cut tests moved over; T7.
6. **Firmware:** the storage glue without write counting, the check task in
   `App`, the reboot for T7.
7. **Docs:** `docs/architecture.md` ("Storage") rewritten, `CLAUDE.md`
   ("Storage") updated, `specs/storage.md` folded into this file or removed.

## Tests

- Every rule in "Write state and snapshots" and "Publication" has a test: a
  snapshot taken during a session is `Busy`; a read that straddles a session
  is `Stale`; a dropped session, including one dropped mid-erase by
  cancelling its future, leaves a new epoch; a session that wrote nothing
  leaves the old epoch; `publish` refuses each sensitive span.
- `compile_fail` doctests: a sensitive snapshot has no `publish`; `Writer`
  has no `program` outside a session; a `Writer<AlternateApp>` isn't a
  `Writer<CurrentApp>`; no handle is `Clone`.
- The chain's tests reuse today's fixtures and board captures, plus: a table
  without an MD5 entry, a table with a bad MD5, a table followed by garbage
  instead of the end entry, and a found table whose app entry overlaps
  `config_0`.
- The OTA power-cut tests keep running against the simulated bootloader.

## Open questions for review

1. Whether `Factory` gets a reader before anything reads it, or no handle at
   all until the factory module exists.
2. Whether `FoundOtadata` is worth handing out, or the unverified tier
   reports the otadata as raw entries only through the clone export.
3. Whether the formats crate is one crate or three (image, table, otadata).
