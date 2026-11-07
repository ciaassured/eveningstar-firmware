# Diagnostics: messages across reboots and firmware versions

Status: proposed 2026-11-08, revised the same day after review; for the
user's review, to be refined. No code is written until the user approves it.
Builds on `specs/storage-v2.md` (the `diag` region and its store) and
`specs/ota-signing.md` (the trial, its self-checks, rollback, and the image
information block's encoding).

## Asked for by the user

- A way to pass messages between reboots and between firmware versions: crash
  logs, reboot reasons, and why an update was rolled back.
- The `diag` region, LP RAM, or both, chosen on the merits.

This revisits the LP RAM reset record deferred on 2026-10-17, which was put
off to avoid `unsafe` `static mut` code while the watchdog supervisor was
reworked.

## Requirements

Each with the mechanism that holds it and what shows it, other than the
authors.

1. **A crash, a missed watchdog deadline or the end of a trial is recorded,
   and survives every reset but a chip reset.**
   - Held by: the mailbox, written at the moment, in memory TRM 8.1.1 says
     every reset but a chip reset keeps.
   - Shown by: the board's retention test across each reset reason, and the
     fault catalogue's records read back over the API.
2. **The history survives power loss, and a torn write loses at most the
   record being written.**
   - Held by: the log's per-record CRC and its mount rules.
   - Shown by: the host model of the log with torn and unstable flash, and
     the USB power-cut rig of `specs/ota-signing.md`.
3. **Every firmware version reads every other's records.**
   - Held by: the record encoding's rules.
   - Shown by: the committed fixtures of every released format and the
     "future" fixtures, in CI.
4. **A rollback is recorded once, with its cause.**
   - Held by: the "activated" record and the inference rules.
   - Shown by: the host inference tests, and the board's rollback scenarios
     under the power-cut rig.
5. **Diagnostics never block a boot, an update or a confirmation, and never
   hang the board.**
   - Held by: best-effort writes; the panic handler's re-entry guard.
   - Shown by: the fault catalogue with the `diag` region filled with
     garbage, and a panic inside the panic handler.

## Sources read

- ESP32-C6 Technical Reference Manual v1.2, chapter 8 (reset and clock):
  section 8.1.1 and table 8.1-1.
- esp-hal 1.2.2 (`ld/esp32c6/memory.x`, `ld/esp32c6/linkall.x`, `build.rs`,
  `src/rtc_cntl/rtc/esp32c6.rs`, `src/system.rs`, `src/soc/mod.rs`,
  `src/soc/esp32c6/lp_core.rs`), esp-riscv-rt 0.15.0 (`__pre_init`),
  esp-hal-procmacros 0.23.0 (`#[ram(persistent)]`), esp-rtos 0.4.0 and
  esp-sync 0.3.0, and the esp32c6 PAC 0.24.0 (`ASSIST_DEBUG`).
- ESP-IDF 5.5.2: `bootloader/Kconfig.projbuild`,
  `bootloader_support/src/bootloader_common_loader.c` and
  `bootloader_utility.c` (the retained RTC struct),
  `soc/esp32c6/include/soc/soc.h` and `reset_reasons.h`,
  `esp_hw_support/port/esp32c6/cpu_region_protect.c`, the bootloader's and
  the ROM's linker scripts, and `bootloader_esp32c6.c`.
- This repository: the panic handler and `main` (`crates/eveningstar/src/
  main.rs`), the software reset (`reset.rs`), `ResetCause`, the reboot
  service and the supervisor (`crates/eveningstar-system`), the reboot
  reasons (`services/reboot.rs`), the confirmation window
  (`storage/confirmation.rs`), the self-checks (`eveningstar-ota`
  `checks.rs`), the `Diag` role (`eveningstar-storage` `roles.rs`), and the
  diag store's notes in `specs/storage-v2.md`.

What they establish:

- **Retention.** The ESP32-C6 has four resets: CPU, core, system and chip.
  "All reset types mentioned above (except Chip Reset) preserve the data
  stored in internal memory" (TRM 8.1.1). A chip reset is power-on, or a
  brownout when the brownout detector is set to reset the chip (code 0x01);
  a brownout set to reset the system is 0x0F and keeps memory. Software
  reset (0x03), the MWDT and RWDT core resets, USB Serial/JTAG resets and
  the RWDT and super watchdog system resets (0x10, 0x12) all keep it.
  `esp_hal::system::reset_reason()` returns `None` for a code it doesn't
  know, and esp-hal's own persistent statics treat `None` as power-on; so
  does this design.
- **Nothing else writes the top of LP RAM.** LP RAM is 16 KiB at
  `0x5000_0000`. The bootloader's and the ROM's linker scripts place nothing
  there. esp-hal, esp-rtos, esp-storage, rtt-target and embassy reference
  it nowhere, except esp-hal's LP core loader, which zero-fills all 16 KiB;
  the firmware doesn't use the LP core. esp-riscv-rt zeroes
  `.rtc_fast.bss` at every boot and `.rtc_fast.persistent` at power-on, and
  touches nothing else. probe-rs's ESP32-C6 target maps no memory there, so
  a `cargo run` reflash doesn't clobber it.
- **The bootloader's reservation.** With `BOOTLOADER_RESERVE_RTC_MEM` on,
  the ESP-IDF bootloader keeps a struct at the top of LP RAM and clears it.
  The option is selected by `BOOTLOADER_FACTORY_RESET`,
  `BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP` and
  `BOOTLOADER_CUSTOM_RESERVE_RTC`, all off by default and not pinned today.
  `specs/ota-signing.md` decision 16 pins them off.
- **LP RAM is writable in any panic.** The bootloader's PMP leaves
  `0x5000_0000`–`0x5000_4000` readable, writable and executable, and
  esp-hal adds only its own regions. Stores to it are plain data-bus stores.
- **esp-hal's linker setup.** `memory.x` comes from esp-hal, copied to its
  `OUT_DIR` and included by `linkall.x`; nothing in this repository can
  shrink `RTC_FAST` itself. A fragment of the firmware's own, passed with
  `-T` from its build script (which already passes `-Tdefmt.x`), can place
  an output section at a fixed address inside it.
- **Where the firmware resets.** In the panic handler, and through
  `SoftwareReset`, which the reboot service (`GracefulReboot`,
  `ImmediateReboot`) and the supervisor (`MissedDeadline { task }`) use.
  `GracefulReboot` carries no reason today. MWDT0 and RWDT reset the chip
  without any software hook when the supervisor itself stalls.
- **Errata (v0.2).** CPU-863: a misaligned load followed by a store across
  PMP regions can fault; the mailbox's copies use byte or aligned word
  accesses. The flash datasheet (XM25QH64C, `specs/ota-signing.md`
  "Hardware facts"): an interrupted program or erase leaves data undefined,
  and bits can read differently from one read to the next.
- **Where a watchdog reset hung.** The C6 bootloader enables the debug
  assist's PC recording, which survives a core reset, and the PAC exposes
  it (`ASSIST_DEBUG.rcd_pdebugpc`). To be confirmed on the board.
- **`diag`** is 256 KiB (64 sectors), shared by both slots, `Sensitive` (no
  reader, no snapshot), with a writer in the layout and boot tiers, never
  published raw. Today the firmware drops that writer.

## Two layers

- **The mailbox, in LP RAM.** Written at the moment something goes wrong,
  when writing flash is the riskiest thing to do: a panic, a missed watchdog
  deadline, the reboot that ends a trial. A flash erase takes tens of
  milliseconds, the fault may be in the flash path itself, and a crash loop
  would spend erase cycles on every reset. Every memory survives all but a
  chip reset; LP RAM is chosen because nothing else writes its top.
- **The log, in `diag`.** Durable history: early in each boot the firmware
  moves the mailbox's entries into the log, and events outside a crash (an
  update activated, confirmed or refused, a switch, self-check results) are
  written to the log directly.

