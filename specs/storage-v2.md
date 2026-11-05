# Storage, second design: regions, snapshots and content modules

Status: implemented, apart from `eveningstar-system`, which is built on its
own (#74) and isn't part of this stack. How it is built and why is in
`docs/architecture.md` ("Storage"), and the rules for working on it are in
`CLAUDE.md` ("Storage"). This file keeps the design's reasons, the decisions
made while it was built ("Decided during implementation", which take
precedence over the text below where they differ), what is left to build, and
what isn't verified yet.

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
- The table is judged exactly as the bootloader judges and reads it: no check
  is stricter than the bootloader's, and the partitions compared with the
  layout are the ones the bootloader loads.
- Bytes between verified values that nothing reads, such as the table's MD5
  entry padding, are served as `0xFF` and reported as sanitized. Unused space
  elsewhere is cut, not exported. In the unverified tier a table with no MD5
  entry publishes no bytes, and entries after the MD5 entry are cut off.
- A byte may also leave when the parser pins it down: proven by a check,
  a function of proven values, constrained to a known set, or a constant
  (sanitized otherwise). So the otadata's raw entries are published when
  their CRC matches and their state is defined (decided 2026-11-04).
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

### Decided after review (2026-11-04), for the user to confirm

These answer the review's findings and the earlier open questions; the user
was asleep, so they are recorded here to be confirmed or changed.

- `Factory` and `Chip` are declared roles, so their spans are protected and
  known, but no handle is made until a module owns them: a handle nobody owns
  would sit where any code could take it.
- One formats crate, with `image`, `table` and `otadata` modules: they share
  `Source`, `ReadError`, `Text`, hashing and pacing.
- `eveningstar-system` takes reasons as a type parameter the firmware
  declares, and holders and supervised tasks as values registered at
  start-up. `eveningstar-ota` was to request a reboot through an object
  (`&'static dyn RebootRequest<ota::Reason>`), not a type parameter;
  superseded, as it requests none.
- The confirmation window was to run only in the boot tier; superseded by the
  user's decision that it runs in every tier (see "Decided during
  implementation").
- `activate` doesn't hash the slot again: the slot's epoch proves no write
  touched it since it verified, and the bootloader verifies the image at boot
  and falls back if it fails.

### Decided during implementation (2026-11-04), for the user to confirm

The code follows these where the sections below say otherwise.

- **Snapshots and waits.** `Reader::snapshot` also fails `OutOfRange` for a
  span outside the region. `Reader::changed()` completes once the region is
  stable at a state other than the one it was in when `changed` was called:
  a session that was open at the call ends, even without a write, and
  completes it. A device holds at most 16 waiters at once; with more, they
  wake each other in turn until the region changes.
- **Epochs.** A region's epoch is the sum of its spans' counts together with
  which spans they are, so epochs of regions over different spans never
  compare equal. A counter that would wrap leaves its span written to for
  good (every snapshot of it fails) rather than an epoch repeating.
- **The confirmation window** (decided by the user, replacing "no reboot below
  the boot tier"). In every tier, when the running image awaits a
  confirmation, the firmware requests a graceful reboot 10 minutes after boot
  (`crates/eveningstar/src/storage/confirmation.rs`, `Reason::Unconfirmed`,
  the API's `cannot_confirm`). An image awaits one when
  `eveningstar_boot::otadata::awaits_confirmation` says so: it has no boot
  state, or its boot state `needs_confirmation`. That is any state but
  `Valid`, a fallback, no otadata or one that doesn't read, the unverified
  tier without a located otadata, and a board started by `cargo run`. The
  task reads the boot state again when the window ends, so a confirmation from
  any path cancels the reboot. A rollback bootloader turns a `PendingVerify`
  image into `Aborted` at the next boot and starts the previous image; with
  no rollback target the board reboots every 10 minutes, which is accepted
  over a lockout. The task has its own watchdog handle, retired once it is
  done and at boot when no window opens. `EVENINGSTAR_CONFIRM_WINDOW_S` at
  build time shortens the window, as a test aid. The window belongs to the
  firmware, not to `eveningstar-ota`, which requests no reboot and doesn't
  depend on `eveningstar-system`. Nothing calls `Ota::confirm` yet, as no
  endpoint does, so an activated update rolls back after 10 minutes until one
  exists. This replaces "Rolling back an image that can't be confirmed" under
  "OTA" and the confirmation-window bullet above.
- **The layout record.** Every app image embeds the compiled layout right after
  its app descriptor, at offset 0x100 of the first segment's data: a 32-byte
  header (the magic `EVENINGSTAR-LYT1`, the table's offset, the flash size and
  the entry count as little-endian `u32`, four zero bytes) and the entries in
  the ESP-IDF table's 32-byte encoding
  (`eveningstar_esp_formats::image::LayoutRecord`, read by `app_layout`; the
  compiled one is `eveningstar_storage::LAYOUT_RECORD`). The firmware links it
  into `.flash.appdesc.layout`, which esp-hal 1.2.2's `rodata.x` places after
  `.flash.appdesc` in the first output section, and the record's alignment of 1
  leaves no gap after the descriptor. The offset is fixed so that a reader finds
  the record without a symbol table; `app_layout` gives none when the first
  segment ends before the record does, the magic differs, the reserved bytes
  aren't zero or the count exceeds the table's 96 entries. `AppSlot::layout`
  (the `App` module) compares a verified image's record with the compiled one,
  at the model's epoch. `Update::finish` and `Ota::verify_other`, so `roll_back`
  too, refuse an image whose record is missing or differs (`OtaError::Layout`),
  after the project name. `check-images`, in the `build` group, fails when the
  release app lacks the record or it isn't the generated layout. Reason: the
  layout can't change over OTA, and an update built for another layout would
  boot into the unverified tier and could leave a remote board on its default
  configuration. Consequence: after an update from firmware without the record,
  a manual `roll_back` to it is refused; the bootloader's own rollback isn't
  affected.
- **OTA readiness.** `Ota::new` waits for the current slot's check, and OTA
  refuses every write, `confirm` included, until the running image verified
  and its descriptor reads (`Unavailable`). Readiness is judged once. A change
  of a slot's bytes that no session makes, such as flash decay, goes unseen
  until the next session on it or the next boot, at which the bootloader
  verifies the image it starts and falls back when it fails.
- **Located readers.** The unverified tier has one `FoundApp` reader: the app
  partition the bootloader keeps at the booted offset of the located table,
  or, with no valid table, the image at the booted offset that MMU entry 0
  gives. A known bootloader whose table offset isn't the compiled one leaves
  that table without a reader: only the compiled sector and a table found by
  the search have readers.
- **Content modules.** `AppSlot` owns the reader of one app region and keeps
  its check's result per epoch; `App` refers to the tier's slots and runs
  their checks (`App::run`). `AppSlot::verdict(epoch)` is what OTA awaits. The
  bootloader's and the apps' descriptors are read again on request rather
  than kept in the model. Long jobs take a progress hook whose
  `Progress::idle()` is called before each wait for a change only something
  else brings about; the firmware pauses the check task's watchdog there.
- **Failures carry no flash values.** A content module's failure keeps its
  reason without the values it read (`ImageFailure`, `TableFailure`): a
  table the bootloader refuses reports neither its stored nor its computed
  MD5.

## Crates

```
eveningstar-esp-formats   pure: Source, ReadError, Text, image, table, otadata (model and planners), md5, hash, pace
        ↑
eveningstar-storage       devices, roles, write state, handles, snapshots, publication,
        ↑                 layout, known bootloaders, verification chain, split
eveningstar-boot          content modules: bootloader, partition table, otadata, current and alternate app
        ↑
eveningstar-ota           updates, activation, confirmation, rollback; requests no reboot
        ↑
eveningstar               the firmware: the device implementations, the services, the API

eveningstar-system        reboot service, reboot holds, watchdog supervisor (built apart, #74)
```

- **`eveningstar-esp-formats`** reads the ESP formats from any `Source` and
  touches no flash. The otadata planners stay here: they are pure, and they
  describe ESP-IDF's rules. It takes `image.rs`, `table.rs`, `otadata.rs`,
  `source.rs`, `text.rs`, `md5.rs`, `hash.rs` and `pace.rs` with their tests.
  `ReadError` gains `Stale`, so a parser reading through a snapshot can tell
  a region written under it from a failed device read.
- **`eveningstar-storage`** depends on the formats crate for the chain only:
  the bootloader's header and image (its flash size field, length and stored
  digest) and the table's entries and MD5. It exposes `test-support` (the RAM
  flash with per-call hooks, the simulated bootloader, the power-cut harness)
  behind a feature, for the crates above.
- **`eveningstar-boot`** turns readers into views. It holds no writer.
- **`eveningstar-ota`** holds the boot writers and reads through them; it asks
  `eveningstar-boot` for views and verification results of the slots and the
  otadata. It requests no reboot, so it doesn't depend on
  `eveningstar-system`.
- **`eveningstar-system`** takes the reboot service and its holds
  (`services/reboot.rs`) and the watchdog supervisor (`watchdog.rs`) out of
  the firmware. Today their reasons, holders and supervised tasks are closed
  enums naming firmware parts (`Reason::LinkDown`, `Reason::Api`,
  `Holder::…`, the `wd::` markers). In the crate the reasons are a type
  parameter the firmware declares, converting with `From` from each system
  crate's own reasons (`eveningstar_ota::RebootReason`); holders and
  supervised tasks are values registered at start-up (a supervisor with a
  fixed number of slots hands out named handles). The crate holds the logic
  only and builds on the host: the chip's parts (the reset, the main and RTC
  watchdogs) stay in the firmware or the BSP behind a trait the crate takes at
  start-up, and its logging goes through an optional `defmt` feature, as the
  other host crates keep `defmt` out. Its host tests need a time driver for
  `embassy-time` (its `mock-driver` or `std` feature), to be confirmed when
  the crate is made.
