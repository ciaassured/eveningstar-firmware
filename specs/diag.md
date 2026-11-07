# Diagnostics: messages across reboots and firmware versions

Status: proposed 2026-11-08, revised twice the same day after review; for
the user's review. No code is written until the user approves it. Builds on
`specs/storage-v2.md` (the `diag` region and its store) and
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

1. **A crash, a hang, a missed watchdog deadline or the end of a trial is
   recorded, and survives every reset but a chip reset.**
   - Held by: the mailbox, written at the moment, in memory TRM 8.1.1 says
     every reset but a chip reset keeps; the main watchdog's interrupt stage
     for a hang.
   - Shown by: the board's retention matrix across every reset reason
     (brownout included), and the fault catalogue's records read back over
     the API.
2. **The history survives power loss, and a cut write loses at most the
   record being written.**
   - Held by: the log's per-record CRC, the mount rules, words programmed
     once.
   - Shown by: the host model of the log with torn and unstable flash, and
     board power cuts during a drain and during a rotation (the power-cut rig
     of `specs/ota-signing.md`, which switches the board's DC input).
3. **Every firmware version reads every other's records.**
   - Held by: the record encoding's rules.
   - Shown by: the committed fixtures of every released format and the
     "future" fixtures; a committed registry of every type's minimum length
     and field offsets, which CI compares with the crate, so a release can't
     raise a minimum or move a field; the board rollback scenarios, where the
     old image reads the new one's mailbox.
4. **A rollback is recorded once, with its cause.**
   - Held by: the "activated" record and the inference rules, run before
     anything can change the otadata.
   - Shown by: the host inference tests (bootloader abort, the firmware's own
     rollback, fallback, power cut, a switch during a trial, an update begun
     straight after a rollback, `flash-all` between runs, the "activated"
     record rotated out), and the board's rollback scenarios.
5. **Diagnostics never block a boot, an update or a confirmation, and never
   hang the board.**
   - Held by: best-effort writes; the panic handler's re-entry guard; the
     watchdogs.
   - Shown by: the fault catalogue with the `diag` region full of garbage,
     a full head sector at boot (one 40–400 ms erase in start-up), and a
     panic inside the panic handler.

## Sources read

- ESP32-C6 Technical Reference Manual v1.2: chapter 8 (reset: 8.1.1 and table
  8.1-1), 12.4.4 (brownout), 18.4.2 (the debug assist's PC recording).
- ESP32-C6 errata v1.3; the XM25QH64C datasheet (7.2.14 Page Program, reset
  and power-up), as in `specs/ota-signing.md` "Hardware facts".
- esp-hal 1.2.2 (`ld/esp32c6/memory.x`, `linkall.x`, `rtc_fast.x`,
  `build.rs`, `src/rtc_cntl/rtc/esp32c6.rs`, `src/system.rs`,
  `src/soc/esp32c6/lp_core.rs`, `src/timer/timg.rs`), esp-riscv-rt 0.15.0
  (`__pre_init`), esp-rtos 0.4.0, esp-sync 0.3.0, esp-storage 0.10.0, and the
  esp32c6 PAC 0.24.0.
- ESP-IDF 5.5.2: `bootloader/Kconfig.projbuild`,
  `bootloader_support/src/bootloader_common_loader.c` (the retained RTC
  struct), `bootloader_esp32c6.c`, `bootloader_mem.c`,
  `esp_hw_support/port/esp32c6/cpu_region_protect.c`,
  `esp_system/port/soc/esp32c6/reset_reason.c` (the reset reason hint),
  `espcoredump/src/core_dump_flash.c`, and the bootloader's and ROM's linker
  scripts.
- Flash logs elsewhere: littlefs's design notes, Zephyr's NVS and FCB, and
  ESP-IDF's NVS.
- This repository: `main.rs`, `reset.rs`, `watchdog.rs`, `ResetCause` and
  the reboot service (`eveningstar-system`), `services/reboot.rs`,
  `storage/confirmation.rs`, `eveningstar-ota`, the otadata planner, the
  `Diag` role, and `Session::program`.

What they establish:

- **Retention.** The ESP32-C6 has four resets: CPU, core, system and chip.
  "All reset types mentioned above (except Chip Reset) preserve the data
  stored in internal memory" (TRM 8.1.1). A chip reset is power-on (0x01).
  The bootloader enables the brownout detector's system reset
  (`bootloader_esp32c6.c`), which keeps memory (0x0F); esp-hal leaves it.
  Software reset (0x03), the MWDT and RWDT core resets, USB Serial/JTAG
  resets and the RWDT and super watchdog system resets (0x10, 0x12) keep it
  too. Every code in table 8.1-1 is in esp-hal's `SocResetReason`.
- **Nothing else writes the top of LP RAM.** LP RAM is 16 KiB at
  `0x5000_0000`. The bootloader's and the ROM's linker scripts place nothing
  there. esp-hal, esp-rtos, esp-storage, rtt-target and embassy reference it
  nowhere, except esp-hal's LP core loader, which zero-fills all 16 KiB; the
  firmware doesn't use the LP core. esp-riscv-rt zeroes `.rtc_fast.bss` at
  every boot and `.rtc_fast.persistent` at reset code 0x01, and touches
  nothing else. probe-rs's ESP32-C6 target maps no memory there.
- **The bootloader's reservation.** With `BOOTLOADER_RESERVE_RTC_MEM` on, the
  ESP-IDF bootloader keeps a CRC-checked struct at the top of LP RAM, where
  the mailbox goes. The option is selected by `BOOTLOADER_FACTORY_RESET`,
  `BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP` and
  `BOOTLOADER_CUSTOM_RESERVE_RTC`, all off by default and not pinned today.
  Pinning them off (`specs/ota-signing.md` decision 16) is a precondition of
  the mailbox.
- **LP RAM is writable in any panic.** The bootloader's PMP leaves
  `0x5000_0000`–`0x5000_4000` readable, writable and executable; esp-hal
  adds only its own regions. Stores to it are plain data-bus stores.
- **Errata (v0.2).** CPU-863: a misaligned load followed by a store across
  PMP regions with different permissions (flash and LP RAM differ) can
  fault; the mailbox's copies use byte or aligned word accesses. CPU-206
  (late commits of stores to LP RAM) is fixed in v0.2; a `fence` before a
  reset keeps the intent in the code anyway.
- **The flash.** Page Program programs "previously erased (FFh)" locations;
  nothing in the datasheet allows programming a location twice, so the log
  programs each word once after an erase. An interrupted program or erase
  leaves data undefined, and bits that may read differently each time.
- **esp-hal's linker setup.** `memory.x` comes from esp-hal; nothing in this
  repository can shrink `RTC_FAST` itself. A fragment of the firmware's own,
  passed with `-T` from its build script (which already passes
  `-Tdefmt.x`), can place a `NOLOAD` output section at a fixed address
  inside it.
- **Where the firmware resets.** In the panic handler, and through
  `SoftwareReset`, which the reboot service (`GracefulReboot`,
  `ImmediateReboot`) and the supervisor (`MissedDeadline { task }`) use.
  `GracefulReboot` carries no reason today, though the service has it at the
  reset. MWDT0 resets the system without a software hook when the supervisor
  itself stalls; esp-hal can give its stage 0 an interrupt
  (`MwdtStageAction::Interrupt`).
- **The debug assist's recorded PC is gone by `main`.** TRM 18.4.2: it keeps
  the PC across a CPU reset, but the bootloader enables recording again at
  every boot, so by the time the firmware runs it holds a current PC.
- **A flash write from a panic is possible** (ESP-IDF's core dump erases and
  programs from its panic handler, with the cache off), and is avoided here
  for the reasons under "Two layers".
- **`diag`** is 256 KiB (64 sectors), shared by both slots, `Sensitive` (no
  reader, no snapshot), with a writer in the layout and boot tiers, never
  published raw. Today the firmware drops that writer. `Session::program`
  takes whole words only.

## Two layers

- **The mailbox, in LP RAM.** Written at the moment something goes wrong: a
  panic, a hang, a missed watchdog deadline, the reboot that ends a trial.
  The log isn't written then: a flash erase takes up to 400 ms, the fault
  may be in the flash path itself, and a crash loop would spend erase cycles
  on every reset. LP RAM costs a few stores. Every memory survives all but a
  chip reset; LP RAM is chosen because nothing else writes its top.
- **The log, in `diag`.** Durable history: early in each boot the firmware
  moves the mailbox's entries into the log, and writes a `Boot` record and
  the OTA events directly.

## The record encoding

The mailbox and the log use the record encoding `specs/ota-signing.md`
defines for the image information block ("The image's own records"): the
`eveningstar-records` crate, containers of type-length-value entries padded
to 4 bytes, unknown types skipped, values that grow at the end only, meanings
that never change, and the committed registry. Older firmware reads what
newer firmware wrote (after a rollback), and newer reads older (after an
update). On top of it, fixed for good:

- **Integrity:** mailbox entries, log records and log sector headers carry a
  CRC-32, the otadata's (`esp_rom_crc32_le` with an initial value of
  `0xFFFF_FFFF`, as `eveningstar-esp-formats` implements it).
- **Magics:** `0x424D_5345` ("ESMB") for the mailbox and `0x4C44_5345`
  ("ESDL") for a log sector.
- **Codes:** enumerations that only gain values; a reader shows a value it
  doesn't know as its number. Every code a record carries is fixed for good:
  reboot reasons, self-checks (each check gets a permanent ID), refusal
  reasons, trial end reasons, reset causes.

## The mailbox

### Placement

- The last 1 KiB of LP RAM, `0x5000_3C00` to `0x5000_4000`: a `NOLOAD`
  output section placed there by a linker fragment of the firmware's own,
  with an `ASSERT` that esp-hal's `.rtc_fast.*` sections end below it. The
  address and the format are a permanent contract between every firmware
  version, like the partition layout.
- The bootloader options that reserve the top of LP RAM are pinned off, and
  `_check-sdkconfig.py` fails when `BOOTLOADER_RESERVE_RTC_MEM` is set.
  Using the LP core needs the mailbox moved first.
- The fixed-address access is the only `unsafe` here: one small module in the
  firmware crate's glue (as `storage.rs` holds `FlashStorage::new`), with
  volatile byte or aligned word copies in and out. Everything else works on
  bytes through `eveningstar-records` and is tested on the host.

### Format

- A header written once and never changed: magic, total length, CRC-32 over
  both.
- A ring of 4 entries. Each: length, a sequence number (`u32`), the boot ID
  of the boot that wrote it (0 when written before the boot ID exists), a
  repeat count, one record, and a CRC-32 over all of it.
- **Validity** is the header's magic and CRC, and each entry's CRC; nothing
  else. The reset reason is recorded, never used to discard: random bytes
  after a power-on don't pass a CRC-32.
- **The head** is the valid entry with the highest sequence number, as with
  the otadata. A writer writes the next slot, the entry's CRC last, so an
  interrupted write tears at most that entry. There is no index to update.

### The boot ID

A random `u64`, drawn from the RNG right after `Board::new`, kept in a static,
and stamped into every mailbox entry and log record of the boot. It groups a
boot's records and identifies a mailbox entry in the log; it isn't secret.
Entries written before it (a panic in the first statements of `main`) carry
0.

### Writers

Each entry carries the boot's hardware reset reason, running slot, version
and uptime, so a crash loop that never reaches the storage split is still
described.

- **The panic handler**, first, before the defmt log: a `Panic` record. It
  formats the message through a truncating `fmt::Write` that can't panic,
  and an atomic guard resets at once if the handler is entered again, since
  a panic before the supervisor starts would otherwise hang a board with no
  reset button.
- **The main watchdog's interrupt stage.** MWDT0's stage 0 becomes an
  interrupt whose handler writes a `Hang` record with the interrupted PC
  (`mepc`); its stage 1 resets a second later, and RWDT stays the backstop.
  This is the only record of a hang the supervisor itself didn't see. Its
  limits, as the panic handler's: nothing is recorded while interrupts are
  off, or while the flash cache is off inside a program or erase, which
  only the next boot's reset reason shows.
- **`SoftwareReset::reset`**: a `Reset` record with its `ResetCause`.
  `ResetCause::GracefulReboot` gains the reason's permanent code, through a
  trait the firmware's reboot reasons implement.
- **The confirmation window**, when it ends a trial: a `TrialEnd` record with
  the reason. The graceful reboot that follows can end in a different reset
  (a watchdog, a power cut); the records then show both.
- **Coalescing:** a writer whose record equals the head's (same type, cause
  and location) writes it again in the next slot with the repeat count plus
  one, instead of a new entry with a count of 1.