Neither alone is enough: the log can't be written safely from a panic, and
the mailbox is lost at a chip reset and holds no history.

## The record encoding

The mailbox, the log and the OTA spec's image information block share one
encoding, in a new host-buildable crate, `eveningstar-records` (`no_std`, no
allocation, `#![forbid(unsafe_code)]`, listed in `host_crates` in
`nix/checks.nix`). Older firmware reads what newer firmware wrote (after a
rollback), and newer reads older (after an update).

- A **container** starts with a magic (`u32`) and a total length, and holds
  entries up to that length. The container header never changes.
- An **entry** is a type (`u16`), a length (`u16`) and that many bytes of
  value.
- **Unknown types are skipped**, and reported as unknown with their type and
  length.
- **Values grow at the end only.** Fields are fixed-size and at fixed
  offsets. Each type has a minimum length, set when the type is first
  released and never raised; fields added later are optional, and a reader
  reads one the value ends before as absent and ignores bytes past what it
  knows. Only a value shorter than the type's minimum is refused.
- **A field never changes meaning.** A new meaning is a new type. Codes are
  enumerations that only gain values; a reader shows a value it doesn't
  know as its number.
- **Integrity** is the container's: the image information block is covered by
  the image's digest; mailbox entries and log records carry a CRC-32 each,
  since a reset or a power cut can tear them.
