# Diagnostics: messages across reboots and firmware versions

Status: proposed 2026-11-08, for the user's review; to be refined. No code
is written until the user approves it. Builds on `specs/storage-v2.md` (the
`diag` region and its store) and `specs/ota-signing.md` (the trial, its
self-checks and rollback).

## Asked for by the user

- A way to pass messages between reboots and between firmware versions: crash
  logs, reboot reasons, and why an update was rolled back.
- The `diag` region, LP RAM, or both, chosen on the merits.

This revisits the LP RAM reset record deferred on 2026-10-17, which was put
off to avoid `unsafe` `static mut` code while the watchdog supervisor was
reworked.

## Sources read

- esp-hal 1.2.2: `ld/esp32c6/memory.x` (`RTC_FAST`, 16 KiB at
  `0x5000_0000`), the `#[ram(unstable(rtc_fast, persistent))]` documentation
  in esp-hal-procmacros 0.23.0, and `SocResetReason` in `src/rtc_cntl`.
- ESP-IDF 5.5.2: `components/bootloader/Kconfig.projbuild`, where
  `BOOTLOADER_RESERVE_RTC_MEM` is selected only by the factory reset and the
  deep sleep validation skip, and the bootloader's linker script, which
  places nothing in LP RAM.
- This repository: the panic handler (`crates/eveningstar/src/main.rs`), the
  software reset (`reset.rs`), `ResetCause` and the reboot service
  (`crates/eveningstar-system`), the reboot reasons
  (`crates/eveningstar/src/services/reboot.rs`), the `Diag` role
  (`crates/eveningstar-storage/src/roles.rs`), and the diag store's notes in
  `specs/storage-v2.md` ("Left to build").

What they establish:

- LP RAM keeps its contents across a software reset, a panic's reset, deep
  sleep and watchdog resets (the esp-hal documentation), and loses them at
  power-on. esp-hal warns that a reset during an update of a persistent
  static, or before its first zeroing, can leave arbitrary bytes, and
  suggests a checksum.
- `SocResetReason` distinguishes resets of the digital core from resets of
  "the digital core and rtc module" (`SysRtcWdt`, `SysSuperWdt`,
  `SysClkGlitch`), and reports a brownout as power-on (0x01). Whether LP RAM
  survives each of these on the ESP32-C6 isn't documented in these sources;
  it is measured on the board (see "Verification").
- With the bootloader configuration pinned in `specs/ota-signing.md`
  (decision 16), the bootloader doesn't touch LP RAM.
- The firmware resets in three places: the panic handler, and
  `SoftwareReset`, through which the reboot service (`GracefulReboot`,
  `ImmediateReboot`) and the watchdog supervisor (`MissedDeadline { task }`)
  reset. Each knows why at the moment it resets, and today only logs it over
  defmt.
- `diag` is 256 KiB (64 sectors), shared by both slots, `Sensitive`, with a
  writer in the layout and boot tiers. It is never published raw.

## Two layers

- **The mailbox, in LP RAM.** Written at the moment something goes wrong,
  when writing flash is the riskiest thing to do: a panic, a missed watchdog
  deadline, the reboot that ends a trial. A flash erase takes tens of
  milliseconds, the fault may be in the flash path itself, and a crash loop
  would spend erase cycles on every reset. LP RAM costs a few stores.
- **The log, in `diag`.** Durable history: early in each boot the firmware
  moves the mailbox's entries into the log and clears them, and events
  outside a crash (an update begun, activated, confirmed, a switch, self-check
  results) are written to the log directly.

Neither alone is enough: the log can't be written safely from a panic, and
the mailbox is lost at power-on and holds no history.

## The record encoding

The mailbox, the log and the OTA spec's image information block share one
encoding, in a new host-buildable crate (`eveningstar-records`, `no_std`, no
allocation, `#![forbid(unsafe_code)]`). Its problem in all three is the
same: an older firmware reads what a newer one wrote, for example after a
rollback.

- A **container** starts with a magic (`u32`) and a total length, and holds
  entries up to that length. The container header never changes.
- An **entry** is a type (`u16`), a length (`u16`) and that many bytes of
  value.
- **Unknown types are skipped**, and reported as unknown with their type and
  length.
- **A value only grows.** Its fields are fixed-size and at fixed offsets; a
  later version appends fields, and a reader takes the prefix it knows. A
  value shorter than the reader knows is refused.
- **A field never changes meaning.** A new meaning is a new type. Enumerated
  codes only gain values; a reader shows a value it doesn't know as its
  number.
- **Integrity** is the container's: the image information block is covered by
  the image's digest, mailbox entries and log records carry a CRC-32 each,
  since a reset or a power cut can tear them.

## The mailbox

### Placement