- Writers outside the panic handler and the watchdog interrupt write inside
  a critical section, which esp-sync's re-entrant implementation allows
  from anywhere; a `fence` precedes a reset.

### Reading

- Early in `main`, before anything can write it, the firmware copies the
  mailbox's valid entries into RAM.
- Once it has the writer, the diag store moves them into the log and then
  clears the entries up to the highest sequence number it moved, keeping
  any written since.
- A log record keeps the entry's boot ID and sequence number. An entry whose
  pair is in the head sector (or the sector before it, after a rotation) isn't
  written again, so a reset between the log write and the clear duplicates
  nothing.
- In the unverified tier there is no writer of `diag`: the entries stay in
  the mailbox, and the API serves them from RAM. A board flashed with
  `cargo run` is in that tier, and relies on this.

## The log

### Format

- 64 sectors used as a ring. Each sector starts with a header: the magic, a
  sector sequence number (`u32`), and a CRC-32 over both. A header is read
  back after it is programmed, before the sector takes a record.
- Records follow, each a container entry whose value starts with a record
  sequence number (`u32`), the boot ID and the uptime in milliseconds, and
  ends with a CRC-32 over the whole entry. A length of `0xFFFF` ends a
  sector's data, and `0xFFFF_FFFF` is never used as a sequence number.
