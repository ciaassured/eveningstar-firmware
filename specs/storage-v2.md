# Storage, second design: regions, snapshots and content modules

Status: proposal, for review. Nothing here is built yet. It replaces the
storage layer described in `docs/architecture.md` ("Storage") and
`specs/storage.md`; once it is implemented, both are rewritten to match and
`CLAUDE.md` ("Storage") is updated with it.

> **Publication is an allow list.** A raw region read is never exported.
> Bytes leave the device only through a published view, and a view exists
> only over bytes that were located from a known point (the bootloader at
> 0x0, the table the chain locates, an entry of that table, the image at the
> booted offset) and that a parser verified with a check covering every one of
> them. Bytes nothing verifies, such as the space after an image in its slot,
> never leave. Don't relax this, add a route from a raw reader to a response,
> or hand out a writer outside the tiers below without asking the user.

## Why

The first design is sound in its layout gate, its role types and its otadata
writes, and those carry over. Four things change:

1. **Public meant the whole region.** Every public handle read, hashed and
   downloaded its whole span: the bootloader's 0x0–0x8000 around an image of
   about 22 KiB, the table's 0xC00 bytes, the otadata's 8 KiB around two
   32-byte entries, and each 3 MiB app slot. Neither an update nor esptool
   erases past the image, so those spans hold whatever was there before,
   possibly data from an earlier layout.
2. **OTA lived in the storage crate:** the update typestate, the otadata
   planners, image verification, the full tier (which existed to gate `Ota`),
   the descriptor and project checks, and the reboot that rolls back an image
   that can't be confirmed.
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
- Publication is an allow list, not a deny list: located from a known point,
  and verified by a check that covers every published byte. A compiled deny
  list of the sensitive spans stays as a second check only.
- Formats that can hold secrets (configuration, diag, factory data) get a
  strong integrity check per record, so their parsers can tell a record from
  leftover data before exposing anything, and expose only sanitized fields.
- Reading and writing run concurrently. Readers read through snapshots, which
  fail once the region is written; the writer always wins. While a write
  session is open, no snapshot of the region can be taken.
- The write state is a type, not an even or odd number checked in several
  places.
- The table is published only when it is certain to be the table, and the
  only one. It is reported beside the expected layout, in the same shape,
  with how it was located.
- The table is judged exactly as the bootloader judges it: no check of the
  MD5 or of the entries is stricter than the bootloader's.
- Bytes between verified values that nothing reads, such as the table's MD5
  entry padding, are served as `0xFF` and reported as sanitized. Unused space
  elsewhere is cut, not exported. In the unverified tier a table with no MD5
  entry publishes no bytes, and entries after the MD5 entry are cut off.
- A bootloader image with a verifying SHA-256 is published whether or not it
  is a known one; being known only decides the tier.
- The boot writers (alternate slot and otadata) are handed out only at the
  end of the verification chain: a known bootloader that supports rollback, so
  the table's offset is certain, then the table and the layout.
- No writes in the unverified tier. Recovery is a reflash over USB.
- No whole-slot downloads. No zeroize capability: wiping is an erase by the
  region's owner.
- Hashes are two separate things: integrity fields that formats store (app and
  bootloader SHA-256, table MD5, otadata CRC), and transport digests over
  any public range a client asks for, computed on request.
- Secrets may leave the device later through protected paths: a configuration
  backup, and a clone export of the whole chip. Both need authentication,
  which isn't built.
- Separate crates. The storage, OTA, reboot, watchdog and other system
  components are meant to be reusable by other firmware, so they don't depend
  on the application.
- Code comments, docs and messages describe each check by what it does; the
  check labels of the first spec (T1–T7, O3–O8) aren't used outside specs.

## Crates