- **Stable codes.** Every code a record carries is a number fixed for good:
  reboot reasons, self-checks (each check gets a permanent ID, in an
  enumeration that only grows), refusal reasons, trial end reasons.

## The mailbox

### Placement

- The last 1 KiB of LP RAM, `0x5000_3C00` to `0x5000_4000`: a `NOLOAD`
  output section placed there by a linker fragment of the firmware's own,
  with an `ASSERT` that esp-hal's `.rtc_fast.*` sections end below it. The
  address and the header are a permanent contract between every firmware
  version, like the partition layout.
- The bootloader options that reserve the top of LP RAM are pinned off
  (`specs/ota-signing.md` decision 16). Using the LP core needs the mailbox
  moved first.
- The fixed-address access is the only `unsafe` here: one small module in the
  firmware crate's glue (as `storage.rs` holds `FlashStorage::new`), with
  volatile copies in and out. Everything else works on bytes through
  `eveningstar-records` and is tested on the host.

### Format

- Header: magic, total length, a write counter (`u32`), the head index, and
  the repeat count of the latest crash ("Wear").
- A ring of 4 entries. Each: length, CRC-32 (over length, boot ID, sequence
  number and record), the boot ID of the boot that wrote it, a sequence
  number from the write counter, and one record.
- A writer stores the entry's bytes and CRC first, and then the head index,
  one aligned `u32` store. An interrupted write tears at most that entry,
  which its CRC then rejects.
- **Clearing** empties the entries and keeps the write counter and the
  repeat count. A chip reset, a header that doesn't parse, or a reset reason
  of `None` starts the mailbox afresh.

### The boot ID

A random `u64`, drawn from the RNG right after `esp_hal::init` and before
anything else in `main`, kept in a static, and stamped into every mailbox
entry and log record of the boot. It is the boot's identity for duplicate
checks and for grouping records; it isn't secret.

### Writers

- **Every boot**, right after drawing the boot ID: a `Boot` record with what
  is known at once (running slot from the MMU, version and ELF SHA-256 from
  its own descriptor, the hardware reset reason, the recorded PC). The
  storage glue completes it (boot state, tier) once it has read the otadata.
- **The panic handler**, first, before the defmt log: a `Panic` record and a
  `Reset` record (cause: panic). It formats the message through a
  truncating `fmt::Write` that can't panic, and an atomic guard resets at
  once if the handler is entered again, since a panic before the supervisor
  starts would otherwise hang a board with no reset button.
- **`SoftwareReset::reset`**: a `Reset` record with its `ResetCause`.
  `ResetCause::GracefulReboot` gains the reason's permanent code, through a
  trait the firmware's reboot reasons implement, so the record names the
  reason.
- **The confirmation window**, when it ends a trial: a `TrialEnd` record with
  the reason, at the request. The graceful reboot that follows can end in a
  different reset (a watchdog, a power cut); the records then show both.
- Writers outside the panic handler write inside a critical section, which
  esp-sync's re-entrant implementation allows from anywhere.

### Reading

- Early in `main`, before anything can write it, the firmware copies the
  mailbox's valid entries into RAM.
- The diag store moves them into the log once it has the writer, then
  clears the mailbox. A log record keeps the entry's boot ID and sequence
  number; an entry whose pair is already in the log isn't written again, so
  a reset between the log write and the clear duplicates nothing.
- In the unverified tier there is no writer of `diag`: the entries stay in
  the mailbox, and the API serves them from RAM. A board flashed with
  `cargo run` is in that tier, and relies on this.

## The log

### Format

- 64 sectors used as a ring. Each sector starts with a header: a magic that
  never changes, a sector sequence number (`u32`), and a CRC-32 over both. A
  torn header fails its CRC; such a sector doesn't parse.
- Records follow back to back: length, type, value, CRC-32 over length, type
  and value. Each value carries a record sequence number (`u32`), the boot
  ID, and the uptime in milliseconds; a wall-clock time is added as a field
  once the firmware knows the time. A length of `0xFFFF` ends a sector's
  data, and `0xFFFF_FFFF` is never used as a sequence number.
- **Mount:** among the sectors whose header verifies, the one with the
  highest sequence number is the head. The append point is the first byte
  after its last record whose CRC matches. When the bytes from there on
  aren't all `0xFF` (a torn record), the sector counts as full and appending
  continues in the next one.
- **Rotation:** a full head moves on to the next sector, which is erased and
  then given its header; the oldest records go.