- The last 1 KiB of LP RAM, `0x5000_3C00` to `0x5000_4000`, reserved in the
  linker script so no static lands there. The address and the header are a
  permanent contract between every firmware version, like the partition
  layout.
- The fixed-address access is the only `unsafe` here: one small module in the
  firmware crate's glue (as `storage.rs` holds `FlashStorage::new`), with
  volatile copies in and out of a RAM buffer. Everything else works on that
  buffer through `eveningstar-records`, and is tested on the host.

### Format

- Header: magic, total length, and a write counter (`u32`).
- A ring of 4 entries. Each: length, CRC-32, a sequence number (`u32`,
  from the write counter) and one record as a container entry.
- A new entry overwrites the oldest. Four entries keep, for example, a
  watchdog record and the panic that caused the stall, or two crashes before
  a boot gets as far as moving them to the log.

### Writers

- The panic handler: a `Panic` record and a `Reset` record (cause: panic),
  before its log spin.
- `SoftwareReset::reset`: a `Reset` record with the `ResetCause` it is given.
- The confirmation window, before the reboot that ends a trial: a `TrialEnd`
  record with the reason.
- Writers outside the panic handler write inside a critical section. The
  panic handler writes without one: on this single-core chip a panic during
  another write tears at most that entry, which its CRC then rejects.

### Reading

- Early in `main`, before anything can write it, the firmware copies the
  mailbox into RAM. When the hardware reset reason is power-on, or the header
  doesn't parse, the mailbox is cleared and nothing is read. Otherwise every
  entry whose CRC matches is kept and the rest are dropped.
- The mailbox is cleared once its entries are in the log. Where the boot
  doesn't reach a writer of `diag` (the unverified tier), the entries stay in
  the mailbox for a later boot, and the API serves them from RAM meanwhile.
- An entry already in the log, recognised by its boot ID and sequence
  number, isn't written again: a reset between the log write and the clear
  duplicates nothing.

## The log

### Format

- 64 sectors used as a ring. Each sector starts with a header: a magic that
  never changes and a sector sequence number (`u32`).
- Records follow back to back: length, type, CRC-32, value. Each carries a
  record sequence number (`u32`), the boot ID (a random `u64` drawn at each
  boot), and the uptime in milliseconds. A wall-clock time is added as a
  field once the firmware knows the time.
- **Mount:** the sector with the highest valid sequence number is the head.
  The append point is the first byte after its last record whose CRC
  matches. When the bytes from there on aren't all `0xFF` (a torn record),
  the sector counts as full and appending continues in the next one.
- **Rotation:** a full head moves on to the next sector, which is erased
  first; the oldest records go.
- A sector whose header doesn't parse is erased only when the ring reaches
  it, so a firmware never wipes history it merely can't read.

### Owner and failures

- The diag store owns `Writer<Diag>`, as `specs/storage-v2.md` plans.
- The log is best effort. A failed read or write is logged over defmt and
  reported by the API; it never blocks a boot, an update or a confirmation.
- It never holds configuration values or secrets. The one exception is the
  panic message, which formats values the code chose (decision 6).

### Wear

- A crash loop writes one record per boot. To bound it, a `Panic` or `Reset`
  record equal to the latest one in the log (same type, cause and location)
  isn't written again; the latest record's repeat count is kept in the
  mailbox, and a `Repeat` record is written each time the count reaches a
  power of two, and when the loop ends.
- With that rule a loop of one boot per second writes a few records per
  hour. Without it, it would erase each of the 64 sectors about once an hour,
  which the flash's endurance (about 100 000 erases per sector) would bear
  for years; the rule keeps the history readable rather than saving the
  flash.

## Records

The first types, each a container entry. Codes are enumerations that only
gain values.

- **`Boot`**, written at every boot: boot ID, hardware reset reason (the raw
  code), running slot, version string, the first 8 bytes of the image's
  SHA-256, storage tier, and boot state (`Valid`, `PendingVerify`, fallback).
- **`Reset`**, from the mailbox: the firmware's cause (`GracefulReboot` with
  the reboot reason's code, `ImmediateReboot`, `MissedDeadline` with the
  task's name, at most 16 bytes, `Panic`), uptime, version.
- **`Panic`**, from the mailbox: file (at most 48 bytes), line and column,
  and the message (at most 96 bytes).
- **`TrialEnd`**, from the mailbox: why the trial ended (a required
  self-check failed, with its ID; the deadline passed), the trial image's
  version, and the otadata sequence number of its entry.
- **`Ota`**, written directly: update begun, refused (the refusal code),
  activated (versions from and to, sequence number), confirmed, switched,
  trial extended.
- **`SelfCheck`**, written directly during a trial: the check's ID and its
  result.