```
eveningstar-esp-formats   pure: Source, ReadError, Text, image, table, otadata (model and planners), md5, hash, pace
        ↑
eveningstar-storage       devices, roles, write state, Reader/Writer/WriteSession/Snapshot, publication,
        ↑                 layout, known bootloaders, verification chain, split
eveningstar-boot          content modules: bootloader, partition table, otadata, current and alternate app
        ↑
eveningstar-ota  ──→  eveningstar-system   reboot service, reboot holds, watchdog supervisor
        ↑                   ↑
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
  `eveningstar-boot` for views of the slots and the otadata, and requests
  reboots through `eveningstar-system`.
- **`eveningstar-system`** takes the reboot service and its holds
  (`services/reboot.rs`) and the watchdog supervisor (`watchdog.rs`) out of
  the firmware. Today their reasons, holders and supervised tasks are closed
  enums naming firmware parts (`Reason::LinkDown`, `Reason::Api`,
  `Holder::…`, the `wd::` markers). In the crate they are declared by the
  firmware: the reboot service is generic over a reason type, each system
  crate defines its own reasons (`eveningstar_ota::RebootReason`), and the
  firmware's reason type converts from them with `From`. The crate holds the
  logic only and builds on the host: the chip's parts (the reset, the main and
  RTC watchdogs) stay in the firmware or the BSP behind a trait the crate
  takes at start-up, and its logging goes through an optional `defmt`
  feature, as the other host crates keep `defmt` out. Its host tests need a
  time driver for `embassy-time` (its `mock-driver` or `std` feature), to be
  confirmed when the crate is made. `eveningstar-ota` depends on it, so OTA
  stays buildable and testable on the host.
- The configuration store, diag store and factory data module own their
  regions later. Each may be its own crate or a firmware module.
- Crate names keep the `eveningstar-` prefix for now; system crates never
  depend on application crates (`morningstar`, the charge-controller service),
  so they can be renamed and moved out later.
- Every crate stays `no_std` and allocation-free, builds on the host, and is
  added to `host_crates` in `nix/checks.nix`.

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

A handle is a `Reader<'f, R>`, a `Writer<'f, R>` or an `Exporter<'f, R>`
(later), where `R` is a role: an uninhabited marker type. A role knows its
device and its class. A role of the compiled layout also knows its size at
compile time; its offset is known at compile time only when it doesn't depend
on the slot that booted (`Placed`).

```rust
pub trait Role: Sealed {
    type Device;
    const NAME: &'static str;
}
/// A role whose size comes from the compiled layout. The current and
/// alternate roles have one, though their offset depends on the slot booted.
pub trait LayoutSize: Role {
    const SIZE: u32;
}
/// A role at a fixed offset of the compiled layout, whatever slot booted.
pub trait Placed: LayoutSize {
    const OFFSET: u32;
}
/// A role located from a known point, whose verified contents may be
/// published.
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

`Bootloader`, `PartitionTable`, `Factory`, `Otadata` and `Diag` are `Placed`;
the current and alternate roles are `LayoutSize` only; `Chip` is neither, as
the flash's size is read at runtime.

The unverified tier's roles have spans set at runtime, each located from a
known point, and are neither `Placed` nor `LayoutSize`:

| Role           | Located by                                                                                  | Class       |
|----------------|---------------------------------------------------------------------------------------------|-------------|
| `FoundTable`   | the search, when it finds exactly one table (the sector of that table)                      | publishable |
| `FoundApp`     | an app entry of the located table; or MMU entry 0 (the booted offset) when no table is located, spanning the image its header describes | publishable |
| `FoundOtadata` | the located table's `data`/`ota` entry                                                      | publishable |

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
  abandoned update), stores `Stable(e')` with `e' != e` if any erase or
  program was attempted, else `Stable(e)` again. The session marks itself
  written before each device call, not after it succeeds, as a failed erase
  or program may still have changed bits. A session borrows the writer
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
  those of epoch `e`. Epochs never return to an earlier value: they grow by
  one per session in a 31-bit counter, which no boot exhausts, and the entity
  tag adds the random value drawn at boot.
- Compared with the borrow checker's shared-xor-mutable rule: memory safety
  doesn't depend on this, as the flash isn't Rust memory and every device call
  is serialized. What the borrow checker would guarantee at compile time
  (bytes don't change under a reader) is checked at runtime instead, and its
  failure is an error, not wrong data. Readers can be starved by a busy
  writer, which is intended: the writer always wins.
- **A reader whose span covers several regions** has no write state of its
  own. `Chip` is one, and so is a `Found*` role whose span crosses regions of
  the compiled layout. Its snapshot requires every region it overlaps to be
  `Stable`, records each of their epochs, and checks all of them before and
  after every read, so a clone export taken during an update fails rather
  than returning torn bytes.
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
    /// The bytes `range` of the snapshot, for export, with the bytes of each
    /// `erased` range (inside `range`, at most `MAX_ERASED`) served as `0xFF`
    /// whatever the flash holds.
    pub fn publish(
        self,
        range: Range<u32>,
        erased: &[Range<u32>],
    ) -> Result<Published<'r, R>, PublishError>;
}

/// A published view: bytes that may leave the device, as of one epoch.
pub trait PublicRead {
    fn len(&self) -> u32;
    fn epoch(&self) -> Epoch;
    /// The ranges served as `0xFF` that hold something else on the flash.
    fn sanitized(&self) -> &[Range<u32>];
    fn read(&self, offset: u32, buf: &mut [u8]) -> impl Future<Output = Result<(), ViewError>>;
}
```

**Sanitized bytes.** A view may serve bytes that sit between verified values,
which nothing reads or checks, as `0xFF`, the value of erased flash. The view
records every such range whose flash bytes aren't `0xFF` already
(`sanitized`), and the API reports them, so a download that differs from a
dump of the flash says where and why; the list reveals only that those bytes
weren't erased, never their values. No digest the device publishes covers a
sanitized byte's value on the flash: a transport digest covers the bytes as
served, and a format's own check (the table's MD5) is published only when it
doesn't cover them. A digest over a few unknown bytes would give them away, as
they can be found by trying every value. Unused space that isn't between verified values, such as the space
after an image, is cut, not sanitized.