- A sector whose header doesn't parse is erased only when the ring reaches
  it, so a firmware never wipes history it merely can't read.
- An interrupted erase or program can leave bits that read differently each
  time. A record or header is trusted only through its CRC; a sector whose
  tail isn't cleanly erased is never appended to again; and the mount
  reads each header twice and takes a sector as the head only when both
  reads agree.
- Sequence numbers can't wrap in the device's life: 64 sectors at 100 000
  erases each are 6.4 million sector numbers. Clearing the log restarts
  them.

### Owner and failures

- A new host-buildable crate, `eveningstar-diag`, holds the store. It
  depends on `eveningstar-storage` and `eveningstar-records`, never on the
  application, and takes the `Writer<Diag>` the firmware drops today.
- The log is best effort. A failed read or write is logged over defmt and
  reported by the API; it never blocks a boot, an update or a confirmation.
- It never holds configuration values or secrets. The one exception is the
  panic message, which formats values the code chose (decision 6).

### Wear

- A crash loop: a `Panic` or `Reset` record equal to the latest crash in the
  log (same type, cause and location) isn't written again, and neither are
  the `Boot` records of those boots. The mailbox header counts the repeats;
  a `Repeat` record (type, count of crashes and boots) is written when the
  count reaches a power of two and when the loop ends.
- A record is 40 to 200 bytes. Without the rule, a loop of one boot per
  second writes about three records per boot and cycles the ring once or
  twice an hour, which the flash's endurance (about 100 000 erases per
  sector) bears for years; the rule keeps the history readable.

## Records

The first types. Every code is permanent.

- **`Boot`**: boot ID, hardware reset reason (the raw code), the PC the
  debug assist recorded (after a watchdog reset), running slot, version,
  the app descriptor's ELF SHA-256 (first 8 bytes), storage tier, and boot
  state (`Valid`, `PendingVerify`, fallback).
- **`Reset`**: the firmware's cause (`GracefulReboot` with the reason's code,
  `ImmediateReboot`, `MissedDeadline` with the task's name, at most 16
  bytes, `Panic`), uptime, version.
- **`Panic`**: the file (its last 48 bytes, as registry paths begin with
  nothing useful), line and column, and the message (at most 96 bytes).
- **`TrialEnd`**: why the trial ended (a required self-check failed, with its
  ID; the deadline passed), the trial image's version, and the otadata
  sequence number of its entry.
- **`Ota`**, written directly: update refused (the refusal code), activated
  (versions from and to, the otadata sequence number), confirmed, switched,
  trial extended.
- **`SelfCheck`**, written directly during a trial: the check's ID and its
  result.
- **`Rollback`**: see below.
- **`Repeat`**: the repeated record's type and the counts.

## Rollback reasons, end to end

The old image writes its `Ota` "activated" record, with the new entry's
otadata sequence number, before it writes that entry. A rollback is then
recognised at any later boot by the latest "activated" record in the log
that has no later "confirmed" or `Rollback` record, when the otadata entry
with that sequence number is `Aborted`:

- the running slot is the other one: the bootloader rolled back;
- the running slot is the trial's own: the previous image didn't load, and
  the bootloader fell back to the trial image (`specs/ota-signing.md`,
  "Rollback and recovery").

The firmware then writes one `Rollback` record: the sequence number, both
versions (the trial's from its `TrialEnd` record or, failing that, the
alternate slot's descriptor), which of the two cases, and the cause, in this
order:

1. a `TrialEnd` record of the trial's boots (from the mailbox, or already in
   the log): its reason;
2. a `Panic` or `Reset` record of the trial's boots: the panic or the missed
   deadline;
3. a `Boot` record of the next boot with a chip reset reason: power lost
   during the trial;
4. otherwise: a reset with no message, with the reset reason.

