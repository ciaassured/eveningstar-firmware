# Storage: what is left

Status: implemented. How it is built and why is in `docs/architecture.md`
("Storage"), and the rules for working on it are in `CLAUDE.md` ("Storage").
This file keeps what those don't: the work that builds on the storage layer, and
what hasn't been verified.

> **The storage layer's checks are what keep a bad write from the boot chain,
> the running app and the secrets.** Don't relax a tier, a protected region or
> the split between sensitive and public handles without asking the user.

## Stop and ask the user when

1. You want to change the layout in `nix/board.nix` once boards have shipped.
   The layout can't change over OTA.
2. You want to remove a bootloader from `known.rs`. That takes boards out of the
   full tier.
3. You want to relax a trust tier, the protected regions, or the sensitive and
   public split of the handles, or add a route to the internal flash around
   them.
4. A change needs `Ota` to write anything but the other slot and the otadata.

## Left to build

- **Config store.** It owns the `Config` handle (`into_nor_flash`) and keeps
  the running slot's settings, secrets included, in 128 KiB. It reads the other
  slot's configuration (`OtherConfig`) to migrate after an update and can erase
  it. Whether the store accepts records zeroed in place (NOR programs only clear
  bits) is open.
- **Diag store.** It owns the `Diag` handle. Crash records are backtraces, not
  memory, and events such as updates and reboots, likely as a priority ring
  buffer. Capturing a crash (likely an LP RAM record copied to `diag` on the
  next boot) was deferred earlier.
- **OTA service.** It owns `Ota` and decides how updates arrive and are
  authenticated: `VerifiedImage` means intact and meant for this board, not
  authentic. Its confirmation policy:
  - After a boot with `BootState::needs_confirmation`, health checks run in a
    window of 10 minutes, and the service calls `Ota::confirm` when they pass.
  - On timeout it requests a graceful reboot in every case, including when
    there is no rollback target. A board then reboots every 10 minutes until it
    is confirmed. That is accepted so that a user can confirm whichever image
    comes up.
  - Today only the storage glue acts on an unconfirmed image, and only a
    `PendingVerify` one below the full tier (`Report::cannot_confirm`).
- **API and web UI views** of the layout, the report (tier, limit, table
  comparison, bootloader, running slot) and the boot state, and of the reason
  for a reboot. `Reason::CannotConfirm` is already in the API.
- **T6, the untrusted tier's reporting.** The firmware runs on default
  configuration that it doesn't save (Ethernet with DHCP) and logs why. The API
  and the status LED don't say so yet.
- **A sanitized flash dump**, if it is wanted: each store owns an allowlist
  sanitizer for its partition and fails closed, so a sensitive partition never
  leaves as raw bytes, and the endpoint needs authentication.
- **The MD5 of the partition table** is not checked. Checking it, and the erased
  entry that the ESP-IDF bootloader requires after the MD5 entry, would make the
  comparison equal to the bootloader's instead of slightly more lenient. It is
  an option, not a need: the comparison is exact on the entries.
- **The external NAND** is out of scope here. It will hold bulk data logs.

Out of scope until the user asks: updating the bootloader or the partition
table from the firmware, flash encryption and secure boot (the build forbids
both), image signatures, and factory provisioning of the `factory` partition.

## Not verified yet

Verified on the dev board: the booted offset for both slots; the first boot
after `flash-all` writing otadata entry 0 with sequence 1 and `Valid`
(`testdata/otadata-first-boot.bin`); a `New` entry booting as `PendingVerify`
and, unconfirmed, rolling back with its entry `Aborted`; `split` taking about
17 ms; `cargo run` giving the untrusted tier; and T7's reboot, indirectly.

Not verified:

- A real `Ota` update, activation and `confirm()` on a board, with power cuts.
  The power-cut tests run on a RAM flash and a simulated bootloader.
- The interrupt latency of sector erases next to Ethernet traffic.
- The stack high-water mark with `esp-storage`'s 4 KiB read buffer.
- The warning logged when the bootloader fell back from a corrupt selected
  image.
- T7 with its log captured. The reboot was seen, not the log line.
- The path where MMU entry 0 is invalid, so the booted offset is unknown and
  the tier is untrusted.