- Every word is programmed once after its sector's erase; nothing is
  programmed twice.
- **Every read verifies the CRC:** the mount, the duplicate check, each API
  page. A record or header that fails it doesn't exist.
- **Mount:** among the sectors whose header verifies, the one with the
  highest sequence number is the head. The append point is the first word
  after its last record whose CRC matches. When the words from there on
  aren't all `0xFFFF_FFFF` (a torn record), the sector counts as full and
  appending continues in the next one.
- **Rotation:** a full head moves on to the next sector, which is erased and
  then given its header; the oldest records go.
- A sector whose header doesn't verify is erased only when the ring reaches
  it, so a firmware never wipes history it merely can't read.
- 64 sectors at 100 000 erases never reach `2^32` sequence numbers. Clearing
  the log restarts them.

### Owner and failures

- A new host-buildable crate, `eveningstar-diag`, holds the store. It
  depends on `eveningstar-storage` and `eveningstar-records`, never on the
  application, and takes the `Writer<Diag>` the firmware drops today.
- The log is best effort. A failed read or write is logged over defmt and
  reported by the API; it never blocks a boot, an update or a confirmation.
- It never holds configuration values or secrets. The one exception is the
  panic message, which formats values the code chose (decision 6).

### Wear

- A crash loop is coalesced in the mailbox. The drain writes the loop's
  record with its count when the count reaches a power of two and when the
  loop ends, and skips the `Boot` records of the boots in between.