- The configuration store, diag store and factory data module own their
  regions later. Each may be its own crate or a firmware module.
- Crate names keep the `eveningstar-` prefix for now; system crates never
  depend on application crates (`morningstar`, the charge-controller service),
  so they can be renamed and moved out later.
- Every crate stays `no_std` and allocation-free, builds on the host, and is
  added to `host_crates` in `nix/checks.nix`.

## Devices

A device is one flash chip. The storage crate defines one trait per kind of
flash and one module per kind: `nor` now, `nand` later. A role names its
device with a marker type (`Internal`); the firmware's implementation of the
device is reached through a trait object, so the storage crate never names
it.

```rust
/// A NOR flash: reads at any alignment, erases 4 KiB sectors, programs
/// 4-byte words, and programming only clears bits.
///
/// Each call is atomic with respect to every other call on the same device,
/// and acts as a compiler and hardware barrier (the internal flash runs each
/// call in a critical section). A power cut during an erase or a program may
/// leave any subset of that call's bits changed, and none of a later call's.
pub trait NorDevice: Sync {
    fn capacity(&self) -> u32;
    fn read(&self, address: u32, buf: &mut [u8]) -> Result<(), DeviceError>;
    fn erase_sector(&self, address: u32) -> Result<(), DeviceError>;
    fn program(&self, address: u32, bytes: &[u8]) -> Result<(), DeviceError>;
}
```

- The trait has no associated constants, so it stays dyn-compatible:
  `nor::SECTOR_SIZE` and `nor::WORD_SIZE` are constants of the module.