Keying on the "activated" record, not on the sequence number alone, keeps it
to one record per rollback: a `flash-all` restarts the otadata's sequence
numbers, but a later activation writes a newer "activated" record. When the
"activated" record has rotated out of the log, nothing is written, rather
than a wrong record. The web UI shows the latest `Rollback` record, for
example: "Update to 1.5.0 rolled back: required self-check `config
migration` failed."

## Across firmware versions

- The encoding's rules cover both directions.
- The mailbox's address and header, the log's sector header, and the record
  encoding are permanent once boards ship; changing them needs the user's
  approval, like the layout.
- Committed fixtures: a mailbox and a log sector of every released format,
  which every later firmware must read; and a "future" mailbox and sector,
  with an unknown type and every value longer than known, which must read as
  without the additions.

## API (sketch)

- `GET /api/v1/diag`: records, newest first, paged by record sequence number.
  Each request copies one bounded page of records into a RAM buffer under
  the store's lock, and the answer is formatted from that copy, as picoserve
  formats a body twice and needs the same bytes both times. Each record has
  its sequence number, boot ID, uptime, type and fields; an unknown type
  shows its type and length only.
- The latest `Rollback` and `Reset` records also appear in `GET /firmware`
  and `GET /system`, for the web UI's banners.
- Mailbox entries not yet moved to the log (unverified tier) appear in
  `GET /diag` marked as such.
- `DELETE /api/v1/diag` erases the log, with the highest permission tier: 64
  sector erases, with its own watchdog handle fed per erase and a yield
  between them. It erases the head last, so a cut clear leaves a log that
  mounts at its old head with fewer records.

## Verification

- **Host, encoding:** fuzz targets for the container parser, the mailbox
  parser and the log mount; the fixtures above; every truncation and every
  bit flip of a mailbox and of a sector reads without a panic and yields no
  record whose CRC failed.
- **Host, log:** a property test on the test flash: for any sequence of
  appends, rotations and clears, with a power cut at any program or erase,
  torn sector headers and partly erased sectors included, and the affected
  bits unstable (read 0 or 1 at each read), the mount finds
  every record whose write completed, in order, and nothing torn.
- **Host, mailbox:** random bytes in the mailbox's place never produce an
  entry; a torn entry is dropped; a duplicate drain writes nothing.
- **Host, rollback inference:** for each case above (rollback, fallback,
  power cut, `flash-all` between runs, the "activated" record rotated out),
  exactly the expected `Rollback` records.
- **Board, retention:** a confirmation of TRM 8.1.1 with a test build: the
  mailbox survives a software reset, a panic, MWDT0, the RWDT system reset,
  a reset through USB Serial/JTAG (probe-rs, esptool), and not a power
  cycle; a brownout if one can be provoked safely. Also, whether the
  recorded PC after a watchdog reset points at the hang.
- **Board, end to end:** a panic and a missed deadline appear in `GET /diag`
  after the reboot; a panic before `esp_rtos::start`, inside a critical
  section and inside an interrupt handler, and a panic inside the panic
  handler, each reset the board and leave their record; a crash loop that
  never reaches the storage split is recorded by the first boot that does,
  with `Repeat` records; a trial that fails a required self-check, one that
  reaches its deadline, one cut by a power cycle, and one whose previous
  image doesn't load, each produce one `Rollback` record with the right
  cause, also after a `flash-all` between runs; a power cut during
  `DELETE /diag` leaves a log that mounts.

## Decisions for the user

Each with a recommendation.

1. **Two layers.** Recommended: the mailbox in LP RAM and the log in `diag`.
2. **Mailbox size and place.** Recommended: the last 1 KiB of LP RAM, fixed
   for good, by a linker fragment of the firmware's own. Alternative for a
   minimal start: an LP_AON store register (as ESP-IDF's reset reason hint
   uses), which holds one 32-bit word and needs no linker work.
3. **Mailbox entries.** Recommended: a ring of 4.
4. **Log format.** Recommended: a ring of sectors with CRC-checked headers,
   records with a CRC-32 each, a torn tail ending the sector.
5. **Crash loops.** Recommended: repeated crashes and their boots coalesced,
   with `Repeat` records at powers of two.
6. **The panic message.** It formats values the code chose, so it may one
   day hold something sensitive. Recommended: kept, at most 96 bytes, and
   served only with the highest permission tier; the other fields with the
   read tier.
7. **The shared encoding.** Recommended: one crate for the image information
   block, the mailbox and the log, so the compatibility rules are written
   and tested once.
8. **A `Boot` record at every boot.** Recommended: yes, except during a crash
   loop.
9. **Where the `unsafe` lives.** Recommended: one module in the firmware
   crate's glue, volatile copies only, all parsing safe and on the host.
10. **Clearing the log.** Recommended: `DELETE /diag` with the highest
    permission tier, and no clearing at a factory reset unless the user asks
    for it then.
11. **Unverified tier.** Recommended: the mailbox is read and served but not
    cleared, as there is no writer of `diag`.
12. **Reboot reasons across the trait boundary.** Recommended:
    `ResetCause::GracefulReboot` carries the reason's permanent `u16` code.

## Left open

- Backtraces: the panic record holds the location and the message; a stack
  walk is later work.
- Wall-clock time in records, once the firmware has a time source.
- Priorities in the log (crash records kept longer than routine events), if
  64 sectors prove too few.
- How the diagnosis and status tree reads the log and the advisory
  self-checks.