- A record is 40 to 200 bytes. Without the rule, a loop of one boot per
  second, at two records per boot, would cycle the ring several times an
  hour, which the flash's endurance (100 000 erases per sector) still bears
  for years; the rule keeps the history readable.

## Records

The first types. Every code is permanent.

- **`Boot`**, written to the log by the diag store: hardware reset reason,
  running slot, version, the app descriptor's ELF SHA-256 (first 8 bytes;
  zero in images built without it), storage tier, and boot state (`Valid`,
  `PendingVerify`, fallback).
- **`Panic`**: the file (its last 48 bytes; release builds remap the build
  path with `--remap-path-prefix`), line and column, and the message (at
  most 96 bytes).
- **`Hang`**: the interrupted PC.
- **`Reset`**: the firmware's cause (`GracefulReboot` with the reason's code,
  `ImmediateReboot`, `MissedDeadline` with the task's name, at most 16
  bytes).
- **`TrialEnd`**: why the trial ended (a required self-check failed, with its
  ID; the deadline passed), the trial image's version, and the otadata
  sequence number of its entry.
- **`Ota`**, written to the log directly: update refused (the refusal code),
  activated (versions from and to, the otadata sequence number), confirmed,
  switched.
- **`Rollback`**: see below.

## Rollback reasons, end to end

The old image writes its `Ota` "activated" record, with the new entry's
otadata sequence number, before it writes that entry; "confirmed" and
"switched" records name the sequence number they end. Right after the
storage split, before the boot writers go to `Ota` (where an update would
replace the evidence), the diag store drains the mailbox and then looks at
the latest "activated" record with no later "confirmed", "switched" or
`Rollback` record for its sequence number. When the otadata entry with that
sequence number exists and is `Aborted` or `Invalid`, the trial was rolled
back:

- `Aborted`, and the running slot is the other one: the bootloader ended
  the trial;
- `Invalid`: the firmware ended the trial (the confirmation window's
  rollback at the deadline);
- the running slot is the trial's own: the previous image didn't load, and
  the bootloader fell back to the trial image (`specs/ota-signing.md`,
  "Rollback and recovery").

A switch during a trial also leaves `Invalid`, but its "switched" record
ends the activation first. An activation whose entry was never written (a
power cut between the record and the entry) has no entry with that
sequence number, and counts as nothing.