- **`Rollback`**, written by the image that resumes: the sequence number of
  the `ABORTED` entry, the trial image's version, and the cause (see below).
- **`Repeat`**: the type and count of a repeated record.

## Rollback reasons, end to end

At every boot the firmware compares the otadata with the log. When the entry
for the other slot is `ABORTED` and the log holds no `Rollback` record for
its sequence number, it writes one, with the cause found in this order:

1. a `TrialEnd` record in the mailbox: its reason;
2. a `Panic` or `Reset` record in the mailbox: the panic or the missed
   deadline;
3. none, with a power-on reset reason: power lost during the trial;
4. none otherwise: a reset with no message, with its reset reason.

The sequence number makes it one record per rollback, however many times the
board boots afterwards. The web UI shows the latest `Rollback` record, for
example: "Update to 1.5.0 rolled back: required self-check `config
migration` failed."

The trial image may also write `SelfCheck` and `Ota` records to the log
while it runs, so those survive a power cut that empties the mailbox.

## Across firmware versions

- The older image reads the newer image's records after a rollback, and the
  newer reads the older's after an update; the encoding's rules cover both.
- The mailbox's address and header, the log's sector header, and the record
  encoding are permanent once boards ship; changing them needs the user's
  approval, like the layout.
- Committed fixtures: a mailbox and a log sector of every released format,
  which every later firmware must read; and a "future" mailbox and sector,
  with an unknown type and every value longer than known, which must read as
  without the additions.

## API (sketch)

- `GET /api/v1/diag`: records, newest first, paged by record sequence
  number. Each record has its sequence number, boot ID, uptime, type and
  fields; an unknown type shows its type and length only.
- The latest `Rollback` and `Reset` records also appear in `GET /firmware`
  and `GET /system`, for the web UI's banners.
- Mailbox entries not yet moved to the log (unverified tier) appear in
  `GET /diag` marked as such.
- `DELETE /api/v1/diag` erases the log, with the highest permission tier.

## Verification

- **Host, encoding:** fuzz targets for the container parser, the mailbox
  parser and the log mount; the fixtures above.
- **Host, log:** a property test on the test flash: for any sequence of
  appends, rotations and power cuts at any program or erase, the mount finds
  every record whose write completed, in order, and nothing torn.
- **Host, mailbox:** random bytes in the mailbox's place never produce an
  entry; a torn entry is dropped; a duplicate drain writes nothing.
- **Board, LP RAM retention:** with a test build, which mailbox entries
  survive each reset: software reset, panic, MWDT0, the RWDT system reset,
  a reset through USB Serial/JTAG (probe-rs, esptool), a power cycle, and a
  brownout if one can be provoked safely. The result decides whether causes
  like `SysRtcWdt` are reported from the mailbox or only from the hardware
  reset reason.
- **Board, end to end:** a panic and a missed deadline appear in
  `GET /diag` after the reboot; a trial that fails a required self-check,
  one that reaches its deadline, and one cut by a power cycle each produce a
  `Rollback` record with the right cause, once; a crash loop produces
  `Repeat` records at powers of two.

## Decisions for the user

Each with a recommendation.

1. **Two layers.** Recommended: the mailbox in LP RAM and the log in `diag`.
2. **Mailbox size and place.** Recommended: the last 1 KiB of LP RAM, fixed
   for good.
3. **Mailbox entries.** Recommended: a ring of 4.
4. **Log format.** Recommended: a ring of sectors with headers, records with
   a CRC-32 each, a torn tail ending the sector.
5. **Crash loops.** Recommended: repeated records coalesced, with `Repeat`
   records at powers of two.
6. **The panic message.** It formats values the code chose, so it may one
   day hold something sensitive. Recommended: kept, at most 96 bytes, and
   served only with the highest permission tier; the other fields with the
   read tier.
7. **The shared encoding.** Recommended: one crate for the image information
   block, the mailbox and the log, so the forward-compatibility rules are
   written and tested once.
8. **A `Boot` record at every boot.** Recommended: yes; one small record per
   boot costs little wear and gives every other record its context.
9. **Where the `unsafe` lives.** Recommended: one module in the firmware
   crate's glue, volatile copies only, all parsing safe and on the host.
10. **Clearing the log.** Recommended: `DELETE /diag` with the highest
    permission tier, and no clearing at a factory reset unless the user asks
    for it then.
11. **Unverified tier.** Recommended: the mailbox is read and served but not
    cleared, as there is no writer of `diag`.

## Left open

- Backtraces: the panic record holds the location and the message; a stack
  walk is later work.
- Wall-clock time in records, once the firmware has a time source.
- Priorities in the log (crash records kept longer than routine events), if
  64 sectors prove too few.
- How the diagnosis and status tree reads the log and the advisory
  self-checks.