A byte is published only when all three allow rules hold. The first two are
enforced by the storage crate, the third by the content module.

1. **Type.** Only `Publishable` roles have `publish`. A configuration, diag,
   factory or `Chip` snapshot has no such method, whatever the tier.
2. **Location.** Every `Publishable` role is located from a known point: the
   bootloader at 0x0, where the ROM loads it; the table at the offset a known
   bootloader reads, or the table the search finds; an entry of the located
   table; or the image at the booted offset, from MMU entry 0. The search is
   the one scan allowed, and it is bounded: it looks only at sector
   boundaries below the booted offset, and a candidate counts only when it
   is a valid table with an app entry at the booted offset and the only such
   table. No other region is located by scanning flash for something that
   looks like a format, so nothing elsewhere can be published.
3. **Verification.** The content module publishes only bytes its parser
   verified with a check that covers each of them:
   - **An image** (app or bootloader): its segments, its checksum, and the
     SHA-256 appended to it, which covers every byte before it. The view is
     the image, the digest included. An image without an appended SHA-256,
     which the ROM accepts on its checksum alone, isn't published: an XOR
     checksum proves nothing about what the bytes are. The space after an
     image in its region is never published.
   - **The partition table**, when it is the located table: the entries,
     the MD5 entry and the end entry, as the bootloader read them. Its entries
     are covered by their equality with the compiled layout in the layout and
     boot tiers, and by the table's MD5 entry in the unverified tier. The
     bytes the bootloader ignores are sanitized: the MD5 entry's 14 bytes
     between its magic and its digest, and the end entry's 28 bytes after its
     magic, type and subtype. In the unverified tier, entries after the MD5
     entry, which the bootloader accepts but the MD5 doesn't cover, end the
     view after the MD5 entry, and a table with no MD5 entry publishes no
     bytes and no labels: its offset and each entry's type, subtype, offset,
     size and flags are still reported, as they say where regions lie, not
     what they hold. When the search finds several tables, none is the located
     table: their offsets are reported, and neither their entries nor their
     bytes.
   - **The otadata**: parsed fields only, in every tier. For each entry: its
     validity (blank, valid, corrupt, invalid, aborted), its sequence number
     and the slot it selects when its CRC matches, and its state when the
     state is a value ESP-IDF defines. The CRC covers only the sequence
     number, so the 20-byte label and an unknown state value are never
     published, nor any raw byte.
   - **Configuration, diag and factory data** are never published raw. Their
     formats (not built yet) give every record a strong integrity check, so
     the parser can tell a record of its own from leftover data. A store may
     publish sanitized fields of a record that verifies, and nothing of one
     that doesn't. A stored digest of secret data is never published, as it
     lets weak secrets be guessed offline; where a digest has to be visible
     to anything outside the store, it is a MAC keyed by a device secret.

As a second check, `publish` also refuses a range that overlaps a span the
compiled layout gives to sensitive data (`factory`, both configurations,
`diag`), in every tier. It catches a parser that verifies something it
shouldn't at the spans this firmware writes secrets to. It doesn't cover
secrets left elsewhere by another layout, which is why the allow rules above
are the ones relied on.

Holding the reader is the permission to publish, so a content module never
hands its reader or its snapshots to anything else; it hands out views and
parsed models.

