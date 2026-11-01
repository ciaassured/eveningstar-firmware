# Storage: the internal flash

Status: specified, under review. Nothing is implemented yet. When it lands, the
durable design moves to `docs/architecture.md` ("Storage") and this file keeps
only what is left and what hasn't been verified, like `specs/web-ui.md`.

Scope: the ESP32-C6 module's internal SPI NOR flash (8 MiB). The external SPI
NAND (`Board::flash`) is out of scope; it will hold bulk data logs later.

## Goal

A storage layer that later features (configuration, diagnostics, OTA updates
and rollback, a layout view in the web UI) build on, and that makes the
dangerous mistakes hard or impossible to write:

- writing the running app's slot, the bootloader or the partition table;
- writing configuration in space the board's actual layout gives to something
  else;
- marking an image bootable that wasn't fully written and verified;
- sending secrets off the device by passing the wrong handle to a function that
  exports data.

Storage never stops the device from booting. When the flash isn't laid out the
way the firmware expects, it hands out fewer handles and says why.

## Shape

| Part | Where | Holds |
|---|---|---|
| Layout | `nix/board.nix`, generated into `eveningstar-storage/src/layout.rs` | every partition's label, type, subtype, offset, size, flags |
| Split and trust | `eveningstar-storage` (`partitions`) | the table and bootloader checks, the tiers, the handles |
| Formats | `eveningstar-storage` (`table`, `image`, `otadata`) | parsing and verifying the partition table, ESP images and otadata |
| OTA mechanism | `eveningstar-storage` (`ota`) | the update writer, `VerifiedImage`, activation, confirm, rollback |
| Glue | `eveningstar::storage` | `Flash` over `esp_storage::FlashStorage`, the booted offset, the boot log |
| Policy (later) | firmware services | config store, diag store, OTA service, API endpoints |

`eveningstar-storage` is `no_std`, has no Espressif dependencies, builds and
tests on the host (it joins `host_crates` in `nix/checks.nix`), and talks to
the chip through a small `Flash` trait. The OTA mechanism is a module of this
crate rather than its own crate so that raw otadata writes stay crate-private:
the typestate API is the only public way to change the boot selection.