The firmware then writes one `Rollback` record: the sequence number, both
versions (the trial's from its `TrialEnd` record or, failing that, the
alternate slot's descriptor), which of the cases, and the cause, in this
order:

1. a `TrialEnd` record of the trial's boots: its reason;
2. a `Panic`, `Hang` or `Reset` record of the trial's boots: that;
3. a chip reset reason at this boot: power lost during the trial;
4. otherwise: a reset with no message, with the reset reason.

A `flash-all` restarts the otadata's sequence numbers, but a later
activation writes a newer "activated" record, so an old record never
matches a new rollback. When the "activated" record has rotated out of the
log, nothing is written, rather than a wrong record.

## Across firmware versions

- The encoding's rules cover both directions.
- The mailbox's address and format, the log's sector header, and the record
  encoding are permanent once boards ship; changing them needs the user's
  approval, like the layout.
- Committed fixtures: a mailbox and a log sector of every released format,
  which every later firmware must read; and a "future" mailbox and sector,
  with an unknown type and every value longer than known, which must read as
  without the additions.

## API (sketch)

- `GET /api/v1/diag`: records, newest first, paged by record sequence number,
  each page copied into RAM under the store's lock. Each record has its
  sequence number, boot ID, uptime, type and fields; an unknown type shows
  its type and length only. Mailbox entries not yet moved to the log
  (unverified tier) appear marked as such.
- The latest `Rollback` and `Reset` records also appear in `GET /firmware`
  and `GET /system`.
- `DELETE /api/v1/diag` erases the log, with the highest permission tier: 64
  sector erases, with its own watchdog handle fed per erase and a yield
  between them, the head last, so a cut clear leaves a log that mounts at
  its old head with fewer records.

## Verification

- **Host, encoding:** fuzz targets for the container parser, the mailbox
  parser and the log mount; the fixtures; the registry comparison; every
  truncation and every bit flip of a mailbox and of a sector reads without
  a panic and yields no record whose CRC failed.
- **Host, log:** a property test on the test flash: for any sequence of
  appends, rotations, drains and clears, with a power cut at any program or
  erase, torn sector headers and partly erased sectors included, and the
  affected bits unstable (read 0 or 1 at each read), the mount finds every
  record whose write completed, in order, and nothing torn.
- **Host, mailbox:** random bytes in the mailbox's place never produce an
  entry; a torn entry is dropped; coalescing counts; a drain that is cut and
  repeated writes nothing twice; entries written during a drain survive its
  clear.
- **Host, rollback inference:** the cases listed under requirement 4, each
  producing exactly the expected `Rollback` records.
- **Board, retention:** with a test build, the mailbox across a software
  reset, a panic, MWDT0, the RWDT system reset, a reset through USB
  Serial/JTAG (probe-rs, esptool), a brownout from the power-cut rig's slow
  ramp (code 0x0F expected, mailbox kept), and a full power cycle (lost).
- **Board, end to end:** a panic and a missed deadline appear in `GET /diag`
  after the reboot; a hang with interrupts on gives a `Hang` record with a
  PC inside the hanging function; a panic before `esp_rtos::start`, inside a
  critical section and inside an interrupt handler, and a panic inside the
  panic handler, each reset the board and leave their record; a crash loop
  that never reaches the storage split is recorded by the first boot that
  does, coalesced; each rollback case of requirement 4 produces one
  `Rollback` record with the right cause, also after a `flash-all` between
  runs; power cuts during a drain, a rotation and `DELETE /diag` leave a log
  that mounts with every completed record.

## Decisions for the user

Each with a recommendation.

1. **Two layers.** Recommended: the mailbox in LP RAM and the log in `diag`.
2. **Mailbox size and place.** Recommended: the last 1 KiB of LP RAM, fixed
   for good, by a linker fragment of the firmware's own.
3. **Mailbox entries.** Recommended: a ring of 4, the head found by sequence
   number.
4. **Log format.** Recommended: a ring of sectors with CRC-checked headers,
   padded records with a CRC-32 each, every word programmed once, a torn
   tail ending the sector.
5. **Crash loops.** Recommended: coalesced in the mailbox, written to the log
   at powers of two.
6. **The panic message.** It formats values the code chose, so it may one
   day hold something sensitive. Recommended: kept, at most 96 bytes, and
   served only with the highest permission tier; the other fields with the
   read tier.
7. **The shared encoding.** Recommended: the one `specs/ota-signing.md`
   defines, with its committed registry.
8. **Hangs.** Recommended: MWDT0's stage 0 as an interrupt that records the
   PC, stage 1 resetting a second later.
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

- Backtraces: the panic and hang records hold a location; a stack walk is
  later work.
- Wall-clock time in records, once the firmware has a time source.
- How the diagnosis and status tree reads the log and the advisory
  self-checks.