## Content modules (`eveningstar-boot`)

Each module owns one or more readers and offers parsed models and published
views. A model's constructor is its parser.

| Module           | Owns                                      | Parsed model                                                                                     | Published view                                         |
|------------------|-------------------------------------------|--------------------------------------------------------------------------------------------------|--------------------------------------------------------|
| `Bootloader`     | `Reader<Bootloader>`                      | header, descriptor, length, stored digest, file SHA-256, known entry                              | the image, when its SHA-256 verifies, known or not     |
| `PartitionTable` | `Reader<PartitionTable>`, `Option<Reader<FoundTable>>` | entries, MD5 stored and computed, end, how it was located, other candidates' offsets | the located table's verified bytes                     |
| `Otadata`        | `Reader<Otadata>` or `Reader<FoundOtadata>` | both entries, validity, the active entry, boot state                                           | none; parsed fields only                               |
| `App`            | `Reader<CurrentApp>`, `Reader<AlternateApp>`, or `Reader<FoundApp>`s | header, descriptor, length, stored digest, file SHA-256, verification result | the image, once verified                               |

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
  the firmware (a store's own checks).
- The file SHA-256 of an app or bootloader image (the one `sha256sum`
  prints) is kept from the verification pass, as now; it costs nothing.

## The verification chain and the tiers

`split` runs once per boot. It never fails: the worst outcome is the
unverified tier, in which the firmware runs on its default configuration
without saving it.

1. **Bootloader.** The ROM loads it from 0x0. Its image is verified (header,
   segments, checksum, appended SHA-256) up to the compiled table offset. The
   image's length comes from its header and segment headers. Its stored digest
   is looked up in `known.rs`.
2. **Table offset.** A known bootloader's entry gives the offset it reads the
   table from: `TableSource::KnownBootloader`. Otherwise the board is
   searched (`table::find`, as now), and the table counts only as the single
   candidate: `TableSource::Searched`.
3. **Table.** The table at that offset is valid as the bootloader judges it
   (`esp_partition_table_verify` in ESP-IDF 5.5), no more strictly: entries
   whose spans lie inside the flash, at most one MD5 entry, whose digest
   matches the entries before it, and an end entry (magic, type and subtype
   `0xFF`) that isn't the first. The search judges each candidate the same
   way.
4. **Layout.** The entries equal the compiled layout's, field by field.
5. **Flash and booted slot.** The flash is at least the layout's size, and
   the booted offset (MMU entry 0, read by the firmware) is a slot's offset.

| Tier         | Reached when                                                                                    | Hands out                                                                                   |
|--------------|-------------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------|
| `Unverified` | step 2 locates no single table, or any of steps 3–5 fails                                        | readers: `Bootloader`, `PartitionTable`, `Chip`, and the `Found*` roles of what was located   |
| `Layout`     | steps 2–5 pass, with the table found by search or a known bootloader without rollback            | + readers of every layout role; `Store`s for the configurations and diag                    |
| `Boot`       | steps 2–5 pass with `TableSource::KnownBootloader`, and the known entry supports rollback        | + `BootWriters { alternate_app: Writer<AlternateApp>, otadata: Writer<Otadata> }`            |

A bootloader that doesn't verify in step 1 is unknown, so step 2 searches.

- `known.rs` entries gain two facts: the table offset the bootloader reads
  and whether it supports rollback. The list still only grows once boards
  have shipped.
- Matching the bootloader's judgement adds no failure from bit rot: a flipped
  bit the bootloader would reject stops the board before the firmware runs,
  and one it ignores is ignored here too. The table is never rewritten. The
  Nix table always has an MD5 entry (`PARTITION_TABLE_MD5` defaults to on),
  and `flash_partitions.c` checks it whenever it is present, whatever the
  build's configuration.
- The `Found*` readers aren't handed out over a compiled sensitive span: a
  found entry that overlaps one gets no reader, and is reported only.
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
the table source and offset, the other candidates' offsets, the table's
parse result and MD5, the label-matched comparison, the booted offset and
slot, the tier, and the first step that failed. It holds nothing about OTA
readiness, which `eveningstar-ota` reports.

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
- `Update::finish` programs the last word, ends the session, then verifies
  the slot at the new stable epoch. `VerifiedImage` holds that epoch;
  `activate` refuses it once the slot's epoch has moved, and hashes the slot
  again, as now.
- **Rolling back an image that can't be confirmed.** The bootloader starts a
  newly activated image once in the state `PendingVerify` and rolls it back at
  the next boot unless the firmware confirms it. Confirming writes the
  otadata, which needs the boot writers. When the firmware boots such an image
  without them (the layout tier), it requests a graceful reboot, so the
  bootloader rolls the image back instead of leaving it running unconfirmed.
  This moves from the storage glue to `eveningstar-ota`, which reads the boot
  state from the `Otadata` module in every tier that has one and requests the
  reboot through `eveningstar-system` with its own reason, which the
  firmware's reason type converts from.
- **The confirmation window**, the OTA service's policy, built on the same
  reboot handle: after a boot whose image needs confirming
  (`BootState::needs_confirmation`), a user has 10 minutes to confirm it,
  which calls `Ota::confirm`. On timeout the service requests a graceful
  reboot in every case, including when there is no rollback target, so a
  board reboots every 10 minutes until someone confirms. That is accepted so
  that a user can confirm whichever image comes up.

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
- `/storage/internal/table` gives the expected and found entries, the table
  source and the other candidates' offsets; the positional `layout` field per
  region goes.
- The otadata is served as parsed fields, not bytes.

## Order of work

Each step is a PR on a stack, each passing `nix run .#checks`:

1. **Formats crate:** move the pure modules out of `eveningstar-storage`,
   with their tests and fixtures. No behaviour change.
2. **Storage core:** roles, `Reader`, `Writer`, `WriteSession`, `Snapshot`,
   `WriteState`, publication and its second check, the `NorDevice` trait, and
   `test-support`. Tests for each rule here, including the `compile_fail`
   doctests for the type rules.
3. **Chain and split:** the five steps, the three tiers, the `Found*` roles,
   `known.rs` with table offset and rollback, the table judged as the
   bootloader judges it, the label-matched comparison.
4. **`eveningstar-boot`:** the four content modules and the transport digest.
5. **`eveningstar-system`:** the reboot service, its holds and the watchdog
   supervisor moved out of the firmware, generic over the firmware's reasons,
   holders and tasks, with the chip's reset and hardware watchdogs behind a
   trait the firmware implements.
6. **`eveningstar-ota`:** `Ota` on `BootWriters`, with the OTA and otadata
   power-cut tests moved over; the reboot that rolls back an image that can't
   be confirmed.
7. **Firmware:** the storage glue without write counting, the check task in
   `App`, the system crate wired in.
8. **Docs:** `docs/architecture.md` ("Storage", and the reboot and watchdog
   sections) rewritten, `CLAUDE.md` ("Storage") updated, `specs/storage.md`
   folded into this file or removed. The check labels (`(T1)`…`(T7)`,
   `(O3)`…`(O8)`) leave the code's comments in the steps that touch them.

## Tests

- Every rule in "Write state and snapshots" and "Publication" has a test: a
  snapshot taken during a session is `Busy`; a read that straddles a session
  is `Stale`; a dropped session, including one dropped mid-erase by
  cancelling its future, leaves a new epoch; a session that wrote nothing
  leaves the old epoch; the second check refuses each sensitive span.
- `compile_fail` doctests: a sensitive snapshot has no `publish`; `Writer`
  has no `program` outside a session; a `Writer<AlternateApp>` isn't a
  `Writer<CurrentApp>`; no handle is `Clone`.
- Publication: an app slot publishes its image and not the bytes after it; an
  image without an appended SHA-256 publishes nothing; a found app entry over
  leftover data whose SHA-256 doesn't verify publishes nothing; a table's
  ignored bytes that aren't `0xFF` are served as `0xFF` and listed as
  sanitized, and ones that are `0xFF` aren't listed; several candidate tables
  publish nothing; the otadata publishes no label and no unknown state value.
- The chain: today's fixtures and board captures, plus a table without an MD5
  entry, a table with a bad MD5, entries after the MD5 entry, and an end entry
  whose last 28 bytes aren't `0xFF`, each judged as the bootloader judges it.
- A clone export snapshot fails when any region it covers enters a write
  session.
- The OTA power-cut tests keep running against the simulated bootloader.

## Open questions for review

1. Whether `Factory` gets a reader before anything reads it, or no handle at
   all until the factory module exists.
2. Whether the formats crate is one crate or three (image, table, otadata).
3. Whether `eveningstar-system` takes the reasons, holders and supervised
   tasks as type parameters, or as values registered at start-up.