The BSP hands out the chip's `FLASH` peripheral as `Board::internal_flash`, and
the `SPI0` singleton (the flash cache's MMU) for reading the booted offset.
Only `eveningstar::storage` may create a `FlashStorage` (clippy
`disallowed-methods`).

## Layout

Declared in `nix/board.nix`, with every offset written out: the layout is a
contract that can't change over OTA, so nothing in it is left to
`gen_esp32part`.

| Partition | Offset | Size | Handle | Class |
|---|---|---|---|---|
| bootloader | 0x0 | 32 KiB | read | public |
| partition table | 0x8000 | 4 KiB | read | public |
| `factory` | 0x9000 | 4 KiB | none (reserved for durable factory data) | — |
| `otadata` | 0xA000 | 8 KiB | read view; writes only through `Ota` | public |
| (gap to 64 KiB alignment) | 0xC000 | 16 KiB | — | — |
| `ota_0` | 0x10000 | 3 MiB | running: read; other: read view, writes only through `Ota` | public |
| `ota_1` | 0x310000 | 3 MiB | as `ota_0` | public |
| `config_0` | 0x610000 | 128 KiB | running slot's: read, erase, program; other's: read, erase | sensitive |
| `config_1` | 0x630000 | 128 KiB | as `config_0` | sensitive |
| `diag` | 0x650000 | 256 KiB | read, erase, program | sensitive |
| `reserved` | 0x690000 | 1472 KiB | none | — |

- `nvs` goes: nothing uses the ESP-IDF NVS format, and a partition shared by
  both slots would undo the reasoning that per-slot configuration buys.
- `factory` (data/undefined, `readonly` flag) and `reserved` (data/undefined)
  hold space for later firmware, which can only claim space the table already
  has. No handle is handed out for them now.
- `config_0` and `config_1` pair with `ota_0` and `ota_1`. 128 KiB holds about
  30 KB of live data (three RSA-4096 keypairs with certificates and chains, a
  CA, password hashes, settings) with the headroom a log-structured store needs.
- `diag` will hold diagnostics: crash records (backtraces, not memory) and
  events such as updates and reboots, likely as a priority ring buffer. Only the
  space is allocated now.
- 3 MiB app slots leave about 2× headroom over a firmware with WiFi and TLS
  (today's image is 225,792 bytes).

### Generated layout

- **G1.** Nix generates `crates/eveningstar-storage/src/layout.rs` from
  `board.nix` with `nix eval` (no ESP-IDF build): the table offset and, per
  partition, label, type, subtype, offset, size and flags. It is committed, so a
  plain `cargo build` works without Nix. A dev-shell command, `storage-layout`,
  writes it.
- **G2.** The `lint` group fails when the committed file differs from what
  `board.nix` generates. The `build` group also checks that the table ESP-IDF
  builds (`partition-table`'s `partitions.json`) resolves to the same offsets.
- **G3.** Generation fails when an offset is missing, a data partition isn't
  4 KiB aligned, an app partition isn't 64 KiB aligned, two partitions overlap,
  one lies below 0x9000 or past the flash size, or `config_0` and `config_1`
  (or `ota_0` and `ota_1`) differ in size.
- **G4.** Rust adds roles and capabilities on top of the generated constants.
  Renaming or removing a partition that has a role breaks the Rust build. A
  partition with no role (`factory`, `reserved`) still takes part in the table
  check but gets no handle.

## Trust tiers

`split` runs once per boot and returns a `Storage` value holding a `Report` and
the handles of the tier it reached. It never fails: the worst outcome is the
untrusted tier.

| Tier | Condition | Handles |
|---|---|---|
| Untrusted | the table at the compiled offset doesn't match the layout | read: bootloader, the table found on the board (if any), running app |
| Data | the table matches the layout | + config, diag, boot state (otadata read view), other app read view |
| Full | + the bootloader is known | + `Ota` |

- **T1. Table check.** The table at the compiled offset matches when its
  entries, in order, equal the generated layout: label, type, subtype, offset,
  size and flags, followed by the MD5 entry or erased bytes. Bytes after the
  entries are ignored, so a flipped bit in the padding, which the bootloader
  ignores too, can't make storage stricter than the boot chain. The `Report`
  holds the comparison per partition: matches, differs (naming the fields),
  missing, or unexpected.
- **T2. Finding the board's table.** When the compiled offset holds no matching
  table, `split` looks for the table the bootloader used: each 4 KiB-aligned
  sector below the running app, taking the first whose entries are well formed
  and include an app partition starting at the booted offset. The `Report`
  holds its offset and entries (unknown types and subtypes kept as raw values,
  labels as bytes), and a public read handle covers it. If none is found, the
  `Report` says so and there is no table handle.
- **T3. Bootloader check.** The bootloader at 0x0 is verified as the ROM does:
  header, segments, XOR checksum and appended SHA-256 over the image. Its
  identity is that digest, and the bytes between the image's end and the table
  are ignored. The full tier needs the digest in the known list, because only a
  known bootloader is known to support rollback; an update with any other could
  brick the board.
- **T4. Known bootloaders.** The list lives in `known.rs`, committed, with a
  name per digest. It only grows once a board has shipped: every bootloader a
  board in the field may have stays accepted. The `build` group fails when the
  bootloader built from `nix/board.nix` isn't in it, printing the line to add.
- **T5. Running slot.** The booted offset comes from the MMU. In the data and
  full tiers it must be the offset of `ota_0` or `ota_1`; otherwise the result
  is the untrusted tier. In the untrusted tier, the running app's size comes
  from its entry in the board's table, or its image length when there is no
  table.
- **T6. Boot never depends on storage.** In the untrusted tier the firmware
  runs on default configuration that it doesn't save (Ethernet with DHCP), and
  the API and the status LED say why.
- **T7. An image that can't trust the board doesn't keep running
  unconfirmed.** If the running image is `PendingVerify` and the tier isn't
  full, the firmware requests a graceful reboot, so the bootloader rolls back to
  the previous firmware. A release that forgot a bootloader digest therefore
  costs one reboot per board. (OTA never changes the table, so a board in the
  untrusted tier can't have received an OTA image.)

`cargo run` writes probe-rs's own bootloader and table, so it gives the
untrusted tier. That is accepted.

## Handles

```rust
pub struct Partition<'f, R: Role> { /* region, PhantomData<R> */ }

pub enum Config {}                    // a marker type, never a value
// Role, SensitiveRead, Erasable and Writable implemented for Config.

async fn run(config: Partition<'static, Config>)   // a Diag handle doesn't type-check
```

- **H1. Roles.** Each role is an uninhabited marker type; `Partition<'f, R>` is
  the handle, and it stores its span at runtime (per-slot roles pick one of two
  offsets at boot). Roles that map to layout partitions also implement
  `FixedSize`, whose `SIZE` comes from the generated layout, so code can size
  buffers and assert its needs at compile time (`const { assert!(R::SIZE >= 2
  * SECTOR_SIZE) }`). `split` builds those handles only from the generated
  constants, so `len() == R::SIZE` always. The boot-chain read handles
  (bootloader, table, running app) don't implement `FixedSize`: their spans
  are found at runtime, as they describe what is on the board, and code that
  needs `R::SIZE` doesn't compile for them.
- **H2. Capabilities.** Sealed traits on the role decide what a handle can do:
  `SensitiveRead` (read; every role), `Read: SensitiveRead` (a marker: the
  contents may leave the device), `Erasable` and `Writable`. `erase` and
  `program` exist only on handles whose role has them.
- **H3. Public and sensitive.** Public roles: bootloader, partition table,
  running app, other app, otadata. Sensitive: `Config`, `OtherConfig`, `Diag`.
  A function that exports data takes `impl Read` and so can't be given a
  sensitive handle; one that handles secrets takes `impl SensitiveRead` and
  accepts both. `Read` is the name people reach for, so the default is the
  restrictive one.
- **H4. Sharing.** Public read handles are `Copy`, so an API download and the
  OTA service can read the same slot. Sensitive and writable handles are unique
  values made only by `split`: holding one is the permission, and there is one
  of each.
- **H5. NOR traits.** Sensitive handles don't implement the
  `embedded_storage_async::nor_flash` traits, which would be a second, generic
  way to read them. `into_nor_flash(self)` on `Config` and `Diag` consumes the
  handle and returns an adapter implementing `ReadNorFlash`, `NorFlash` and
  `MultiwriteNorFlash`, for the store that owns the partition.
- **H6. Writes.** Writable handles have NOR semantics: erase sets 4 KiB sectors
  to `0xFF`, program only clears bits, at 4-byte alignment. There is no
  read-modify-write, which would erase a sector the caller didn't ask to lose.
  Programming zeros over a record destroys it in place.
- **H7. The other slot's configuration** can be read (to migrate after an
  update) and erased (factory reset, wiping secrets), never programmed. Erase is
  the secure wipe: NOR has no translation layer keeping old copies elsewhere.
- **H8. Protected regions.** Bounds are checked on every access. On top of the
  types, every erase and program checks the absolute range against the
  bootloader, the table's sector, `factory`, `reserved` and the running app, and
  refuses it (`Protected`), so a bug in the crate's own arithmetic can't reach
  them.
- **H9. Long operations** are async and yield after every sector erased and
  every 4 KiB programmed or hashed. A ROM flash operation runs with interrupts
  disabled for up to one sector erase (typically ~45 ms, up to ~400 ms), so no
  single call erases more than one sector or programs more than 4 KiB.
- **H10.** Stack frames stay under the 1 KiB `large_stack_frames` limit: no
  sector-sized buffers on the stack.

## OTA and rollback

The otadata format and selection rules are ESP-IDF's
(`bootloader_common_get_active_otadata`, `esp_rewrite_ota_data`,
`esp_ota_mark_app_valid_cancel_rollback`), so the bootloader built with
`BOOTLOADER_APP_ROLLBACK_ENABLE` reads them the way the firmware writes them.

The crate writes its own otadata code. `esp-bootloader-esp-idf` 0.6.0
(`src/ota.rs`) can't be used, because:

1. reading either entry with a bad CRC or an unknown state fails every
   operation, which is the torn write the two copies exist for;
2. it picks the entry with the highest sequence number even when it is
   `Aborted` or `Invalid`, so after a rollback it names the bad image as current
   and `set_current_ota_state(Valid)` revives it;
3. activation copies the target sector's old state and needs a second write to
   set `New`, so a power cut between them boots the new image as `Valid`, with
   no rollback;
4. activation reads the target sector first, so a corrupt entry there blocks
   updates for good.

Requirements:

- **O1. Entries.** Each entry is judged on its own: valid when its sequence
  number isn't `0xFFFFFFFF`, its CRC matches, and its state isn't `Invalid` or
  `Aborted`. The active entry is the valid one with the higher sequence number.
  A corrupt sector never blocks reading the other or overwriting it.
- **O2. Boot state** (data tier and up): the running slot, the slot the otadata
  selects, the running image's state (`New`, `PendingVerify`, `Valid`,
  `Invalid`, `Aborted`, `Undefined`), and whether the other slot is a rollback
  target (its entry is valid and selects it).
- **O3. `Ota::begin_update`** returns an `Update` writer for the other slot. If
  the otadata selects the other slot (an activation not yet booted, or a broken
  image the bootloader fell back from), it first writes an entry selecting the
  running slot, so a half-written slot is never the one selected. It then erases
  the slot's first sector.
- **O4. `Update::write`** takes the image sequentially in chunks of any size,
  erasing sectors just ahead of the data. Writing past the slot is an error.
- **O5. `Update::finish`** verifies the written image: header magic, chip ID
  (ESP32-C6), segments inside the slot, the XOR checksum, the appended SHA-256
  (required), an app descriptor whose project name matches the running app's,
  and that exactly the image's bytes were written. Only then does it return a
  `VerifiedImage`.
- **O6. `Ota::activate(VerifiedImage)`** is the only way to select the other
  slot. It checks that the `VerifiedImage` comes from the latest update (a new
  `begin_update` makes older ones stale), hashes the slot again, and writes one
  entry, with state `New`, to the otadata sector that isn't active, so a power
  cut leaves the old selection in place. `Ota::verify_other` makes a
  `VerifiedImage` from the image already in the other slot, to switch back to
  it.
- **O7. `Ota::mark_valid`** sets the running image's entry to `Valid` unless it
  already is, rewriting the active entry in place as ESP-IDF does. A power cut
  there leaves the other entry active, which is a rollback, not a brick. It
  refuses when the active entry doesn't select the running slot.
- **O8. `Ota::roll_back`** marks the running image `Invalid` so the bootloader
  starts the other slot next boot. It refuses unless the other slot is a
  rollback target holding an image that verifies.
- **O9.** Nothing in storage reboots. Activating and rolling back return, and
  the caller asks the reboot service.

When to confirm an image (health checks after an update), how updates arrive,
and their authenticity are the OTA service's job, later.

## Glue (`eveningstar::storage`)

- **F1.** Implements `Flash` with `esp_storage::FlashStorage` behind a
  critical-section mutex: `read` (any alignment), `erase_sector` (one sector per
  call) and `program` (`write_nor`).
- **F2.** Reads the booted offset from MMU entry 0, inside a critical section
  (`esp-storage` uses the same index register). Every tier needs it: it decides
  the running slot, which configuration is the running slot's, what is
  protected and what OTA writes to. The otadata can't stand in, as it says what
  was selected, not what booted. `esp-bootloader-esp-idf`'s `booted_partition`
  does the same read, but only after reading and MD5-checking the table at the
  compiled offset, so it fails when the table has moved (T2). The read goes
  through the `esp32c6` PAC's `SPI0` MMU registers, with the `SPI0` singleton
  from the BSP.
- **F3.** Logs the tier, the table comparison, the bootloader's name or digest,
  the running slot, the app version and the boot state.
- **F4.** Implements T6 and T7.

## Tests (host)

- Layout: the generated layout against `board.nix`'s rules (G3).
- Table check: the real layout matches; each field of each entry changed in turn
  is reported as that difference; missing, extra and reordered entries; a
  flipped bit in the padding still matches; garbage and truncated tables never
  panic.
- Table discovery: a moved table is found; a decoy table without an entry at the
  booted offset is skipped; none found.
- Images: built in tests with segments, checksum and digest; each corruption
  (magic, chip, segment length, checksum, digest, no digest, image past the
  partition) rejected.
- Otadata: the CRC against `esp-bootloader-esp-idf`'s test vectors; selection
  with every combination of empty, valid, `Invalid`, `Aborted` and corrupt
  entries; the four failures of that crate listed above, each handled.
- Handles: bounds, alignment, the protected-region guard, NOR program semantics
  on a RAM flash, `Read`/`SensitiveRead` misuse failing to compile (doc tests
  with `compile_fail`).
- OTA flow on a RAM flash: update, finish, activate, then a simulated
  bootloader step; stale `VerifiedImage`; activation pending at `begin_update`;
  mark valid; roll back with and without a target; a power cut (write failing)
  during activation or mark valid leaves a bootable selection.
- `check-images` (example) on the Nix-built bootloader and table, and on an app
  image.

## Out of scope

- The external NAND; the config and diag record formats and stores; the OTA
  service; the API endpoints and web UI views (layout, downloads); factory
  provisioning.
- Crash capture (likely an LP RAM record copied to `diag` on the next boot,
  which was deferred earlier).
- Updating the bootloader or the partition table from the firmware.
- Flash encryption and secure boot (the build forbids both).
- Image signatures. `VerifiedImage` means intact and meant for this board, not
  authentic.
- A debug dump of the whole flash. If it comes later, sensitive partitions go
  through an allowlist sanitizer owned by each store, failing closed, and the
  dump needs authentication.

## Decided

- F2 reads the MMU with one `unsafe` block: the C6's MMU table is reached only
  through an index register (which entry) and a content register (that entry's
  physical page), and the PAC marks writes to the index register `Unsafe`.
  Entry 0 maps the start of the running app's image, so its physical page is
  the running partition's offset.

### Dependencies

`sha2` and `embedded-storage-async` join the workspace (both already in
`Cargo.lock` through other crates), and `esp-storage` becomes a direct
dependency of the firmware (already built through `esp-bootloader-esp-idf`).

## Not verified yet

- Everything on hardware: the booted offset, the otadata the bootloader writes
  on its first boot after `flash-app` (expected: entry 0, sequence 1), and an
  update, activation, `PendingVerify` boot, confirm and rollback with the real
  bootloader.
- The interrupt latency of sector erases next to Ethernet traffic.
- Whether the config store chosen later accepts records zeroed in place.