- The internal flash is a `NorDevice`. Its implementation (`esp-storage`
  behind a critical-section mutex, as now) stays in the firmware; it no longer
  counts writes, as the write state moves into the storage crate.
- The crate calls a device with ranges inside its capacity, aligned, and at
  most one sector erased or 4 KiB programmed per call, so no call keeps
  interrupts off for longer than one sector erase. Long operations yield
  between calls.
- `.clippy.toml` keeps disallowing `FlashStorage::new` outside the firmware's
  storage glue.

What is shared between kinds of device: roles, epochs and `WriteState`,
snapshot rules, publication rules, and spans. What is per kind: the device
trait, the handle types (`nor::Reader`, `nor::Writer`, `nor::Session`), the
session's operations, the protected spans and the layout check.

The external NAND (GD5F2GM7UE, on SPI2 with the W5500) is out of scope. Its
device trait will be async; it will have 2 KiB pages with a spare area,
128 KiB block erases, bad blocks, ECC status on reads, and one program per
page per erase. Its operations will lock the shared bus, not disable
interrupts, and release the bus between status polls. A future of one of its
operations dropped part way must leave the chip and the bus usable (each
operation is cancel-safe, or the session's drop completes it). Its layout
check will need an on-flash label, since it has no ESP partition table.

## Regions and roles

A handle is a `nor::Reader<'f, R>` or a `nor::Writer<'f, R>` (and later an
exporter), where `R` is a role: an uninhabited marker type. A role knows its
device and its class. A role of the compiled layout also knows its size at
compile time; its offset is known at compile time only when it doesn't depend
on the slot that booted (`Placed`).

```rust
pub trait Role: Sealed {
    /// A marker naming the device, such as `Internal`.
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

Each role below gives its span, its class, its handles and their owner.

- `Bootloader`: 0x0 up to the located table, or up to the compiled table
  offset when none is located. Publishable.
  - Handle: reader, every tier. Owner: `boot::Bootloader`.
- `PartitionTable`: the compiled table's sector, 0x8000–0x9000. Publishable.
  - Handle: reader, every tier. Owner: `boot::PartitionTable`.
- `Otadata`: `otadata`. Publishable.
  - Handle: reader in the layout and boot tiers; writer in the boot tier, in
    `BootWriters`. Owner: `boot::Otadata`; `ota`.
- `CurrentApp`: the slot that booted. Publishable.
  - Handle: reader in the layout and boot tiers. Owner: `boot::App`.
- `AlternateApp`: the other slot. Publishable.
  - Handle: reader in the layout and boot tiers; writer in the boot tier, in
    `BootWriters`. Owner: `boot::App`; `ota`.
- `CurrentConfig`: the current slot's configuration. Sensitive.
  - Handle: writer in the layout and boot tiers. Owner: configuration store.
- `AlternateConfig`: the alternate slot's configuration. Sensitive.
  - Handle: erase-only writer in the layout and boot tiers. Owner:
    configuration store.
- `Diag`: `diag`. Sensitive.
  - Handle: writer in the layout and boot tiers. Owner: diag store.
- `Factory`: `factory`. Sensitive.
  - Handle: none until the factory module exists. Owner: factory module
    (later).
- `Chip`: the whole flash. Sensitive.
  - Handle: none until the export service exists. Owner: export service
    (later).

- `Otadata` is `Publishable` at its compiled offset: its content module
  publishes the entries its parser pins down (see "Publication").
  `FoundOtadata` isn't, as only its location is known, not that it is the
  otadata.
- `PartitionTable`, `Factory`, `Otadata` and `Diag` are `Placed`; the current
  and alternate roles are `LayoutSize` only; `Bootloader` starts at 0x0 but
  its length is set at runtime; `Chip` is neither, as the flash's size is
  read at runtime.
- A writer reads at any time: it is its region's only writer. A sensitive
  role's owner therefore gets only the writer; the stores never need a
  snapshot of their own region.

The unverified tier's roles have spans set at runtime, each located from a
known point:

- `FoundTable` (publishable), located by the search, when it finds exactly one
  table: the sector of that table.
- `FoundApp` (publishable), located by the app partition the bootloader keeps
  at the booted offset of the located table; or, when no valid table is
  located, the image at MMU entry 0 (the booted offset), spanning the image
  its header describes, cut to the flash's capacity.
- `FoundOtadata` (sensitive), located by the located table's `data`/`ota`
  partition, as the bootloader loads them (the last such entry).

- No `Found*` reader is made over a span that overlaps one of the compiled
  layout's sensitive spans (`factory`, both configurations, `diag`); such an
  entry is reported only. This holds for a `FoundApp` located by MMU entry 0
  as for one located by the table.
- `reserved` has no role. Space without a role has no handle.
- Each role has at most one reader and one writer per boot. Neither is
  `Clone` or `Copy`. Handle constructors are crate-private, and `split`
  takes the device and the state table as `&'f mut`, so no handle outlives
  them or is made twice.
- Functions take the role they need: `fn activate(slot: &Writer<AlternateApp>)`
  can't be given a configuration, the current slot, or a region of another
  device, and `R::SIZE` is a constant a store can assert on.
- The protected spans keep their guard: every erase and program checks its
  absolute range against the bootloader's span, the table's sector,
  `factory`, `reserved` and the current slot, from the compiled layout, before
  it reaches the device.

## Write state and snapshots

Each physical span of the compiled layout (the bootloader's span, the table's
sector, each partition) has a write state: one atomic word, held in a state
table that `split` borrows mutably for as long as its handles live
(`nor::State`, one per device, placed in a `static` by the firmware). Every
role maps onto the spans it covers. Only the `state` module encodes and
decodes the words.

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
  a `nor::Session<'_, R>`, which carries `erase` and `program` (and `erase`
  only, for `AlternateConfig`). `Writer` itself only reads. `begin` stores
  `Writing(e)`. Before each device call the session marks itself written;
  a range the guard refuses never reaches the device and marks nothing.
  Dropping the session, on any path (an error, a cancelled future, an
  abandoned update), stores `Stable(e')` with `e' != e` if a device call was
  issued, else `Stable(e)` again. A failed erase or program counts as issued,
  as it may still have changed bits. A session borrows the writer mutably, so
  there is one session per region at a time.
- **A reader reads only through a snapshot.** `Reader::snapshot(&self, range)`
  returns `Snapshot<'_, R>` holding `Stable(e)`, `Busy` while the state is
  `Writing`, or `OutOfRange` for a range outside the region. Every
  `Snapshot::read`:
  1. loads the state and requires `Stable(e)`;
  2. reads the device;
  3. loads the state again and requires `Stable(e)`.

  Otherwise it returns `Stale` and the bytes are discarded. A snapshot
  implements `Source`, so the parsers read through it.
- **Ordering.** The state's loads and stores are `SeqCst`. The argument also
  rests on two facts, which the implementation states where it relies on
  them: every device call is a critical section, which is a compiler and
  hardware barrier, so a device call can't move across a state load or store;
  and the ESP32-C6 has one core reaching the flash. If an interrupt-mode
  executor later runs flash code, the barriers still hold, as they don't
  depend on where tasks interleave.
- A snapshot's reads therefore return bytes of epoch `e` or an error, never a
  mix. `begin` stores `Writing` before the first device call of a session, and
  a session that issued a device call ends with a new epoch. A read whose two
  loads both see `Stable(e)` either ran entirely outside every session or
  overlapped only sessions that issued no device call, so its bytes are those
  of epoch `e`. Epochs never return to an earlier value: each span counts its
  sessions that issued a device call in a 31-bit counter, which no boot
  exhausts and which leaves its span written to for good rather than wrap; a
  region's epoch is the sum of its spans' counts with which spans they are;
  and the entity tag adds the random value drawn at boot.
- Compared with the borrow checker's shared-xor-mutable rule: memory safety
  doesn't depend on this, as the flash isn't Rust memory and every device call
  is serialized. What the borrow checker would guarantee at compile time
  (bytes don't change under a reader) is checked at runtime instead, and its
  failure is an error, not wrong data. Readers can be starved by a busy
  writer, which is intended: the writer always wins.
- **A role covering several spans** (`Chip`, or a `Found*` role crossing the
  compiled layout's spans) has a snapshot that requires every span it
  overlaps to be `Stable`, records each epoch, and checks all of them before
  and after every read, so a clone export taken during an update fails rather
  than returning torn bytes.
- A content module's parsed model carries the epoch it was parsed at, and is
  valid while the region's state is `Stable` at that epoch.
- `Reader::changed(&self).await` completes once the region is stable at a
  state other than the one it was in at the call: after a session on the
  region ends with a new epoch, or after a session open at the call ends.
  Several tasks may wait (the app check, OTA, the API), so it is built on
  `embassy-sync`'s `MultiWakerRegistration`, 16 waiters per device, which
  builds on the host. It replaces the firmware's `Writes` signal.

## Publication

Bytes leave the device only through a `Published<'_, R>` view.

```rust
/// A span inside a region or a view: `Copy`, unlike `Range<u32>`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Span { pub offset: u32, pub len: u32 }

impl<'f, R: Publishable> nor::Reader<'f, R> {
    /// The bytes `span` of the region, for export, as of `epoch`, the epoch
    /// the caller's parser read them at. Refused unless the region's state is
    /// `Stable(epoch)` now. The bytes of each `sanitized` span (relative to
    /// the view, at most `MAX_SANITIZED`) are served as `0xFF` whatever the
    /// flash holds.
    pub fn publish(
        &self,
        epoch: Epoch,
        span: Span,
        sanitized: &[Span],
    ) -> Result<Published<'_, R>, PublishError>;
}

/// A published view: bytes that may leave the device, as of one epoch.
pub trait PublicRead {
    fn len(&self) -> u32;
    fn epoch(&self) -> Epoch;
    /// The view-relative spans served as `0xFF` that hold something else on
    /// the flash.
    fn sanitized(&self) -> &[Span];
    fn read(&self, offset: u32, buf: &mut [u8]) -> impl Future<Output = Result<(), ViewError>>;
}
```

- `publish` takes the epoch of the model the content module parsed, not a
  fresh snapshot, so a view never labels spans computed at one epoch with
  another.
- `PublicRead::read` returns `impl Future`, so the trait isn't
  dyn-compatible; the API holds an enum over the view types, as the storage
  view's `Handle` does today.
- A view stores its sanitized spans in a fixed array with a count.

**Sanitized bytes.** A view may serve bytes that sit between verified values,
which nothing reads or checks, as `0xFF`, the value of erased flash. The view
records every such span whose flash bytes aren't `0xFF` already, and the API
reports them, so a download that differs from a dump of the flash says where
and why; the list reveals only that those bytes weren't erased, never their
values. No digest the device publishes covers a sanitized byte's value on the
flash: a transport digest covers the bytes as served, and a format's own check
(the table's MD5) is published only when it doesn't cover them. A digest over
a few unknown bytes would give them away, as they can be found by trying every
value. Unused space that isn't between verified values, such as the space
after an image, is cut, not sanitized.

A byte is published only when all three allow rules hold. The first two are
enforced by the storage crate, the third by the content module.

1. **Type.** Only `Publishable` roles have `publish`. A configuration, diag,
   factory, otadata or `Chip` handle has no such method, whatever the tier.
2. **Location.** Every `Publishable` role is located from a known point: the
   bootloader at 0x0, where the ROM loads it; the table at the compiled
   offset, which a known bootloader reads, or the table the search finds; a
   partition of the located table; or the image at the booted offset, from
   MMU entry 0. The search is the one scan allowed, and it is bounded: it
   looks only at sector boundaries below the booted offset, and a candidate
   counts only when it is a valid table with an app partition at the booted
   offset and the only such table. No other region is located by scanning
   flash for something that looks like a format, so nothing elsewhere can be
   published.
3. **Verification.** The content module publishes only bytes its parser
   verified with a check that covers each of them:
   - **An image** (app or bootloader): its segments, its checksum, and the
     SHA-256 appended to it, which covers every byte before it. The view is
     the image, the digest included. An image without an appended SHA-256,
     which the ROM accepts on its checksum alone, isn't published: an XOR
     checksum proves nothing about what the bytes are. The space after an
     image in its region is never published.
   - **The partition table**, when it is the located table: the entries, the
     MD5 entry and the end entry, as the bootloader read them. Its entries are
     covered by their equality with the compiled layout in the layout and
     boot tiers, and by the table's MD5 entry in the unverified tier. When the
     end entry directly follows the MD5 entry, the bytes the bootloader
     ignores are sanitized: the MD5 entry's 14 bytes between its magic and its
     digest, and the end entry's 28 bytes after its magic, type and subtype.
     In the unverified tier:
     - entries between the MD5 entry and the end entry, which the bootloader
       accepts but the MD5 doesn't cover, end the view after the MD5 entry,
       whose padding is sanitized;
     - a table with no MD5 entry publishes no bytes and no labels: its offset
       and each partition's type, subtype, offset, size and flags are still
       reported, as they say where regions lie, not what they hold;
     - when the search finds several tables, none is the located table: their
       offsets are reported, and neither their entries nor their bytes.
   - **The otadata**, at its compiled offset: the 32-byte entry of each
     sector, when the parser pins down every byte of it. The CRC proves the
     sequence number (and covers nothing else), the CRC is a function of it,
     the state must be one of the values ESP-IDF defines, and the label,
     which the bootloader ignores and the firmware writes erased, is served
     as `0xFF` and listed as sanitized when it isn't erased. So a published
     entry reveals nothing beyond the parsed fields. A blank entry (all
     `0xFF`) is published as such. An entry whose CRC doesn't match, whose
     state isn't a defined value, or whose sequence number is blank while
     its other bytes aren't, has no view; the rest of each 4 KiB sector is
     never published. The parsed fields (`OtadataFields`) remain: validity,
     and, only when the CRC matches, the sequence number, the slot it
     selects, and a defined state. A located otadata publishes fields only.
   - **Configuration, diag and factory data** are never published raw. Their
     formats (not built yet) give every record a strong integrity check, so
     the parser can tell a record of its own from leftover data. A store may
     publish sanitized fields of a record that verifies, and nothing of one
     that doesn't. A stored digest of secret data is never published, as it
     lets weak secrets be guessed offline; where a digest has to be visible
     to anything outside the store, it is a MAC keyed by a device secret.

As a second check, `publish` also refuses a span that overlaps a span the
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

- `Bootloader` owns `Reader<Bootloader>`.
  - Parsed model: header (with its flash size field), descriptor, length,
    stored digest, file SHA-256, known entry.
  - Published view: the image, when its SHA-256 verifies, known or not.
- `PartitionTable` owns `Reader<PartitionTable>`,
  `Option<Reader<FoundTable>>`.
  - Parsed model: entries, the partitions as the bootloader loads them, MD5
    stored and computed, end, how it was located, other candidates' offsets.
  - Published view: the located table's verified bytes.
- `Otadata` owns `Reader<Otadata>` or `Reader<FoundOtadata>`.
  - Parsed model: both entries, validity, the active entry, boot state.
  - Published view: each entry the parser pins down (compiled only).
- `AppSlot` owns `Reader<CurrentApp>`, `Reader<AlternateApp>`, or
  `Reader<FoundApp>`.
  - Parsed model: header, length, stored digest, file SHA-256, verification
    result, per epoch; the descriptor on request.
  - Published view: the image, once verified.

- The app checks run in `App::run`, over the tier's `AppSlot`s, in the
  firmware's check task: background, paced, with their own watchdog handle
  fed per KiB, and run again after a session on a slot ends
  (`Reader::changed`). `AppSlot` is the one verifier of a slot: OTA awaits
  its result at the slot's new epoch (`AppSlot::verdict`) rather than hashing
  the slot itself.
- `PartitionTable` also reports the expected layout from `layout.rs` in the
  same shape as the found partitions (`Entry`: label, type, subtype, offset,
  size, flags), plus the bootloader's and table's regions, so clients compare
  them themselves. The expected rows are plain values, not parsed models.
- The comparison matches partitions by label, not by position: each expected
  entry is `matches`, `differs(fields)` or `missing`, and partitions with no
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
  a read. One generic function serves every `PublicRead`. Digests share the
  downloads' one-at-a-time lock, so clients can't keep the CPU and the flash
  busy with parallel hashes.
- A digest is published under the same rules as the bytes: no digest of a
  range that couldn't be published. Sensitive regions are hashed only inside
  the firmware (a store's own checks).
- The file SHA-256 of an app or bootloader image (the one `sha256sum`
  prints) is kept from the verification pass, as now; it costs nothing.

## The verification chain and the tiers

`split` runs once per boot. It never fails: the worst outcome is the
unverified tier, in which the firmware runs on its default configuration
without saving it.

1. **Bootloader.** The ROM loads it from 0x0. Its header is read whether or
   not the image verifies: its flash size field is the chip size the
   bootloader checks the table against (an unknown code is 2 MiB, as in
   `bootloader_flash_config_esp32c6.c`). The image is verified (header,
   segments, checksum, appended SHA-256) at the length its headers describe,
   within the flash, and its stored digest is looked up in `known.rs`.
2. **Table offset.** A known bootloader's entry gives the offset it reads the
   table from: `TableSource::KnownBootloader`. Otherwise the board is
   searched (`table::find`), and the table counts only as the single
   candidate: `TableSource::Searched`. The bootloader's image has to end at
   or before the table's offset (or the compiled offset when none is
   located), as ESP-IDF requires by construction; otherwise it is reported
   and not published.
3. **Table.** The table at that offset is valid as the bootloader judges it
   (`esp_partition_table_verify` in ESP-IDF 5.5), no more strictly: entries
   whose offset and end (in wrapping 32-bit arithmetic) don't pass the chip
   size from step 1, at most one MD5 entry, whose digest matches the entries
   before it, and an end entry (magic, type and subtype `0xFF`) that isn't
   the first. The search judges each candidate the same way.
4. **Partitions.** The partitions are the ones the bootloader loads
   (`bootloader_utility_load_partition_table`): the entries from the first up
   to the end entry's index less one if an MD5 entry was found. With the end
   entry directly after the MD5 entry, that is every entry before the MD5
   entry. With entries between them, it includes the MD5 entry read as a
   partition and leaves out the last entry, as the bootloader does; such a
   table then doesn't match the layout, without a check stricter than the
   bootloader's. Of several app partitions of one subtype, or several
   `data`/`ota` partitions, the bootloader keeps the last.
5. **Layout.** The table's offset is the compiled `layout::TABLE_OFFSET`,
   and its partitions equal the compiled layout's, field by field.
6. **Flash and booted slot.** The flash is at least the layout's size, and
   the booted offset (MMU entry 0, read by the firmware) is a slot's offset.

- `Unverified`
  - Reached when: step 2 locates no single table, or any of steps 3–6 fails.
  - Hands out: readers of `Bootloader`, `PartitionTable`, and the `Found*` roles
    of what was located.
- `Layout`
  - Reached when: steps 2–6 pass, with the table found by search or a known
    bootloader without rollback.
  - Hands out: the unverified tier's, and readers of the slots and the
    otadata; writers for the configurations and diag.
- `Boot`
  - Reached when: steps 2–6 pass with `TableSource::KnownBootloader`, and the
    known entry supports rollback.
  - Hands out: the layout tier's, and
    `BootWriters { alternate_app: Writer<AlternateApp>, otadata: Writer<Otadata> }`.

A bootloader that doesn't verify in step 1 is unknown, so step 2 searches.

- `known.rs` entries gain two facts: the table offset the bootloader reads
  and whether it supports rollback. The offset is for the report and for
  refusing; it never relocates a `Placed` role. The list still only grows once
  boards have shipped.
- A known bootloader is recognized by its exact digest, so the flash commands
  must write it byte for byte. esptool rewrites the flash mode, frequency and
  size bytes of the header when told values that differ from the image's (the
  reason ESP-IDF doesn't check the bootloader's SHA-256 at boot). `flash-boot`
  and `flash-all` pass no such options, so esptool keeps the header; that
  stays a rule for the commands. A changed header shows as an unknown
  bootloader and the layout tier, not silently.
- Matching the bootloader's judgement adds no failure from bit rot: a flipped
  bit the bootloader would reject stops the board before the firmware runs,
  and one it ignores is ignored here too. The table is never rewritten. The
  Nix table always has an MD5 entry (`PARTITION_TABLE_MD5` defaults to on),
  and `flash_partitions.c` checks it whenever it is present, whatever the
  build's configuration.
- Image verification reads the app descriptor's `mmu_page_size` (0 means
  64 KiB) for the check of where mapped segments lie, as ESP-IDF 5.5 does on
  the ESP32-C6, instead of assuming 64 KiB.
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

The `Report` records each step: the bootloader's header, identity and known
entry, the table source and offset, the other candidates' offsets, the
table's parse result and MD5, the partitions as the bootloader loads them,
the label-matched comparison, the booted offset and slot, the tier, and the
first step that failed. It holds nothing about OTA readiness, which
`eveningstar-ota` reports.

## OTA (`eveningstar-ota`)

`Ota` takes `BootWriters`, the current and alternate `AppSlot`s and the
`Otadata` module. Its behaviour is the first design's; what changes is where
the checks live.

- The descriptor check and the project name, in the first design
  `Limit::AppDescriptor` and the full tier, are `Ota`'s own readiness: it
  reports `Unavailable` with a reason instead of not existing, and refuses
  every write, `confirm` included, until it is ready.
- Each `begin_update`, `activate`, `confirm` and `roll_back` writes through a
  session on the writer it needs. An `Update` holds the alternate slot's
  session for its whole life, so no snapshot of the slot can be taken while
  an image is written, and dropping it ends the session.
- `Update::finish` programs the last word, ends the session, then awaits
  `App`'s verification of the slot at the new epoch, and checks the length
  written, the project name and the layout record (see "The layout record"
  under "Decided during implementation"). `VerifiedImage` holds that epoch;
  `activate`
  refuses it once the slot's epoch has moved. It doesn't hash the slot again:
  the epoch proves nothing wrote it since, and the bootloader verifies the
  image at boot and falls back if it fails.
- **An image that can't be confirmed** (superseded: see "Decided during
  implementation"). The bootloader starts a newly activated image once in the
  state `PendingVerify` and rolls it back at the next boot unless the firmware
  confirms it. Confirming writes the otadata, which needs the boot writers, so
  it is possible in the boot tier only. `eveningstar-ota` requests no reboot;
  the firmware's confirmation window does, in every tier.
- **The confirmation window** (see "Decided during implementation"): in every
  tier, 10 minutes after a boot whose image awaits a confirmation
  (`awaits_confirmation`), the firmware reads the boot state again and, unless
  the image is `Valid`, requests a graceful reboot (`Reason::Unconfirmed`) in
  every case, including when there is no rollback target, so a board reboots
  every 10 minutes until someone confirms. That is accepted over a lockout. A
  confirmation, which calls `Ota::confirm`, cancels it.
- **The layout record.** `Update::finish` and `Ota::verify_other` (so
  `roll_back` too) refuse an image whose layout record is missing or differs
  from the compiled layout (`OtaError::Layout`); see "Decided during
  implementation".

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
  isn't built: the `Chip` exporter and the store's export take one. A press
  of the user button as proof of physical presence is an option to decide
  then.

## What the API gets (later, not part of this work)

The storage view's PRs (#65–#69) are rebuilt on this after the storage
crates land. In outline:

- Each region lists its storage span and, separately, its content: the
  published view's offset, length, epoch and sanitized spans, and the parsed
  model.
- A download serves a published view, whole or by range, with an entity tag
  from the boot's random value, the region and the epoch. `SlotOf` and
  whole-region downloads go.
- The entity tag is never derived from the contents or a digest of them. The
  boot's random value (64 bits from the hardware TRNG, drawn apart from the
  network stack's seed) only keeps tags of different boots apart, as epochs
  restart at every boot; it is public, and is never used as a key or nonce.
  The epoch in a tag reveals how many write sessions a public region has had
  this boot, which is accepted. Only published views have tags, so the epochs
  of sensitive regions never leave the device.
- A transport digest endpoint hashes any range of a published view.
- `/storage/internal/table` gives the expected and found partitions, the table
  source and the other candidates' offsets; the positional `layout` field per
  region goes.
- The otadata is served as parsed fields, and each entry the parser pins down
  as its 32 raw bytes.

## Order of work

Each step is a PR on a stack, each passing `nix run .#checks`. The stack
starts from `main`; its first PR carries the crate commits of the storage
view's PRs (#67–#69) that it needs (the whole-file SHA-256, pacing and the
hasher trait, the bootloader's length and descriptor without hashing, the
table's MD5), without their API and firmware parts.

1. **Formats crate:** move the pure modules out of `eveningstar-storage`,
   with their tests and fixtures; `ReadError::Stale`. No other behaviour
   change.
2. **Storage core:** roles, `nor::Reader`, `nor::Writer`, `nor::Session`,
   `Snapshot`, `WriteState` and the state table, `Reader::changed`,
   publication and its second check, `Span`, the `NorDevice` trait, and
   `test-support` with the RAM device's per-call hooks. Tests for each rule,
   including the `compile_fail` doctests for the type rules.
3. **Chain and split:** the six steps, the three tiers, the `Found*` roles,
   `known.rs` with table offset and rollback, the table judged and read as
   the bootloader judges and reads it, `mmu_page_size`, the label-matched
   comparison.
4. **`eveningstar-boot`:** the four content modules, sanitized views and the
   transport digest.
5. **`eveningstar-system`:** the reboot service, its holds and the watchdog
   supervisor moved out of the firmware. Built apart from this stack (#74);
   the firmware here still has `services/reboot.rs` and `watchdog.rs`.
6. **`eveningstar-ota`:** `Ota` on `BootWriters`, with the OTA and otadata
   power-cut tests moved over. No reboot in the crate: the confirmation window
   is the firmware's (see "Decided during implementation").
7. **Firmware:** the storage glue without write counting, `App`'s check task.
   The system crate is wired in when it lands.
8. **Docs:** `docs/architecture.md` ("Storage") rewritten, `CLAUDE.md`
   ("Storage") updated, and the first design's `specs/storage.md` folded into
   this file and removed. The reboot and watchdog sections change with the
   system crate. The check labels of the first design are gone from the code.

## Tests

- **Harness.** Tasks interleave only between device calls, at the yields,
  so the snapshot tests need a RAM device that runs a hook on its n-th call:
  the hook opens a session, writes and drops it, through a poll-to-completion
  helper. `test-support` provides it.
- Every rule in "Write state and snapshots" and "Publication" has a test: a
  snapshot taken during a session is `Busy`; a read that straddles a session
  is `Stale`; a dropped session, including one dropped mid-erase by
  cancelling its future, leaves a new epoch; a session whose calls the guard
  refused leaves the old epoch; `publish` refuses an epoch that moved; the
  second check refuses each sensitive span.
- `compile_fail` doctests: a sensitive or located-otadata handle has no
  `publish`; `Writer` has no `program` outside a session; a
  `Writer<AlternateApp>` isn't a `Writer<CurrentApp>`; no handle is `Clone`.
- Publication: an app slot publishes its image and not the bytes after it; an
  image without an appended SHA-256 publishes nothing; a found app partition
  over leftover data whose SHA-256 doesn't verify publishes nothing; a
  table's ignored bytes that aren't `0xFF` are served as `0xFF` and listed as
  sanitized, and ones that are `0xFF` aren't listed; several candidate tables
  publish nothing; an otadata entry publishes its 32 bytes when blank or
  pinned down, with a non-erased label sanitized, and nothing when its CRC
  or state doesn't verify; no read reaches past an entry into its sector.
- The chain: today's fixtures and board captures, plus a table without an MD5
  entry, a table with a bad MD5, entries between the MD5 entry and the end
  entry (the MD5 entry read as a partition, the last entry left out), an end
  entry whose last 28 bytes aren't `0xFF`, `[MD5, end]` with no partitions,
  duplicate `data`/`ota` entries, a partition past the header's flash size
  but inside the chip, a bootloader image running past the table offset, and
  a table at another offset under a known bootloader.
- A clone export snapshot fails when any span it covers enters a write
  session.
- The OTA power-cut tests keep running against the simulated bootloader.
- **On the board**, after each step that changes what runs on it: the tiers
  from `flash-all` and from `cargo run`; tables written over USB at 0x8000
  (missing, extra, shifted and overlapping partitions, no MD5 entry, entries
  after the MD5 entry); regions read back over USB (`esptool read-flash`)
  and compared with what the firmware reports and publishes; leftover data
  written past an image in a slot over USB, and shown never to be published;
  an update written and activated through the firmware, checked over USB, and
  its rollback. The board is restored to the `flash-all` baseline after.
- **Corrupt images on the board.** The ESP-IDF bootloader skips an image's
  checksum and SHA-256 while a debugger is attached
  (`esp_cpu_dbgr_is_attached()` in
  `components/bootloader_support/src/esp_image_format.c`), so with probe-rs
  attached during a boot it starts a corrupt image. A board test of image
  corruption attaches a few seconds after the reset; the RTT buffer keeps the
  early boot lines.

## Left to build

- **Layout migration.** The ESP table stays as it is; dividing up `reserved`
  happens inside that partition: a firmware-owned, versioned header at its
  start and roles over its sub-regions, so the table and the layout record
  don't change and old and new firmware stay compatible. Changing the ESP
  table itself needs a power-safe way to rewrite the table sector, which
  doesn't exist yet (one sector, no backup copy). If one is built, the
  migration reaches a board in two updates, as the running firmware decides
  what it accepts: first a firmware with the same layout that knows the
  migration and accepts the specific next layout's record, then the
  firmware with the new layout.
- **Configuration store.** It owns `Writer<CurrentConfig>` and keeps the
  running slot's settings, secrets included, in 128 KiB, through its session
  as an `embedded-storage-async` NOR flash. It reads the alternate slot's
  configuration through `Writer<AlternateConfig>`, which reads at any time and
  whose sessions erase only, to migrate after an update, and erases it once it
  has migrated. Whether the store
  accepts records zeroed in place (NOR programs only clear bits) is open. Its
  format gives every record a strong integrity check (see "Publication").
- **Diag store.** It owns `Writer<Diag>`. Crash records are backtraces, not
  memory, and events such as updates and reboots, likely as a priority ring
  buffer. Capturing a crash (likely an LP RAM record copied to `diag` on the
  next boot) was deferred.
- **Factory data module.** It would own `Factory`, which has no handle until
  it exists; provisioning the partition is out of scope until the user asks.
- **OTA service.** It owns `Ota` (built at boot in the boot tier, in a
  `static`, and taken by nothing yet) and decides how updates arrive and are
  authenticated: `VerifiedImage` means intact and meant for this board, not
  authentic.
- **The confirm endpoint.** Nothing calls `Ota::confirm` yet, as the API has
  no endpoint for it. The confirmation window ("OTA") reboots a board 10
  minutes after a boot whose image awaits a confirmation, so until an
  endpoint exists, an activated update rolls back after 10 minutes.
- **The API's view and the web UI** (see "What the API gets"): the regions,
  the published views and their digests, the report (tier, the step that
  failed, the table comparison, the bootloader, the slot booted), OTA's
  readiness and the boot state, and the reason for a reboot. The storage UI
  stack (#65–#69, `specs/storage-ui.md`) is rebuilt on this.
- **Reporting the unverified tier.** The firmware runs on its default
  configuration, which it doesn't save, and logs why; the API and the status
  LED don't say so yet.
- **The external NAND** (see "Devices"). It will hold bulk data logs.
- **Protected paths** (above): clone export and configuration backup, once
  authentication exists.

Out of scope until the user asks: updating the bootloader or the partition
table from the firmware, flash encryption and secure boot (the build forbids
both), image signatures, and factory provisioning of the `factory` partition.

## Not verified yet

Verified on the dev board with this design: the boot tier after `flash-all`
(`split` about 17 ms, the content modules 12 ms, the checks of both slots
about 3.2 s in the background, the device's SHA-256s equal to the host's); 15
tables written at 0x8000 over USB, each giving the tier and the located
readers the bootloader's judgement and the layout call for, and those the
bootloader refuses never booting until the table is rewritten over USB;
leftover bytes after an image never published; a real update through `Ota`
(on a throwaway branch that copies the running image): rollback
(`PendingVerify`, then `Aborted`), confirm (lasting across reboots), and a
chip reset mid-copy (the running slot boots, the half-written slot fails its
check on the checksum); the bootloader's fallback from a corrupt selected
image (one byte flipped in `ota_1`, selected as `New`: `ota_0` starts, the
firmware logs the fallback and that the boot state needs confirming, and the
alternate slot's check fails on the checksum); the confirmation window, built
with `EVENINGSTAR_CONFIRM_WINDOW_S=60` (a confirmed image retires the
window's watchdog handle at boot; a `New` entry booted as `PendingVerify`
rebooted after the 60 s and the bootloader marked it `Aborted`; with no
rollback target, the same image then rebooted every window); and a corrupt
image started with a debugger attached, which the firmware's check fails and
OTA refuses (`Descriptor(Checksum)`). From the first design: the booted offset
for both slots and the first boot after `flash-all` writing otadata entry 0
with sequence 1 and `Valid`.

Not verified:

- A true power cut, not a chip reset, during an update, an activation and a
  confirmation. The power-cut tests run on a RAM flash and a simulated
  bootloader.
- The unverified tier under `cargo run` (espflash's bootloader and table, the
  search) with this design; the host tests cover it.
- The confirmation window's cancel path on hardware: an image confirmed
  through `Ota::confirm` after the window opened, which the task's second read
  of the boot state sees. The host tests cover `awaits_confirmation`, and none
  of the board runs confirmed an image inside a window.
- The interrupt latency of sector erases next to Ethernet traffic.
- The stack high-water mark with `esp-storage`'s 4 KiB read buffer.
- The path where MMU entry 0 is invalid, so the booted offset is unknown and
  the tier is unverified.
