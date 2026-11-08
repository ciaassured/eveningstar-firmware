# OTA updates with signed images

Status: proposed 2026-11-07, revised 2026-11-08 with the user's decisions
(listed under "Decisions"). No code is written until the user approves it.
Builds on `specs/storage-v2.md` ("OTA") and on `eveningstar-ota` as merged
(`Update::finish`, `VerifiedImage`, `Ota::activate`, `Ota::verify_other`,
`Ota::roll_back`).

## Fixed by the user

- No secure boot and no flash encryption, ever. The board belongs to its
  owner, who can reflash it over USB at any time. This firmware never writes
  an eFuse, and nothing is burnt to stop a fork the owner installs from
  doing so (user, 2026-11-08).
- Every image that arrives remotely is verified for integrity and source. No
  code path skips that verification: nothing reaches a slot remotely except
  through an update, an update's image becomes loadable only once it
  verified, and its slot selectable only after it verified again. Images
  already installed are trusted (decision 28).
- The format is OpenBSD signify's: any release is checked on a desktop with
  stock `signify`.
- The primitives come from existing, reviewed code. Nothing here invents
  cryptography; the only new code is format parsing and the composition
  described below.
- A well-known development key lives in the repository and never rotates.
- The API reports the trusted keys and the OTA state. The web UI warns plainly
  when the running image trusts the development key.
- An update must never leave the device unrecoverable because of a bug in the
  new image (user, 2026-11-07). Rolling back during the trial, automatically
  or on request before confirmation, returns to the image that ran before
  whatever its keys, signature or secure version. Selecting the other slot's
  image some time after confirmation is a different operation.
- The other decisions taken with the user are listed under "Decisions",
  "Decided with the user".
- Anyone can take the source at a release's tag, audit it, build it, and
  get the signed image byte for byte, so nobody has to trust our build
  process (user, 2026-11-08).
- Accepted for now: the device has no API authentication and sits on an
  isolated network. Anyone who reaches the API can upload, confirm and
  switch slots.

## Requirements

What the design has to hold, each with the mechanism that holds it and what
shows it, other than the authors. The rest of the spec is the detail.

1. **Only a verified image runs from a remote update.**
   - Held by: the held first word and the two verifications ("Byte path");
     `Signed` as the only value that activates; the trust set compiled in.
   - Shown by: stock signify and OpenBSD's own verification code agreeing
     with the verifier on every case (differential tests and fuzzing); the
     trust set constants compared with `nix/keys.nix` by the `lint` group;
     Wycheproof and RFC 8032 vectors; the model check of the update
     protocol; mutation testing of the verifier and the policy; board
     refusals read back with `esptool`.
2. **A trial always ends: confirmed, or rolled back to the image that ran
   before, whatever that image's keys, signature or `secure_version`.**
   - Held by: the bootloader's abort of `PENDING_VERIFY`; the confirmation
     window's own rollback at the deadline; the supervisor.
   - Shown by: the model check, with the real bootloader's rules and torn
     and unstable flash; the real bootloader on the board, under the
     power-cut rig and the catalogue of broken images.
3. **Our firmware never takes USB recovery from the owner.**
   - Held by: no route to eFuse programming, and no reconfiguration of the
     USB pins or the USB Serial/JTAG pad, in the firmware ("eFuses"); a
     release image trusts only our release keys.
   - Shown by: the link map's cross-reference table holding no reference to
     the ROM's eFuse programming functions; `release-check`'s constant scan
     of the release ELF against a committed allowlist; bad fixtures each of
     these refuses; the lints. These guard against mistakes, not hostile
     code.
4. **A confirmed image can take the next update.**
   - Held by: the OTA readiness self-check; a known-answer self-check of the
     new image's own verifier; an upload refused unless its image trusts the
     key that signed it.
   - Shown by: the board scenarios, each confirmed release taking the next,
     and a release whose verifier is broken on purpose failing its trial.
5. **No downgrade below the floor, except by USB or during a trial.**
   - Held by: the `secure_version` checks at upload and at a switch after
     confirmation; development builds at 0.
   - Shown by: unit tests over every combination, mutation testing, board
     scenarios.
6. **A release can't trust the development key, and its secret key can't be
   in the repository.**
   - Held by: the trust set generator, `release-check`, the device's upload
     check, the secret key scan.
   - Shown by: those tools refusing generated bad fixtures in CI.
7. **Anyone can rebuild a release from its source and get the signed
   image, byte for byte.**
   - Held by: a release is a function of its tag alone. `nix build
     .#release` takes nothing else: `flake.lock` pins nixpkgs and
     rust-overlay, `rust-toolchain.toml` the compiler, `Cargo.lock` and the
     npm lockfiles every dependency (the web UI bundle is part of the
     image); the build has no network, no secret and no host state; the
     descriptor's date comes from `SOURCE_DATE_EPOCH`, and the vendor,
     toolchain and source paths are remapped (`nix/app.nix`). Signing
     changes no byte of the image: the signature lives in `SHA256.sig` and,
     on the device, in the slot's signature sector, both outside the image.
   - Shown by: anyone's own build from the tag, checked against the
     published, signed listing with stock signify:

     ```
     nix build github:ciaassured/eveningstar-firmware/v<version>#release
     signify -C -p eveningstar-<N>-fw.pub -x SHA256.sig \
       result/eveningstar-<version>-esp32c6.bin
     ```

     and, before a release is signed, a second build on another machine and
     a `nix build --rebuild` in CI, each matching byte for byte; no release
     is signed until they do (decision 44).

## Sources read

Every claim below about another system comes from these, read for this spec.

- OpenBSD `usr.bin/signify` at master: `signify.c` 1.137, `zsig.c` 1.21,
  `crypto_api.c` 1.1, `mod_ed25519.c` 1.1 (SUPERCOP 2013-04-19 `ref`),
  `sc25519.c`, `fe25519.c`, `signify.1` 1.64; `usr.sbin/sysupgrade` and
  `usr.sbin/fw_update` at master; `distrib/sets/lists/base/mi`.
- nixpkgs `signify` 32 (aperezdc portable): `signify.c` 1.136, `zsig.c` 1.19,
  `mod_ed25519.c` 1.1. Its Ed25519 code is OpenBSD's, file for file.
- `/etc/signify` keys `openbsd-80-{base,fw,pkg,syspatch}.pub` and the 8.0
  `amd64/SHA256.sig`. `signify -V -e -p openbsd-80-base.pub -x SHA256.sig`
  verifies it (23 lines, all `SHA256 (name) = hex`); the 79 base key and the
  80 fw key both fail with "checked against wrong key".
- Ted Unangst, "signify: Securing OpenBSD From Us To You", BSDCan 2015.
- RFC 8032 sections 5.1.7, 7.1, 8.4, 8.7 and 8.8.
- Wycheproof `testvectors_v1/ed25519_test.json`: 210 tests, 117 valid, 64
  invalid, 29 acceptable (flags `SmallOrderPublicKey`, `MixedOrderPublicKey`).
- ed25519-dalek 3.0.0 and curve25519-dalek 5.0.0 (both 2026-07-06): README,
  CHANGELOG; libsignify 0.6.0 `Cargo.toml`; ed25519-compact 2.6.0 and salty
  0.3.0 READMEs; the Quarkslab blog post of the 2019 dalek audit; RustSec
  advisories for `curve25519-dalek`, `ed25519-dalek` and `sha2`.
- ESP-IDF 5.5.2 (the copy in the dev shell): `bootloader_support/src/`
  `esp_image_format.c`, `bootloader_utility.c` (selection, rollback, the
  fallback loop), `bootloader_common_loader.c` (otadata validity);
  `app_update/esp_ota_ops.c` (confirm, rollback, anti-rollback);
  `efuse/src/esp_efuse_fields.c` (secure version); `bootloader/`
  `Kconfig.projbuild` and `Kconfig.app_rollback`.
- OpenBSD `distrib/miniroot/install.sub`, `usr.sbin/syspatch/syspatch.sh` and
  `libexec/reorder_kernel/reorder_kernel.sh` at master.
- esp-hal 1.2.2 `src/lib.rs` (`init`), and this repository's
  `crates/eveningstar/src/watchdog.rs` and `main.rs`.
- TUF specification 1.0.37, section 1.5.2; Samuel, Mathewson, Cappos,
  Dingledine, "Survivable Key Compromise in Software Update Systems", CCS
  2010, sections 3 and 6.
- MCUboot design document and `boot/bootutil/src/image_validate.c`,
  `image_ed25519.c` at main.
- The board's resolved bootloader configuration (`nix build .#sdkconfig`),
  ESP-IDF's `bootloader_esp32c6.c`, `bootloader_init.c`, `bootloader_flash.c`
  and `app_update/test_apps/test_app_update/main/test_switch_ota.c`;
  esp-rom-sys 0.1.5's ROM linker scripts; the hardware repository's
  schematics (`psu`, `usb`, `mcu`, `flash`); the XM25QH64C datasheet; the
  ESP32-C6 errata v1.3; and the board itself (`esptool flash-id`,
  `read-flash-status`, `espefuse summary`).

## What the sources establish

### signify

- Keys and signatures are two lines: `untrusted comment: <text>\n` and one
  base64 line ending in `\n`. The decoded structures are a public key
  `"Ed" | keynum[8] | A[32]` (42 bytes), a signature
  `"Ed" | keynum[8] | R[32] | S[32]` (74 bytes), and an encrypted secret key
  (`"Ed" "BK" | rounds | salt | checksum | keynum | seckey`, bcrypt_pbkdf with
  42 rounds unless `-n`).
- The signature is pure Ed25519 over the whole message (`signmsg`,
  `verifymsg`). signify reads the message into memory (`readmsg`, up to
  1 GiB).
- `verifymsg` compares the signature's keynum with the key's before any
  arithmetic and fails with "checked against wrong key". The keynum is 8
  random bytes; the paper calls it a check for "accidental key signature
  mismatches". It is not a security property.
- `-e` puts the message after the signature's two lines. `-C` verifies an
  embedded signature, then parses each line with
  `sscanf("%31s (%1023[^)]) = %223s")`, and fails the whole run on a line
  that doesn't parse. With file arguments it checks only the named files and
  reports a named file missing from the list as `FAIL`; with none it checks
  every listed file, so an empty list verifies.
- `-C` accepts `SHA256` and `SHA512` lines, hex or base64 (`recodehash`),
  and ignores anything after the hash's first whitespace.
- `-z` signs a header of `date=`, `key=`, `algorithm=SHA512/256`,
  `blocksize=65536` and one SHA-512/256 per 64 KiB block of the gzip data
  after the gzip header, embedded in the gzip comment. Verification releases
  each block only after its hash matches. The payload has to be gzip.
- `-t keytype` restricts the key taken from the untrusted comment to
  `/etc/signify/*-keytype.pub`. Without `-p`, the comment chooses the key
  file, so the comment is untrusted only because the key it names must still
  verify the signature.
- The verifier is SUPERCOP's 2013 `ref` code. `crypto_sign_ed25519_open`:
  - reduces S modulo L (`sc25519_from32bytes`, Barrett reduction) and never
    checks S < L, so S + kL verifies too;
  - decodes A with `ge25519_unpackneg_vartime`, which rejects only a y with
    no square root, so small-order and mixed-order keys are accepted;
  - checks the cofactorless equation by packing `[S]B - [k]A` and comparing
    32 bytes with R, so a non-canonical R never verifies.
- `parseb64file` uses `b64_pton`, which skips whitespace inside the base64.
  The comment line has to be at least one character after the header and
  under 1024.

### OpenBSD's keys

- Keys are separated by purpose: `base` (release sets, `SHA256.sig`), `fw`
  (`fw_update` passes `-p openbsd-${version}-fw.pub`), `pkg` and `syspatch`.
  The 8.0 `SHA256.sig` checked against the 8.0 fw key fails on the keynum.
- The paper: "After each release of OpenBSD, we generate a new key pair for
  the release after next. That's plus two." Release N ships N's keys and
  N+1's, so it verifies the release after it, and "if you upgrade every
  release, you will have an unbroken chain of keys".
- The tree today ships the keys of 7.3 to 8.1 (`distrib/sets/lists/base/mi`):
  old keys stay, and 8.1's is present before 8.1 exists. `sysupgrade` takes
  the key name from the untrusted comment, requires it to match
  `^openbsd-[0-9]{2,3}-base.pub$`, and fetches a missing one from a
  `sigbundle` verified with `signify -Vz` by a key already installed.
- The paper on recovery: "Revocation is probably a cure worse than the
  disease", and rotation "will automatically cycle out the bad key". There
  are "no key servers ... No web of trust. Just keys." It doesn't describe
  where secret keys are kept.
- The paper signs checksums rather than files: "it's those checksums that
  are signed ... Assuming SHA256 checksums cannot be forged, this then
  creates a chain of trust."

### RFC 8032 and Wycheproof

- 5.1.7 requires 0 <= S < L and allows the cofactorless check. 8.4: without
  the S < L check signatures are malleable. 8.8: implementations that
  disagree on the valid set enable fingerprinting.
- 8.7: "the receiver MUST buffer the entire message ... to be verified",
  and "any Initialize Update Finalize (IFU) verification interface is prone
  to misuse."
- Wycheproof marks small-order and mixed-order public keys `acceptable`: a
  verifier may accept or reject them.

### Rust candidates and their audits

- **ed25519-dalek 3.0.0** on **curve25519-dalek 5.0.0**, `sha2` 0.11 (the
  workspace's version). `no_std`, no allocation for verification, and no
  `unsafe` in ed25519-dalek. `verify` rejects S >= L unless the
  `legacy_compatibility` feature is on; `verify_strict` also rejects
  small-order A and R. A streaming `verify_stream` exists since 2.2.0, behind
  the `hazmat` feature.
- **Audits.** Quarkslab audited the dalek libraries for Tari Labs in
  June–August 2019 (30 days, two engineers). In scope: `subtle`,
  curve25519-dalek's Edwards, Ristretto and Scalar modules with the u64 and
  AVX2 backends, and bulletproofs. x25519-dalek and ed25519-dalek got "a
  quick, marginal look". The post names no crate versions. It found no
  critical issue; the curve25519-dalek changelog records the fix of the
  unreduced `Scalar` arithmetic it found.
- **Not audited, as far as these sources show:** ed25519-dalek's 2.x and 3.x
  API (rewritten after 2019, including `verify_strict`); curve25519-dalek's
  32-bit backend, the one the ESP32-C6's `riscv32imac` uses (the audit
  covered u64 and AVX2); curve25519-dalek 4.x and 5.x; `sha2`; esp-hal's SHA
  driver and the SHA accelerator. RustSec has one advisory per crate:
  ed25519-dalek RUSTSEC-2022-0093 (signing oracle before 2.0, signing only),
  curve25519-dalek RUSTSEC-2024-0344 (timing of `Scalar29::sub`, fixed in
  4.1.3, a side channel on secret scalars, not verification), sha2
  RUSTSEC-2021-0100 (AVX2 backend miscomputed in 0.9.7, x86 only).
- **libsignify 0.6.0** (the Rust signify port): requires `alloc`, pulls in
  ed25519-dalek 2, sha2 0.10 and bcrypt-pbkdf, and has no audit. Not usable
  without an allocator.
- **ed25519-compact 2.6.0**: offers incremental verification; its README
  mentions no audit. **salty 0.3.0** (TweetNaCl transcription): "not
  audited" in its README.
- No C library was evaluated against its audit claims for this spec.

### The ESP-IDF bootloader without secure boot

`image_load` in `esp_image_format.c` (5.5.2), for an app partition:

- It checks the header magic, the chip ID and revision range
  (`bootloader_common_check_chip_validity`), the segment count, each
  segment's header and load address, and the XOR checksum.
- It verifies the appended SHA-256 only when the header's `hash_appended` is
  set (`process_image_header`: `SECURE_BOOT_CHECK_SIGNATURE ||
  hash_appended`). An image without it boots on the checksum alone.
- It skips the checksum and the SHA-256 when a debugger is attached
  (`esp_cpu_dbgr_is_attached()`).
- `BOOTLOADER_SKIP_VALIDATE_ON_POWER_ON` and `BOOTLOADER_SKIP_VALIDATE_ALWAYS`
  (both default off) skip validation entirely. `nix/bootloader` leaves them
  at their defaults; it doesn't pin them.
- The appended SHA-256 only detects corruption: anyone who rewrites the
  image recomputes it. The anti-rollback check
  (`BOOTLOADER_APP_ANTI_ROLLBACK`) compares `secure_version` with an eFuse,
  which this project never burns.

### TUF, Samuel et al. and MCUboot

- TUF 1.0.37 lists ten attacks: arbitrary installation, endless data,
  extraneous dependencies, fast-forward, indefinite freeze, malicious mirrors
  preventing updates, mix-and-match, rollback, key compromise, and wrong
  software installation. It requires offline root keys and separate keys per
  role.
- Samuel et al. name three things an updater must authenticate: the content
  of updates, their timeliness (a freeze attack withholds them), and the
  repository's state (metadata inconsistency). Principles: responsibility
  separation, multi-signature trust, explicit and implicit revocation, and
  minimizing each key's risk.
- MCUboot validates an image in its slot: SHA-256 over header, body and
  protected TLVs, and by default an Ed25519 signature over that 32-byte
  digest (pure Ed25519 over the image only with `MCUBOOT_SIGN_PURE`, which
  needs the image memory-mapped). Downgrade prevention compares versions
  (software) or a security counter that "must be stored in a non-volatile
  and trusted component". `MCUBOOT_VALIDATE_PRIMARY_SLOT_ONCE` is documented
  as weaker because flash changed after validation would run unverified.

## Hardware facts

Read from the development board (2026-11-08, `esptool` and `espefuse`), its
schematic (the hardware repository's `pcb/mcu.kicad_sch`), and the
datasheets. A claim that rests on one of these says so.

- **Chip:** ESP32-C6 (QFN40), revision v0.2, in an ESP32-C6-WROOM-1-N8
  module; USB Serial/JTAG on the USB-C port.
- **Flash:** XMC XM25QH64C (JEDEC ID `20 40 17`), 8 MiB, inside the module.
  Its datasheet: more than 100 000 program/erase cycles, more than 20 years'
  retention, page program 0.5–3 ms, sector erase 40–400 ms. An interrupted
  program or erase: "data corruption may happen", with no defined partial
  state. The status register's one-time lock exists only on special order;
  its block protection bits are non-volatile but writable.
- **Flash status register:** `0x0200` (`esptool read-flash-status`): no
  block protection, `SRP` 0, `QE` 1. No permanent lock exists on this part
  (the one-time lock is a special order, the power-supply lock clears at a
  power cycle, `/WP` is IO2 while `QE` is set), and `esptool` clears block
  protection. Partial page programs are allowed, which the held first word
  relies on.
- **eFuses:** none of the security fuses is burnt: secure boot, flash
  encryption, download mode, USB Serial/JTAG and its download mode are all
  as shipped, and `WR_DIS` is 0, so every one of them is still writable, by
  any firmware.
- **Power:** the board runs only from its 2-pin DC input, through a fuse,
  a TVS diode and a Schottky diode into a buck converter (LMR38020, with
  22 µF before and 3 × 47 µF after it); USB VBUS powers nothing. A power cut
  is a ramp through those capacitors into the brownout reset, which the
  bootloader arms (`bootloader_esp32c6.c`) and the firmware never disables.
- **Recovery without USB:** the debug header J2 carries the reset line, the
  UART and GPIO9 (labels in `mcu.kicad_sch`; to confirm against the
  netlist). GPIO9 held low at reset enters download boot (TRM table 9.2-2).
  The user button is GPIO21, not a strapping pin.
- **Resolved bootloader configuration** (`nix build .#sdkconfig`):
  `APP_ROLLBACK_ENABLE` on; `APP_ANTI_ROLLBACK`, `SKIP_VALIDATE_*`,
  `FACTORY_RESET` and `CUSTOM_RESERVE_RTC` off; `WDT_ENABLE` on at 9000 ms;
  `FLASH_XMC_SUPPORT` on (the start-up sequence for this flash).
- **Slow clock:** esp-hal defaults the LP slow clock to the internal RC_SLOW
  (136 kHz), and the firmware never changes it. Errata CLK-8588 (the
  internal 32 kHz RC may not start) therefore doesn't touch the RTC
  watchdog, which counts the slow clock. A release never selects the
  internal 32 kHz RC, and moving to the 32 kHz crystal needs a start-up
  check with a fallback, as ESP-IDF's `rtc_clk_init` does (advisory
  AR2024-011 covers changing the slow clock over OTA).
- **Errata CPU-863** (a misaligned load followed by a store across PMP
  regions can fault): rustc emits no misaligned accesses for
  `riscv32imac`, and the ROM's libc isn't linked; only C code linked later
  could be exposed. CPU-206 and SPI-304 are fixed in v0.2.

## Threat model

The attacker reaches the API over the network. The device has no API
authentication, so the attacker can call every endpoint; what stops them is
the trust set compiled into the running image. Physical access (USB, the
debug header, desoldering the flash) is the owner's right and out of scope:
whoever has it can reflash anything, by design.

### Handled

- **Tampering** (TUF: arbitrary installation, wrong software). An image or
  signature changed in transit or on the server fails the signature check
  or the digest comparison, both made over the bytes as they lie in the
  flash, not as they arrived.
- **Wrong signer.** Only keys compiled into the running image verify. The
  keynum selects the key; the signature must then verify under it. A key of
  another purpose is never in the firmware trust set.
- **Downgrade** (TUF: rollback). An image whose descriptor `secure_version`
  is lower than the running image's is refused for an update, and for a
  switch once the running image is confirmed (decision 7). During a trial
  nothing is refused: the bootloader's rollback and a switch both return to
  the previous image. Ordinary versions may go down within one
  `secure_version`. The floor binds only the firmware's own paths: USB
  flashing always restores any image.
- **Replay.** Uploading a signed image again installs the same image again.
  It is harmless within the downgrade rule.
- **Wrong board or layout.** The image's chip ID and revision range (header),
  project name (descriptor), compiled layout (layout record) and board
  (a new board entry, decision 8) are read from the verified bytes and
  compared. All of them are covered by the signature, because the signed
  digest covers the whole image.
- **Endless data.** The image is bounded by the slot less its signature
  sector, the signature file by 4 KiB, before anything is written.
- **Leaked or lost release key.** The next release is signed with key N+1,
  which every image signed with N already trusts, and trusts N+1, N+2 and
  N+3 only. Devices that install it stop trusting N. A lost key is replaced
  the same way. A future key can't be regenerated (installed images trust
  its public key), so every generated key gets both offline copies at once;
  one lost anyway is marked `lost`, and the trust set becomes the next
  three generations from `signing` that aren't `lost` or `compromised`.
- **Unsigned untrusted comment.** The device never reads it: it doesn't pick
  the key, isn't stored apart from the signature file, and isn't shown. The
  release instructions always pass `-p`, so the desktop doesn't use it
  either.
- **Taking the board from its owner.** Firmware that runs can burn eFuses:
  enable secure boot with its own key, enable flash encryption, or disable
  download mode or USB Serial/JTAG, each permanently. Our releases contain no
  route to it ("eFuses"), and a release image installs only images signed by
  our release keys. What remains is accepted: a board running a development
  build accepts firmware anyone signs with the public development key, and a
  stolen release key could sign firmware that burns them. Nothing is burnt
  in advance to prevent it, since that would also stop the owner from
  installing a fork that uses secure boot.
- **Leftovers of a refused upload.** A refused image may be a well-formed
  ESP image with a correct appended SHA-256, or an upload may be cut by a
  power loss before verification. Neither is installed: `begin_update`
  removes every otadata entry naming the slot before its first erase, and
  only the activation of a verified image writes one again, so a switch
  refuses the slot ("Rollback and recovery"). Not installed isn't the same
  as not bootable: the bootloader's fallback loop boots any image that
  loads, without consulting the otadata, when the selected one fails to
  load. So the image's first word, which holds its magic (`0xE9`), stays in
  RAM until the image has verified, and the slot's first word stays erased
  until then. The bootloader checks the magic on every load, a debugger
  attached or not (with validation skipping pinned off, decision 16), so an
  unverified image is never loadable: not while it is written, not after a
  refusal, an abandoned upload or a power cut. Erasing the slot's first
  sector after a refusal is then only for the API's sake (decision 15).

### Accepted

- **Freeze** (TUF: indefinite freeze). The device doesn't fetch updates; the
  owner pushes them. Nothing tells it a newer release exists, and signed
  expiry needs a trusted clock, which it doesn't have (SNTP is
  unauthenticated). The API shows the running image's version and build date.
- **Fast-forward.** No metadata versions; `secure_version` comes only from
  signed images, so only the signer can raise it.
- **Mix-and-match, extraneous dependencies.** One image per update; the
  manifest's other lines are ignored.
- **Malicious mirrors.** No mirrors; the owner chooses the source.
- **Compromise of the current release key before devices update.** Until a
  device installs the release signed with N+1, whoever holds N installs
  anything on it. No revocation list (as OpenBSD: "a cure worse than the
  disease"). With no API authentication, nothing else stands in the way.
- **The development key.** Its secret key is public. Any image that trusts it
  installs whatever anyone signs with it. Release images don't trust it, and
  the web UI warns on every image that does.
- **A release that trusts the development key by mistake.** Signed with a
  release key, it would open every device that installs it to anyone holding
  the public development secret. Three layers stop it: the trust set
  generator refuses a release set holding the development key; the ceremony
  reads the image's trust entry before signing; and every firmware refuses
  an update whose signer is a release key and whose trust entry holds the
  development key (decision 34). If one were installed anyway (by a firmware
  from before the rule), the recovery is a fixed release with a raised
  `secure_version`, so the flawed one can't be switched back to after
  confirmation; the release keys stay valid, since none leaked. A device
  that was already given hostile firmware through the development key is
  recovered over USB.
- **No threshold signatures.** One key signs. signify has no multi-signature
  form, and a second scheme would break the desktop check.
- **Writes outside the firmware.** A probe-rs or esptool write after
  verification isn't seen by the firmware's write state. No firmware write
  can reach the slot between verification and activation (`Signed` holds
  its writer, and activation checks the epoch), and both run in one request;
  a write from outside needs physical access, which is out of scope, so the
  slot isn't hashed again (decision 5). The bootloader's SHA-256 check still
  catches an uncoordinated change at boot, except with a debugger attached.
- **Switching with no authentication.** Until authentication exists, anyone
  on the network can switch to the installed image in the other slot. Once
  it exists, switching needs the highest permission tier.
- **Returning to an installed image.** Whoever may switch can start the
  other slot's image, when its `secure_version` isn't below the running
  image's once that is confirmed, and with it that slot's configuration,
  credentials included. During a trial the previous image is always
  reachable, whatever its `secure_version`. The exposure is one image,
  verified when it arrived or flashed over USB, and the configuration it
  last had.
- **The floor follows the running image.** After a trial rollback the floor
  is the older image's. Without an eFuse there is nowhere trustworthy to
  keep a higher one.

## Signed format

### Options

- **Sign the image directly** (`signify -S -m image.bin`, or `-e` with the
  image as the embedded message).
  - Ed25519 hashes `R | A | M` with SHA-512, so the 3 MiB message has to be
    streamed: ed25519-dalek's `verify_stream` behind `hazmat`, or buffering
    the image, which needs 3 MiB of RAM. RFC 8032 8.7 calls a streaming
    interface "prone to misuse".
  - The whole image goes through software SHA-512 (no SHA-512 in the
    ESP32-C6's accelerator), seconds per verification.
  - The signed message is only the image: version, target and layout come
    from inside it anyway.
  - Desktop: `signify -V -p key.pub -m image.bin`.
- **A signed manifest of hashes** (`signify -C`, OpenBSD's `SHA256.sig`).
  - The Ed25519 message is a short text, verified whole in RAM with the
    ordinary `verify_strict`.
  - The image is bound through its SHA-256, which the slot's check already
    computes from the flash with the accelerator (`VerifiedApp::file_sha256`).
  - The same file signs several artefacts (image, ELF, web bundle) as
    OpenBSD's does, and it is the pattern MCUboot uses by default (signature
    over the digest).
  - Needs a small text parser on the device.
  - Desktop: `signify -C -p key.pub -x SHA256.sig image.bin`.
- **signify's gzip mode** (`-z`).
  - Bounded memory and per-block release before writing, but the payload must
    be gzip: the device would inflate (a 32 KiB window, another parser) and
    the slot would still have to hold the inflated image, so the block
    hashes protect bytes that are not the ones stored.
  - The header's `date=` and `key=` lines are signed but carry nothing the
    device can check.

### Recommendation: the signed manifest

The signature file is what `signify -S -e` writes for a SHA256 listing:

```
untrusted comment: verify with eveningstar-1-fw.pub
RWS...base64 of "Ed" | keynum | R | S...
SHA256 (eveningstar-1.4.0-esp32c6.bin) = 6b1f...64 lowercase hex...
```

The device accepts a subset of what signify accepts, never more:

- The whole file is at most 4096 bytes and holds no NUL.
- Line 1 is `untrusted comment: ` followed by 1 to 1023 bytes and `\n`, the
  range signify accepts. Its content is ignored.
- Line 2 is exactly 100 characters of canonical base64 (one `=`, zero bits
  in the padding), then `\n`, decoding to 74 bytes that begin with `Ed`.
- The rest is the message, 1 to 2048 bytes. Each line is
  `SHA256 (<name>) = <64 lowercase hex>\n`, single spaces, `<name>` of
  1 to 128 bytes from `[A-Za-z0-9._+-]`. Any other line fails the file.
- The message must contain exactly one line whose digest equals the slot's
  file SHA-256 (decision 2). That line's name is reported.

The signed message carries the image's SHA-256 directly. Version
(`secure_version` and the version string), target (chip ID and revision range,
project name, board entry) and layout (layout record) are bytes of the
image, so the digest covers them; the device reads them from the image it
verified, never from the manifest or the file name.

### The desktop command

```
signify -C -p eveningstar-1-fw.pub -x SHA256.sig eveningstar-1.4.0-esp32c6.bin
```

prints `Signature Verified` and `eveningstar-1.4.0-esp32c6.bin: OK`. The file
argument is required: without it an empty or unrelated list verifies. Debian
and Ubuntu install the same program as `signify-openbsd`. The public key comes
from the repository at the release's signed tag and from the release notes.

## Trust set and rotation

### Compiled into the image only

- The public keys live in `keys/` as signify `.pub` files, one per
  generation (`eveningstar-<generation>-fw.pub`) and the development key
  (`eveningstar-dev-fw.pub`, with its `.sec`). Every key stays there for
  good, retired ones included, so any old release can still be checked on a
  desktop, as OpenBSD keeps its old keys in `/etc/signify`.
- `nix/keys.nix` is the one place that says which key is which, as
  `nix/board.nix` is for the layout:

  ```nix
  eveningstar.keys = {
    dev = ../keys/eveningstar-dev-fw.pub;
    generations = {
      "1" = { file = ../keys/eveningstar-1-fw.pub; };
      "2" = { file = ../keys/eveningstar-2-fw.pub; };
      "3" = { file = ../keys/eveningstar-3-fw.pub; lost = true; };
      "4" = { file = ../keys/eveningstar-4-fw.pub; };
      "5" = { file = ../keys/eveningstar-5-fw.pub; };
    };
    signing = 2; # the generation releases are signed with now
  };
  ```

  A generation below `signing` is past, `signing` is current, the next two
  usable ones are future; a generation may be marked `compromised = true`
  or `lost = true`, and is then skipped (here 3, so the example trusts 2, 4
  and 5). A release trusts `signing` and the next two usable generations; a
  development build trusts the same three and the development key. Rotating
  is one edit: raise `signing` and add the next generation's file.
- The generator refuses: a gap in the generations; a missing file for the
  three usable generations from `signing`; a duplicate keynum. A generation
  marked `lost` or `compromised` is skipped, never trusted again. The
  README's key table (generation, status, public key line) is generated
  from the same file and checked by the `lint` group.
- A generated, committed module holds the trust sets as constants, checked
  up to date by the `lint` group like `layout.rs`. The type of a trust set
  has no public constructor, so the generated constants are the only trust
  sets.
- Nothing at run time adds, removes or reorders a key: no API, no
  configuration, no file in flash. A new trust set arrives only inside a
  signed image.
- The generator also refuses a key that `VerifyingKey::is_weak` flags
  (small order), one that isn't torsion-free (mixed order), a non-canonical
  encoding, and a release set that holds the development key. Keys
  `signify -G` makes always pass.
- Each image also embeds its trust set in a record (decision 9), described
  under "The image's own records".

### The image's own records

The firmware already places the layout record right after the app
descriptor, through the `.flash.appdesc.*` input sections esp-hal's linker
script puts at the start of the first segment, and reads it from a fixed
offset (`image::app_layout`). The other facts an update is judged by follow
it in one block, the image information, at the offset right after the
layout record. esp-hal's `.flash.appdesc` output section keeps the
`.flash.appdesc.*` input sections in no defined order, so the layout record
and the block are one `static` in one input section, the record first;
`check-images` checks both offsets.

The block's problem is the configuration's: an older firmware reads the
block of a newer image. So it uses one encoding that never changes and grows
only by addition, which `specs/diag.md` also uses for its mailbox and log;
it is defined here, once, and lives in a new host-buildable crate,
`eveningstar-records` (`no_std`, no allocation, `#![forbid(unsafe_code)]`,
in `host_crates`):

- **Container:** magic (`u32`) and total length (`u16`), little-endian. No
  format version: the rules below make one unnecessary. The block is at
  most 1 KiB.
- **Entries:** each a type (`u16`), a length (`u16`) and that many bytes of
  value, zero-padded to a multiple of 4 (the length counts the padding),
  one after the other, up to the total length.
- **Unknown types are skipped.** A newer image adds a type, and older
  firmware ignores it.
- **A value only grows.** Its fields are fixed-size and at fixed offsets.
  Each type has a minimum length, set when the type is first released and
  never raised. A later version appends optional fields; a reader takes the
  fields it knows that the value holds, reads a later field the value ends
  before as absent, and ignores bytes past what it knows. Only a value
  shorter than the type's minimum is refused, so a newer firmware still
  reads an older image's block.
- **A field never changes meaning.** A change of meaning is a new type. For
  a transition both types are written; the old one is dropped once no
  supported release needs it.
- **Each reader names the types it requires.** A missing one, or a type that
  appears twice, refuses the upload.
- **The registry.** The crate exports every type's number, minimum length
  and field offsets; a committed copy is compared in CI, so a release can't
  raise a minimum or move a field.

The types now:

- **Board:** board name and the revisions the image supports.
- **Compatibility:** the oldest configuration schema the image migrates
  from, and its own.
- **Trust:** the generation it is signed with (`signing`), and per trusted
  key its generation (0 for the development key), keynum and 32-byte public
  key.

The layout record stays as it is: it is compared byte for byte, and it can
never change over OTA anyway.

The block is bytes of the image, so the signed digest covers it: the running
firmware reads it from the uploaded image as the slot's check verified it,
at the same epoch as the signature. That is how it knows what the new image
trusts before activating it, and checks:

- the key that signed the upload is in the new image's trust entry
  (decision 26);
- a release-signed upload doesn't trust the development key (decision 34);
- a development-signed upload has `secure_version` 0, and a release-signed
  one at least 1 (decision 33);
- the board and the configuration schema fit (decisions 8 and 35).

The API shows each slot's block, and `check-images` and the ceremony print
it. The bootloader ignores it. Tests: the parser is fuzzed; a committed
fixture of every block format ever released parses; and a fixture of a
"future" block, with an unknown type and every value longer than known,
parses to the same result as without the additions.

### Purposes

- One purpose now: `fw`, firmware images. File names follow signify's
  convention: `eveningstar-<generation>-fw.pub` and `eveningstar-dev-fw.pub`.
- A later purpose (a signed configuration bundle, say) gets its own keys and
  its own trust set; the firmware trust set never holds them, so a signature
  of one purpose never verifies another.

### Generations, N to N+2

- Release keys are numbered generations. An image signed with generation N
  trusts N, N+1 and N+2: as OpenBSD release N ships N+1's key, with one more
  generation so a device can skip releases.
- Generations rotate on a schedule whatever happens (decision 11), and at
  once when the signing generation is compromised or lost. The next release
  is signed with N+1 and trusts N+1 to N+3; N+3 is generated in that
  release's ceremony. A compromised or lost future generation is skipped
  instead, and the current one keeps signing.
- A device stays directly updatable while the release it runs trusts the
  signing generation: up to two rotations behind. Further behind, it updates
  through a release in between, or over USB. No `sigbundle`: a key bundle
  would be a second trust path.
- Rotation decides only which releases a device can install. It never
  decides what can be switched to: a switch checks no signature.

### Development and release keys

- The development key is generated once with `signify -G -n` (no
  passphrase); both halves are committed and it never rotates.
- Development builds (the default `cargo` and Nix builds) trust the
  development key and the current release generations, generated from the
  same list so they can't drift apart. A board running a development build,
  one flashed by mistake included, updates over the air to a release, which
  trusts only N, N+1 and N+2 (decision 10): from then on development-signed
  images are refused.
- Development builds always have `secure_version` 0, and releases at least
  1 (decision 33); the build and the ceremony check both. An image that
  trusts the development key accepts firmware from anyone, so it is
  vulnerable by design, and the floor already says so: once a release is
  confirmed, switching to a development build in the other slot is refused
  like any lower `secure_version`, and during the trial it is allowed. No
  signature is checked and no key list is consulted. Uploading a
  development-signed image to a release image is refused anyway: the release
  trusts no development key. The confirm control says: "The image in the
  other slot is a development build. It can't be started again after
  confirmation, except by flashing over USB."
- One build setting chooses the flavour: `EVENINGSTAR_RELEASE`, set only by
  `nix build .#release`. The firmware's build script derives everything
  from it and from `nix/keys.nix` and `nix/release.nix`: the trust set (with
  or without the development key), the trust entry's `signing` (the release
  generation, or 0 for a development build), and `secure_version` (the
  release's value, or 0). `main.rs` passes that `secure_version` to the
  long form of `esp_app_desc!`, so the descriptor's value has one source;
  `ESP_BOOTLOADER_ESP_IDF_CONFIG_SECURE_VERSION`, which the short form
  reads when that crate compiles, isn't used. `nix/release.nix` keeps every
  release's `secure_version`, so a check knows the previous one at the tag.
  `check-images` checks the combination.
- Nix signs every development build with the development key and writes
  `SHA256` and `SHA256.sig` beside the image, so development exercises the
  same path as a release.
- The firmware recognizes the development key by its public key bytes,
  compiled into every build, not by a name. The dev-key flag is "the trust set
  holds those bytes".

### The release key ceremony

- The first ceremony generates N, N+1 and N+2; each rotation generates one
  more. An offline machine (a live system, no network interface up) runs
  `signify -G -p eveningstar-N-fw.pub -s eveningstar-N-fw.sec` with a
  passphrase per key. The secret keys are copied to two offline media kept in
  different places. They are never on a networked machine.
- The public keys are committed in a signed commit, and published in the
  release notes and the README. Each fits on one line (56 base64 characters)
  for comparison by eye or voice.
- Signing a release: the image is built twice from the tag (CI and a local
  Nix build) and the digests compared. The offline machine runs
  `release-check` over the shipped files and `SHA256` listing, with the
  image digest read off CI's page as an independent input, then
  `signify -S -e -s eveningstar-N-fw.sec -m SHA256 -x SHA256.sig`. Only
  `SHA256.sig` leaves it. `docs/releasing.md` is the procedure.
- Reproducible builds are a requirement (requirement 7), not a convenience.
  What would vary is already fixed: the descriptor's build time and date
  come from `SOURCE_DATE_EPOCH`, which Nix sets; `nix/app.nix` remaps the
  vendor, toolchain and source paths, so neither the image nor the ELF
  names the build machine. The rest is rustc's and the linker's own output
  for the same inputs, which the `build` group checks with
  `nix build --rebuild` of the release, and a second machine checks before
  signing. A difference is a bug to fix before the release, found with
  `diffoscope`.

### Secret keys and the repository

Release secret keys never come near the repository; the ceremony keeps them
on offline media. These checks catch a mistake anyway:

- **Content, not names.** A signify secret key's second line is the base64
  of a 104-byte structure beginning `Ed` `BK`: 140 characters beginning
  `RWRCS`. A check scans every file, whatever its name, for such a line, and
  for an `untrusted comment:` line ending in `secret key`. The only match
  allowed is `keys/eveningstar-dev-fw.sec`, and only when its keynum equals
  `keys/eveningstar-dev-fw.pub`'s and its KDF rounds are 0 (the development
  key has no passphrase).
- **Where it runs.** In the `lint` group, so on pre-commit (over the staged
  tree) and in CI; and on pre-push, since a hook can be skipped and a push
  publishes. CI also scans every commit of a pull request, not just its
  tip, so a key added and removed again is still caught.
- **`.gitignore`:** `*.sec` except `keys/eveningstar-dev-fw.sec`, so a
  plain `git add` skips one.
- **Passphrases.** Release keys are generated with a passphrase (42 rounds
  of bcrypt_pbkdf, signify's default); a leaked file alone doesn't sign.
  The ceremony refuses a release secret key with 0 rounds.
- **If one is published anyway**, it is compromised: rewriting history
  doesn't unpublish it. Its generation is marked `compromised` in
  `nix/keys.nix`, and the next release raises `secure_version` (decision
  33). It is signed with the generation after the compromised one when that
  was the signing generation, and with the current one when it was a
  future generation, which is then skipped.

## Rollback and recovery

### The ESP-IDF bootloader, as configured here

The board's bootloader is ESP-IDF 5.5.2 with `BOOTLOADER_APP_ROLLBACK_ENABLE`,
without `BOOTLOADER_APP_ANTI_ROLLBACK` and without secure boot.

- **Selecting.** An otadata entry is valid when its sequence number isn't
  `0xFFFFFFFF`, its CRC matches, and its state is neither `INVALID` nor
  `ABORTED` (`bootloader_common_ota_select_valid`). Of the valid entries the
  highest sequence number wins.
- **Ending a trial.** Before selecting, every entry in `PENDING_VERIFY` is
  rewritten `ABORTED` (`bootloader_utility_get_selected_boot_partition`). Any
  reset during a trial (panic, watchdog, software reset, power cut) therefore
  rolls back at the next boot, and the trial image is never started again
  from its entry. An entry selected in state `NEW` is rewritten
  `PENDING_VERIFY` before its image is loaded.
- **Its own writes can tear.** `write_otadata` erases the sector and
  programs the 32-byte entry in one write. A power cut during it can leave
  the sequence number and CRC whole and the state word partly programmed:
  a valid entry, selecting the trial slot, in a state that is neither
  `PENDING_VERIFY` nor `VALID`, which no later boot aborts. The firmware's
  own writes avoid this by programming the sequence number last; the
  bootloader's don't.
- **Loading.** `bootloader_utility_load_boot_image` tries the selected slot,
  then works down to slot 0 and the factory app, then up through the other
  OTA slots, then the test app, without consulting the otadata. The first
  image that loads boots (header, chip, segments, checksum, appended SHA-256
  when present; checksum and SHA-256 skipped with a debugger attached). When
  none loads: "No bootable app partitions", a software reset, and the same
  again, forever.
- **Keys and versions.** Without secure boot nothing checks a signature.
  Without anti-rollback `check_anti_rollback` returns `true`, so the
  descriptor's `secure_version` plays no part in selecting or loading.

So, on this board, the bootloader's rollback returns to the previous image:

- when it has a lower `secure_version`: yes;
- when it is unsigned, or signed by a key the new image doesn't trust: yes,
  the bootloader knows no keys;
- when it doesn't load (corrupted since): no. The fallback loop skips it and
  boots the next image that loads, which is the trial image itself, now
  `ABORTED`. The firmware sees a fallback boot. If that image has no network,
  only USB recovers the board.

For comparison, ESP-IDF's other modes, never used here: anti-rollback
raises an eFuse only when an image becomes `VALID`, so a trial still rolls
back to a lower secure version, but after confirmation the older image is
skipped even by the fallback loop; secure boot verifies every load, so an
image its keys don't verify is never a rollback target.

Watchdogs: the bootloader arms the RTC watchdog (`BOOTLOADER_WDT_ENABLE`, 9 s
by default). esp-hal's `init` configures the clocks with it still running,
then disables RWDT, MWDT0 and MWDT1. `main` then builds the board, logs (RTT
in its default `NoBlockSkip` mode, which never waits) and builds the timer
groups, and the supervisor's constructor enables MWDT0 and RWDT: straight
line code with no waits, in which a panic resets through the panic handler.
From there the executor has the supervisor's main timeout to start it.

The floor of recovery: USB Serial/JTAG and the ROM download mode are in the
chip's ROM, and this firmware never burns the eFuses that disable them
("eFuses"). A board whose firmware is broken in every slot is still
reflashed over USB.

### eFuses

This firmware never programs an eFuse. What a fork the owner installs does
is the owner's business, so nothing is write-protected in advance.

On the ESP32-C6 an eFuse is programmed in two ways: through the eFuse
controller's registers (`EFUSE` at `0x600B_0800`: `PGM_DATA`, then `CONF`
and `CMD`), or through the ROM's `ets_efuse_program`,
`ets_efuse_clear_program_registers`, `ets_efuse_write_key` and
`ets_secure_boot_revoke_public_key_digest` (`0x4000_0804`–`0x4000_080C`,
`0x4000_0880`). esp-rom-sys defines those ROM symbols for every image;
esp-hal's `efuse` module only reads, through the same registers. The guards:

- **The peripheral.** The board crate takes `EFUSE` out of the peripherals
  `esp_hal::init` returns and keeps it; nothing else receives it. Clippy's
  `disallowed-types` lists `esp_hal::peripherals::EFUSE`, `esp32c6::EFUSE`
  and `esp32c6::efuse::RegisterBlock`, and `disallowed-methods` lists
  `esp_hal::peripherals::Peripherals::steal`; the board crate expects the
  lint where it takes the peripheral.
- **The ROM functions.** The firmware's build script passes
  `--Map=<file> --cref` to the linker, and the release output carries the
  map. Its cross-reference table lists every symbol with the objects that
  reference it, archive members and binary blobs included; the check fails
  on any row for the four names. `--wrap` doesn't serve: with rust-lld
  (LLD 23.1.1) a reference to a symbol a linker script defines links to the
  script's address under the `__wrap_` name, without an error (tested;
  GNU ld does fail). A call through a computed address names no symbol, so
  the constant scan also looks for the four ROM addresses.
- **The registers.** `release-check` disassembles the release ELF, resolves
  every address built from `lui` or `auipc` with `addi` (the `0x600B1` with
  a negative offset form included) and every 32-bit word in `.rodata` and
  `.data`, and reports each in `0x600B_0800`–`0x600B_0BFF` with its function.
  The report must equal a committed allowlist of esp-hal's readers. An
  address carried through memory or an argument isn't resolved, so this
  catches constants, not every route.
- **Fixtures** `release-check` must report: a PAC `EFUSE::steal().cmd()`
  write, a raw volatile write to the controller, an `extern "C"` call of
  `ets_efuse_program`, and a call through its address.
- **The USB link.** The same lints cover `esp_hal::peripherals::USB_DEVICE`
  and the GPIO12 and GPIO13 pins; reconfiguring them, or clearing the USB
  Serial/JTAG pad enable, drops the link before the host can enumerate. The
  board crate keeps them.

These guard against mistakes in our code and dependencies, not against
hostile code, which the trust set keeps off the board. When USB is lost
anyway, J2's reset and GPIO9 lines reach download boot ("Hardware facts").

### OpenBSD

No A/B slots, no trial and no automatic rollback: an upgrade verifies
`SHA256.sig` with the next release's key and overwrites the system in place.
`syspatch` keeps a local, unsigned `rollback.tgz` of what a patch replaces,
and `syspatch -r` restores it at any time without a signature or version
check: what is installed is trusted, only what arrives is verified.

### This design: verify what arrives, trust what is installed

As OpenBSD: an image is verified when it arrives; an image already installed
is trusted.

An image is **installed** in a slot while an otadata entry names the slot
with a valid sequence number and CRC and a state other than `Invalid` or
`Aborted`. Such an entry is written only:

- by activating an image that verified on arrival (`Signed::activate`);
- by confirming or settling the running image (`plan_confirm`,
  `plan_begin_update`), which name the running slot;
- by the bootloader on a board whose otadata is blank, for the slot it
  booted (`set_actual_ota_seq`).

`begin_update` first settles the otadata until both sectors name the
running slot, replacing whatever the other sector held, of any state
(`plan_begin_update`: "Once settled, no valid entry names the other slot"),
and only then erases. So:

- an upload that fails verification, or is cut by a power loss at any point
  before activation, leaves the slot not installed;
- an entry that names a slot, of any state, names bytes the firmware hasn't
  written since: a flipped state byte (the CRC covers only the sequence
  number) can only bring back an image that was installed;
- an image flashed over USB is installed once the bootloader booted it from
  a blank otadata, or while an older entry names its slot: the owner's act;
- an image whose trial ended `Aborted`, or that was rolled back `Invalid`,
  is no longer installed.

Three operations:

1. **Automatic rollback, by the bootloader.** Any reset while the new image
   is `PendingVerify`. The firmware takes no part: no key, signature, secure
   version or configuration is consulted. The previous `Valid` entry boots.
2. **Switching to the other slot**, at any time, in either direction, with
   the highest permission tier (decision 29). The other slot must hold an
   installed image. During a trial that is all: the previous image ran
   here, and it may predate the image information block (the first signed
   update replaces firmware that has none); if it no longer loads, the
   bootloader falls back anyway. After confirmation the target must also
   pass the ESP image check (the slot check's result at the slot's current
   epoch), the project name, the layout record and the board entry, a
   missing board entry reading as unknown and refused; a corrupted image
   is refused, as the bootloader would refuse it. No signature or trust set
   is checked: the image was verified when it arrived, and its signature
   status is shown. Once the running image is confirmed, a target whose
   `secure_version` is below the running image's is refused.
   - During a trial, the running entry is marked `Invalid`, as
     `esp_ota_mark_app_invalid_rollback` does, and the other slot's `Valid`
     entry boots.
   - After confirmation, an entry selecting the other slot with state `New`
     goes to the sector that isn't active, replacing the other slot's older
     entry with a newer one for the same slot. The switch boots as a trial,
     and the running image's `Valid` entry stays: a failed switch rolls back
     to it, and after a confirmed one both slots are still installed, so the
     owner can switch back again. The new entry replaces the target's only
     entry, in the one sector the write can use: a power cut during it
     leaves the running image selected and the target no longer installed,
     though its image is intact. The target is then installed again by
     uploading it as an update.
3. **Updates**, the only way bytes reach a slot remotely: written, verified
   as `Signed`, activated as a trial.

What makes 1 always come for an image that can't be confirmed:

- the confirmation window's graceful reboot at 10 minutes, which the user
  may extend in steps up to 24 hours after boot (decision 31), bounded by
  the reboot service's 5-minute deadline while a download holds it. The
  extension lives in RAM only: any reset during the trial rolls back
  anyway;
- the task watchdog supervisor, backed by MWDT0 and RWDT (exists). The
  confirmation window's handle is registered before the executor starts and
  retired only when the image is `Valid`, so a confirmation task that never
  starts, or stalls, resets the board (decision 24);
- the firmware's own rollback before that reboot: when the deadline passes,
  the running image isn't `Valid` and the other slot is a rollback target,
  the confirmation window marks the running entry `Invalid` (as a switch
  during a trial does) and then reboots. This also ends a trial whose entry
  a torn bootloader write left in an undefined state, which the bootloader
  would otherwise boot again forever (decision 39);
- a power cycle, which needs no tools.

All of these except the bootloader's abort are code of the new image; only
the bootloader stays the same across an update, and it has no timer. An
image whose bug breaks the supervisor itself rolls back at the next power
cycle, unless a torn bootloader write also left its entry undefined, which
then needs USB.

A boot the bootloader fell back to, after the selected image failed to load,
has no rollback target to return to: the running entry isn't the active one.
There the confirmation window only reboots, and a confirmation writes a new
`Valid` entry for the running slot.

What keeps a confirmed image updatable, so a bad image that was confirmed can
also be fixed forward:

- a confirmation arrives only through the API, which proves HTTP works, and
  waits for the required self-checks; a required self-check that OTA is
  ready (boot tier, `Ota::readiness`) is added (decision 25);
- an update is refused when the image's trust entry doesn't hold the key
  that signed it, so whoever signed it can always sign its successor
  (decision 26).

The case of a new key and a raised secure version: image A runs `Valid`,
signed with N, trusts N to N+2, `secure_version` 1. Image B is signed with
N+1, trusts N+1 to N+3, `secure_version` 2.

- A verifies B (N+1 is in A's set, 2 >= 1) and activates it with state
  `New`. At the reboot the bootloader rewrites the entry `PENDING_VERIFY` and
  starts B.
- B lost its network stack. Nobody can confirm it, and its network
  self-check would fail anyway. At 10 minutes B reboots itself (or a
  watchdog or a power cut resets it). The bootloader rewrites B's entry
  `ABORTED`, selects A's `Valid` entry and starts A. A's keys, signature and
  secure version are never consulted. A resumes with its configuration
  untouched and reports that the last update was rolled back (the other
  entry is `ABORTED`).
- B instead passes its required self-checks and its MQTT uploads are
  broken. The confirm page lists the MQTT check as failed (advisory), and,
  because B raises `secure_version`, says that confirming ends the way back
  to A over the network. The owner can extend the trial and investigate, or
  let it roll back.
- If the owner confirms B anyway, A's entry is still `Valid` in the other
  sector, so A is installed, but the switch is refused: 1 < 2. B takes a fix
  signed with N+1; USB restores A. Had B kept `secure_version` 1, the owner
  switches to A: an entry for A with state `New`, a trial boot of A, with
  A's configuration as it was when the update began (decision 27).

### Self-checks

- **Required** checks gate confirmation. They cover what the firmware
  itself guarantees and what recovery needs: storage and the configuration
  migration, the boot tier and OTA readiness, the API answering, the
  watchdog supervisor running, and the verifier's known answers: a built-in
  signature that must verify and one that must not. A failed one means the
  image can't be confirmed and rolls back. The image's own signature sector
  is checked against its own trust set as an advisory check: an image
  installed over USB may carry none, or a stale one.
- **Advisory** checks cover what depends on the environment, and states the
  user should know of: an MQTT upload, the charge controller answering on
  MeterBus, WiFi association, the running image trusting the development
  key. They are shown on the confirm page and never gate it, so a broker or
  network outage can't roll back a security release. Later they become nodes
  of the diagnosis and status tree.
- Neither kind affects whether an update can begin: that needs a running
  image confirmed `Valid`.

### Upgrade paths

What an update has to be compatible with is on the flash, and none of it is
the signature's business:

- **Layout:** fixed; the layout record is compared at upload.
- **Board:** the board entry, compared at upload (decision 8).
- **Configuration:** each image carries the migrations from every schema it
  still supports to its own, and runs them in its trial boot from the
  alternate configuration. Schemas change rarely and migrations are small,
  so old ones stay. A failed migration is a failed required self-check:
  the image can't be confirmed and rolls back.

Each image also declares, in the compatibility entry of its image
information block, the oldest configuration schema it migrates from and its
own. The running firmware compares that with its own schema at upload and
refuses an image that couldn't migrate this device's configuration, with the
release to install first, instead of letting it fail its trial (decision
35).

Two independent limits decide how far one update reaches: the signing keys
(two rotations) and the oldest migration the new image carries. Either may
be the shorter; a device far behind steps through intermediate releases,
which the release notes name. A host test migrates a committed fixture of
every past schema to the current one.

### Configuration across a rollback

- **During the trial (1, and 2 during a trial).** The previous image's
  configuration is untouched by construction: an update erases only the
  updated slot's configuration, and the trial image writes only its own
  slot's, through the migration and through user writes, which are allowed
  during a trial (decision 32). The previous image resumes with its
  configuration as it was when the update began; a rollback discards the
  changes made during the trial. A configuration change that locks the user
  out during a trial is undone by the rollback.
- **Switching after confirmation (2).** The target slot's configuration is as
  it was when the update began, and lacks every change made since. Its trial
  boot would migrate from the alternate configuration, which the newer image
  wrote in a newer schema. Recommended (decision 27): when the target's own
  configuration is present, keep it and ignore the alternate one; when it is
  empty, migrate from the alternate one if its schema is one the image
  knows, and otherwise start from defaults. The API says before the switch
  that changes made since the update aren't carried back.

### USB-flashed slots

- An image flashed over USB is installed under the rule above and switchable
  like any other; it needs no signature.
- `flash-app` still writes the build's signature sector, so the API reports
  who signed it. A flasher that doesn't (`cargo run`) leaves the previous
  image's signature file, which the API reports as `stale`, not `invalid`.

## The check-to-use window

### Slot layout

- The last sector of each app slot (offset `0x2FF000`, 4 KiB) is the slot's
  signature sector, holding the signature file padded with `0xFF`. An image
  must end before it. This changes no partition; the bootloader ignores bytes
  past the image.
- The signature sector belongs to the image: `begin_update` erases it with the
  first sector, and the slot's content module reads it at the same epoch as
  the image.
- A `signature` view of each slot publishes the file once it verified
  (decision 3). This changes a publication rule: today nothing past an
  image is published (`app.rs`). The signature file is public data, its
  bytes are pinned down by its parser, and nothing secret is ever written
  there; the spec asks for the change here.

### Byte path

1. `PUT /firmware/update/signature`: the body (at most 4096 bytes) is held in
   RAM by the update task and parsed and verified at once, so a wrong key or
   format is refused before anything is erased. This early verdict decides
   nothing later.
2. `PUT /firmware/update/image`, refused before anything is erased unless a
   signature file is held: `Ota::begin_update` settles the otadata on
   the running slot, erases the alternate slot's first sector and signature
   sector, and wipes the alternate configuration. `Update::write` programs the
   body as it arrives, each sector erased before its first bytes, refusing
   bytes past the signature sector.
   `Update` holds the image's first word in RAM and leaves the slot's first
   word erased.
3. `Update::finish` programs the last word and the held signature file into
   the signature sector, and ends the write session at epoch E. The slot
   now holds the whole image but its first word, so it doesn't load.
4. **First verification.** The slot is read at E through a snapshot, with
   the held first word laid over its first 4 bytes: header, segments,
   checksum, appended SHA-256, the file SHA-256 (accelerator), the
   descriptor, the layout record and the image information block; then the
   signature sector, parsed and verified under the running trust set; then
   the matching manifest line's digest compared with the file SHA-256. Every
   byte verified is read back from the flash at E, except those 4, which are
   the exact bytes programmed next. Nothing else from the HTTP body is
   trusted. The slot's own check, which also sees epoch E, finds no image
   there; the update's result isn't kept as the slot's verdict.
5. **Policy.** On that result `finish` checks the length written, the
   project, the layout, the board and configuration compatibility, the
   downgrade floor, that the signing key is in the new image's trust entry,
   that a release-signed image doesn't trust the development key, and that
   the signing key's kind matches `secure_version` (0 for the development
   key, at least 1 for a release key).
6. **Commit.** `finish` programs the held first word in a session of its
   own, which ends at epoch E'. Only from here can the image load.
7. **Second verification.** `finish` waits for the slot's own check at E',
   which runs on every new epoch anyway, with nothing laid over: the image
   must verify, and its file SHA-256 and verified signature line must equal
   the first verification's. Then it returns a `Signed` image at E'.
8. `Signed::activate` checks the slot is still at E' and writes the otadata
   entry that selects the slot with state `New`. It doesn't hash the slot a
   third time (decision 5).
9. At the next boot the bootloader checks the image's SHA-256 (no debugger
   attached) and starts it as `PendingVerify`; the confirmation window
   applies as now.

The signature is checked against the signature file and the file SHA-256 as
read from the flash, at the epoch the activation also requires. A refusal at
step 4, 5 or 7, or an upload abandoned before `finish` (a dropped
connection, a client timeout), leaves the slot's first word erased, so
nothing in the slot loads. The first sector is then erased so the API shows
the slot empty (decision 15); the request handler does it, as `Update`'s
`Drop` can't wait for an erase. A power cut between steps 6 and 7 leaves an
image that loads and passed the first verification but isn't installed.

The update's verification over the overlaid snapshot is a new entry point of
the slot's content module, beside its check: same code, a source that
substitutes the first word, and a result returned to the caller instead of
kept for the epoch.

### Types

A new crate, `eveningstar-signify` (`no_std`, no allocation, host-buildable,
`#![forbid(unsafe_code)]`), owns the format and the verification:

```rust
/// A trusted key: purpose, keynum, Ed25519 key. No public constructor.
pub struct TrustedKey { /* private */ }

/// The keys of one purpose. Only the generated constants exist.
pub struct TrustSet { /* private */ }

/// A signature file whose signature verified under one key of a trust set.
/// Only `verify` makes one; not `Clone`, `Copy` or `Default`.
pub struct Verified<'s> { /* key index, message borrowed from the file */ }

impl<'s> Verified<'s> {
    /// The manifest line whose digest is `sha256`, when exactly one is.
    pub fn line(&self, sha256: &[u8; 32]) -> Result<Line<'s>, ManifestError>;
}

pub fn verify<'s>(file: &'s [u8], trust: &TrustSet) -> Result<Verified<'s>, SigError>;
```

`eveningstar-boot`'s app check calls it at the slot's epoch and keeps the
result in the model (`Signature::Verified { key, line }`, `Unsigned`,
`Invalid(reason)`), so both slots report their signature. It keeps the
matching line's index, not its name: the model is copied in critical
sections, and the name is read from the slot on request, as the descriptor
is.

`eveningstar-ota` replaces `VerifiedImage` with `Signed`:

```rust
/// An image in the alternate slot whose signature verified under the
/// running trust set, twice: before its first word was programmed and after,
/// at the slot's epoch E'. It holds the alternate slot's
/// writer, ended at E, and the otadata writer, both borrowed from the `Ota`
/// for its whole life, so nothing writes the slot or the otadata until it
/// is activated or dropped.
#[must_use]
pub struct Signed<'o, 'f> {
    slot: Ended<'o, AlternateApp>, // &'o mut Writer, stable at E'
    otadata: &'o mut Writer<'f, Otadata>,
    // module, digest, key, versions: private
}

impl Signed<'_, '_> {
    /// Selects the image with state `New`, so it boots as a trial.
    pub async fn activate(self) -> Result<(), OtaError>;
}

impl Ota<'_> {
    pub async fn begin_update(&mut self) -> Result<Update<'_, '_>, OtaError>;
    /// The image installed in the other slot: an otadata entry names it, it
    /// passes the image, project, layout and board checks, and once the
    /// running image is confirmed its `secure_version` isn't lower.
    pub async fn installed_other(&mut self) -> Result<Installed<'_, '_>, OtaError>;
}

/// An image installed in the other slot, at the slot's epoch E. Built only
/// from the otadata and the slot's check; holds the same writers as
/// `Signed`.
#[must_use]
pub struct Installed<'o, 'f> { /* private */ }

impl Installed<'_, '_> {
    /// During a trial, marks the running entry `Invalid`; after
    /// confirmation, selects the other slot with state `New`.
    pub async fn switch(self) -> Result<(), OtaError>;
}

impl Update<'_, '_> {
    pub async fn finish(self) -> Result<Signed<'_, '_>, OtaError>;
}
```

- `Signed` is built only in the module that reads `Signature::Verified` from a
  model the slot's check made at E. Its fields are private and it implements
  neither `Clone` nor `Copy`.
- `Ota::activate`, `Ota::roll_back`, `Ota::verify_other` and `VerifiedImage`
  go away. `Signed` and `Installed` are the only values that select a slot:
  `plan_activate` is called only from `Signed::activate`, and the switch's
  plans (`plan_roll_back` during a trial, a `New` entry after confirmation)
  only from `Installed::switch`.
- `eveningstar-storage` gains `Session::end(self) -> Result<Ended<'w, R>, _>`,
  which keeps the session's `&mut Writer` and the epoch: write access without
  an open session, so the slot can be read again.
- Because `Update` and `Signed` borrow the `Ota` mutably, the upload, its
  verification and its activation run in one request handler (decision 14);
  no token outlives the request.

## API and web UI (sketch)

`GET /api/v1/firmware`, extending the sketch in `specs/storage-api.md`:

```json
{
  "trust": {
    "purpose": "fw",
    "dev_key_trusted": true,
    "keys": [
      { "name": "eveningstar-dev-fw", "generation": null, "status": "dev",
        "keynum": "c0ffee0123456789", "public_key": "RWTA/+4BI0VniQ..." },
      { "name": "eveningstar-2-fw", "generation": 2, "status": "signing",
        "keynum": "...", "public_key": "..." },
      { "name": "eveningstar-3-fw", "generation": 3, "status": "future",
        "keynum": "...", "public_key": "..." }
    ]
  },
  "slots": {
    "current": { "signature": { "status": "verified", "key": "eveningstar-dev-fw",
                                "name": "eveningstar-1.4.0-esp32c6.bin" },
                 "secure_version": 1, "trusts_dev_key": true },
    "alternate": { "signature": { "status": "unsigned", "key": null, "name": null },
                   "secure_version": 0, "trusts_dev_key": true,
                   "installed": true, "switch": { "allowed": true, "refusal": null } }
  },
  "update": { "state": "idle" }
}
```

- `public_key` is the key file's second line; `keynum` is hex. Names come
  from the generated trust set, never from a comment.
- `signature.status`: `verified`, `unsigned` (no signature sector),
  `stale` (a signature that verifies under a trusted key but lists no digest
  of this image, as a USB flash leaves behind), `unknown_key`, `invalid`
  (with a reason: format, signature), `checking`.
- `update.state`: `idle`, `signature_held`, `receiving` (bytes written),
  `verifying`, `refused` (reason), `activated` (next boot).
- `PUT /firmware/update/signature`, `PUT /firmware/update/image` (upload,
  verify, activate), `POST /firmware/switch`, `POST /firmware/confirm`;
  answers `422` with the reason for a refused file, `409` for a refused
  switch.
- Each slot also shows its image information block (board, configuration
  schema range, trust entry) and `switch.refusal`: `not_installed`,
  `corrupted`, `no_appended_hash` (an image flashed over USB without one,
  which the bootloader boots but the image check refuses),
  `lower_secure_version`, `running_unconfirmed` (a fallback boot, where
  the running entry isn't the active one and no switch can be planned),
  `checking`. A switch during a trial whose entry a torn write left
  undefined is the same write as the deadline rollback, and is allowed.
- `confirmation` in `GET /firmware`: whether the running image awaits it,
  the rollback deadline, the latest deadline an extension may reach (boot
  plus 24 hours), the required and the advisory self-checks with their
  results, and whether confirming raises the floor above the other slot's
  image. `POST /firmware/confirm/extend` moves the deadline.
- Each slot's `secure_version` and whether a switch to it would be refused.
- Each slot's file SHA-256, as `sha256sum` prints it, so the owner can
  compare the image the device runs with the release's published `SHA256`
  (requirement 7).
- Security release warning, at upload once the image verified and on the
  confirm control, when the image raises `secure_version` above the other
  slot's: "This release fixes a security vulnerability. Once it is
  confirmed, firmware with a lower security version can't be started
  through updates or slot switches. Flashing over USB restores any
  firmware." The confirm control adds the version it rules out: "Version
  1.3.2 can't be started again after confirmation, except by flashing over
  USB."
- Web UI: on every page, while the running image trusts the development key,
  a banner: "This firmware trusts the development signing key. Anyone can
  sign firmware that this device installs." On the update page the same
  warning for an image to be installed that trusts it, and each slot's
  signature status. Copy follows `ui-design-rules`.

## Verification plan

Every check below runs from Nix (`nix run .#checks`) or is recorded with its
command, so its evidence doesn't depend on the authors.

### The model check

- An exhaustive state-space check of the update protocol, run in the `test`
  group: the otadata planner's real code, `Ota`'s operations, the
  bootloader's selection, abort and fallback rules written from
  `bootloader_utility.c`, power cuts at every program and erase, and the
  flash's undefined states: an interrupted program or erase leaves each
  affected bit 0, 1, or unstable, read differently at each read. Built on
  the existing host test flash, with a small explicit-state checker (or
  `stateright`), so the code checked is the code shipped.
- Properties: requirements 1 and 2 (no unverified image loadable; every
  trial ends in a confirmation or the previous image; the installed
  invariant), and that every reachable state boots some image.
- The bootloader's rules in the model are a transcription. The real
  bootloader decides the same cases on the board: each torn otadata state
  the model reaches is written with `esptool` and booted, and the cases of
  ESP-IDF's own `test_switch_ota.c` run as board scenarios.
- The unstable-bit model is bounded: one interrupted operation outstanding,
  its bits resolved at their first read after it.

### Mutation testing

- `cargo-mutants` over the verifier, the policy checks, the otadata planner
  and the `Ota` operations. Every surviving mutant is either killed by a new
  test or recorded with the reason it can't change behaviour. CI fails on a
  new survivor.

### Differential tests against signify

- A Nix derivation builds a corpus with nixpkgs `signify` 32: keys from
  `signify -G -n`, manifests from `sha256sum --tag`, signatures from
  `signify -S -e`, and the OpenBSD 8.0 `SHA256.sig` with
  `openbsd-80-base.pub` as a foreign real-world case.
- Mutations of each case: every byte of the signature file replaced by each of
  a fixed set of values; every bit of R, S and the keynum flipped; S + L and
  S + 2L; comment variants (empty, 1023 and 1024 bytes, CR, NUL); base64
  variants (whitespace inside, missing or extra padding, non-zero padding
  bits, CRLF, no final newline, trailing lines); manifest variants (each hex
  digit changed, uppercase, base64 form, `SHA512` lines, trailing text after
  the hash, extra spaces, duplicate lines, unlisted file, empty list); wrong
  keynum; small-order, mixed-order and non-canonical keys from Wycheproof
  injected as `.pub` files.
- Each case runs the device verifier built for the host (an example of
  `eveningstar-signify`) and signify, and records both verdicts. Signature
  cases (Wycheproof and RFC 8032 messages, which aren't checksum lists) run
  `signify -V -e -p key.pub -x <sig> -m <out>`, so signify's verdict is the
  signature's alone; manifest cases run `signify -C -p key.pub -x SHA256.sig
  <file>`.
- The rule: the device never accepts what signify refuses. Where signify
  accepts and the device refuses, the device's reason must be one of an
  enumerated list (S >= L; small-order R; a small-order or mixed-order key,
  which the trust set generator refuses; a base64, comment or manifest form
  outside the device's subset; a file over 4 KiB). Any other disagreement
  fails the check.

### RFC 8032 and Wycheproof

- RFC 8032 7.1's Ed25519 vectors verify; each with one bit flipped in the
  message, R, S or A doesn't.
- Every Wycheproof `ed25519_test.json` case: `valid` accepted, `invalid`
  refused. `verify_strict` refuses small-order keys but not mixed-order ones,
  so each `acceptable` case's verdict is recorded on the first run, reviewed,
  and fixed in the test. Neither kind can enter a trust set.
- The vectors are vendored with their upstream commit and SHA-256.

### Byte flips

- On the host, over the `eveningstar-ota` test flash: every byte of a small
  signed image, of the signature sector and of the manifest flipped in turn
  after `finish` programs them, and between `finish` and `activate`; each must
  end in a refusal and no otadata write.
- The held first word: until the first verification passes, the slot's first
  word reads `0xFFFFFFFF` after every write, refusal, abandonment and power
  cut; a first word flipped between steps 6 and 7 fails the second
  verification.

### The installed invariant

- A host property test over the otadata planner and `Ota` on the test flash:
  for any sequence of updates, activations, confirmations, switches, boots
  (the bootloader's selection modelled from `bootloader_utility.c`, its own
  otadata writes torn as decision 39 describes) and power cuts at any
  program or erase, no entry of any state names a slot from the
  first erase of an update until the activation of an image that verified,
  and `installed_other` never accepts that slot in between.

### Fuzzing

- `cargo-fuzz` targets for the signature file parser, the manifest parser,
  the `.pub` parser used by the generator, the image information block
  parser, and `verify` end to end. Property: no panic; an accepted input's
  message verifies with `verify_strict` and has exactly one matching line.
- The end-to-end target also links OpenBSD's own verification code
  (signify's parsing and SUPERCOP `ref`, built with `cc` in a test-only
  crate from the vendored C sources) and applies the rule of the
  differential tests to every input.
- CI runs each target briefly from a committed corpus; long runs are manual
  and their corpus additions committed (decision 21).

### The secret key scan

- Fixtures the scan must refuse: a release secret key under its own name,
  renamed to `.txt`, pasted into another file, and copied over
  `keys/eveningstar-dev-fw.sec`; and a development secret key with a
  passphrase. Fixture it must pass: the committed development key. The
  fixtures are generated in the Nix check, never committed.

### compile_fail tests

One doctest per bypass, each pinned to its error code (`compile_fail,E0599`
and so on), so a doctest that fails for another reason doesn't pass:

- building `Signed`, `Verified`, `TrustedKey` or `TrustSet` outside its crate
  (private fields, no constructor);
- activating a `Signed` twice (moved);
- cloning or copying a `Signed`;
- building an `Installed` outside its crate, or using one twice;
- calling `begin_update`, `installed_other` or `confirm` while a `Signed` or
  an `Installed` lives;
- keeping a `Signed` across a later `split` (as the existing `VerifiedImage`
  test);
- calling an activation on `Ota` without a `Signed`, or a switch without an
  `Installed`, neither of which exists;
- opening a session on the alternate slot's writer while a `Signed` holds it.

### The power-cut rig and broken images

- The board's DC input switched by the host (a programmable supply, or a
  relay a second board drives), cut at random offsets during upload, finish,
  activation, the bootloader's otadata writes, confirmation and switches,
  for thousands of cycles. A cut is a ramp into the brownout reset through
  the board's capacitors, not a stop; the rig also tests a slow ramp. After
  each cut the board must come up running an image the rules allow, and
  reachable over the network unless the case says otherwise.
- A catalogue of broken test images, each a build flag: panics at boot,
  hangs before the supervisor starts, no network, a panic after 30 minutes,
  a panic only after confirmation, a failing required self-check, a broken
  configuration migration. Each must end as requirement 2 says.

### Cross-checks with Espressif's tools

- ESP-IDF's `otatool.py` reads the otadata after every write in the board
  scenarios and must agree with the firmware's report; `esptool image-info`
  must accept every image the firmware accepts.

### Supply chain and reproducibility

- `cargo-vet`, with the published audits of other projects imported, and
  local audits for what they don't cover, at least for ed25519-dalek,
  curve25519-dalek, sha2 and base64; `cargo-deny` for advisories, sources and
  licences. An upgrade of a crypto crate is reviewed as a diff.
- The `build` group rebuilds the release flavour with `nix build --rebuild`
  and fails on any byte of difference. Every release is also rebuilt on a
  second machine, by someone other than CI, before it is signed; the
  release notes say who rebuilt it and how anyone repeats it.
- Before the first signed release, an external security review of this spec
  and the implementation.

### On the board

- With a scenario script over the API (as the storage scenario tests): a
  correctly signed image activates and boots; refused, with nothing selected
  and the first sector erased: one flipped byte in the image, in the
  signature, in the manifest; the right file signed by an untrusted key; a
  development-signed image to a release build; a lower `secure_version`;
  another layout; another project; an image too large; a truncated upload; a
  signature file over 4 KiB.
- After each refusal above, and after an upload cut by closing the
  connection or by a power cycle, `esptool read-flash` of the slot's first
  4 bytes gives `ff ff ff ff`.
- Refused at upload, likewise: an image whose trust entry lacks the key that
  signed it; a release-signed image whose trust entry holds the development
  key; an image whose oldest migratable schema is newer than the device's.
- Rollback scenarios, with a test build whose network stack is switched off:
  an update to it with a new key generation and a raised `secure_version`
  must roll back by itself (the 10-minute reboot; an extended deadline; the
  supervisor with the confirmation task not spawned; a power cut during the
  trial), and the previous image must resume with its configuration byte
  for byte, configuration writes made during the trial included. An
  extension past 24 hours after boot must be refused.
- Switch scenarios: after confirming an image with a new generation and the
  same `secure_version`, a switch to the older image (and to a USB-flashed
  one without a signature) must boot it as a trial and leave both installed;
  with a raised `secure_version`, the switch must be refused after
  confirmation and succeed during the trial;
  a switch must be refused after an upload that failed verification, after a
  power cut during an upload, after an automatic rollback (the slot is
  `ABORTED`), to a slot whose image was corrupted, and, after a release is
  confirmed, to a development build in the other slot.
- A trial whose entry is left in an undefined state (written with esptool,
  as a torn bootloader write would leave it): the confirmation window's
  rollback must return to the previous image at the deadline.

## Decisions

The numbers stay as first proposed, so earlier references hold; 38 isn't
used.

### Decided with the user (2026-11-07 and 2026-11-08)

7. **Downgrade floor.** The ESP descriptor's `secure_version`, derived by
   the build from the release flavour ("Development and release keys"), the
   full `u32` (no eFuse). Lower than the running image's is
   refused for updates, and for switches once the running image is
   confirmed; nothing is refused during a trial; equal is allowed whatever
   the version string. Warnings at upload and at confirmation, which say
   that USB always recovers.
9. **The image's own records.** One image information block after the
   layout record: magic and length, then type-length-value entries; unknown
   types skipped, values only grow, meanings never change.
10. **Trust sets per build.** Development builds trust the development key
    and N to N+2, so a board flashed with one by mistake updates over the air
    to a release; release builds trust N to N+2 only.
11. **Rotation.** On a schedule whatever happens, and at once when the
    signing generation is compromised or lost; a compromised or lost future
    generation is skipped. Images trust the signing generation and the next
    two usable ones. Recommended schedule: once a year, with a release.
12. **Switching across a generation.** An installed image is switchable
    whatever key signed it; rotation decides only what can be installed.
13. **USB-flashed slots.** Installed under the same rule and switchable;
    `flash-app` writes the signature sector for reporting only.
15. **Unverified images never load.** The image's first word stays in RAM
    through the first verification and is programmed only once the image
    verified, then the image is verified again from the flash. Recommended:
    erase the slot's first sector after a refusal or an abandoned upload, for
    the API's sake only.
23. **Trust what is installed.** A switch, in either direction, checks the
    target's integrity, never its signature (shown), and after confirmation
    its `secure_version`. "Installed" is an otadata entry naming the slot,
    which only a verified activation writes for a slot an update touched. A
    switch after confirmation boots as a trial. Today `roll_back` works
    after confirmation too but selects the old image as `Valid` with no
    trial, and nothing switches back.
24. **Trial deadline.** No new watchdog: the supervisor, with the
    confirmation window's handle registered before the executor starts, and
    a test that a confirmation task that never runs resets the board.
28. **Reading of "no code path may skip cryptographic verification".** It
    covers every byte that arrives remotely. Switching between installed
    images is outside it, as OpenBSD's `syspatch -r` is.
29. **Permission for a switch.** The highest tier, once authentication
    exists: it rewrites the otadata and brings back the other slot's
    configuration, credentials included. No physical-presence override:
    physical access means USB.
30. **Self-checks.** Required checks gate confirmation, advisory checks are
    shown. Recommended required set: configuration migration, boot tier and
    OTA readiness, the API answering, the supervisor running, the verifier's
    known answers.
31. **Extending the trial.** Up to 24 hours after boot, in RAM only.
    Recommended: steps of one hour, same permission as confirm.
32. **Configuration writes during a trial.** Allowed; they go to the trial
    image's slot and a rollback discards them. This changes the lifecycle
    agreed on 2026-11-05 (user writes only once `Valid`).
33. **Development builds below every release.** Development builds have
    `secure_version` 0 and releases at least 1, so the floor refuses
    switching to a development build once a release is confirmed. A
    compromised release key is handled the same way, by a fix release that
    raises `secure_version`. The device enforces it too: an upload signed
    with the development key must have `secure_version` 0, and one signed
    with a release key at least 1 (found in review: a development-signed
    image with a huge `secure_version` would otherwise lock a development
    board out of every later image once confirmed).
34. **Release-signed images never trust the development key.** Refused at
    upload by every firmware, refused by the trust set generator, and checked
    in the ceremony.
35. **Upgrade compatibility.** Separate from signing: layout and board
    entries, and a compatibility entry (oldest migratable schema, own
    schema) checked at upload; a failed migration in the trial is a failed
    required self-check. Old migrations stay while they are small.
36. **Development key as a self-check.** Advisory.
37. **Secret key checks.** The content scan in `lint`, pre-push and over
    every pull request commit, the `.gitignore` rule, and passphrase-protected
    release keys.

40. **eFuses.** Nothing is write-protected in advance, so the owner stays
    free to install a fork that uses secure boot. Recommended guards: the
    board crate keeping the peripheral and the USB pins, lints, the link
    map's cross-reference check of the ROM's programming functions, and
    `release-check`'s allowlisted constant scan.
44. **Reproducible releases.** Every signed release is reproducible from
    its tag (user, 2026-11-08): `release-check` and the ceremony sign only
    an image that a second, independent build and CI's rebuild both match
    byte for byte, and the first signed release waits until they do.

### Open, with a recommendation

1. **Signed format.** Recommended: the signed manifest (`signify -C`). Not
   the whole image (streaming `hazmat` verification or 3 MiB of RAM) nor
   `-z` (gzip and an inflater on the device).
2. **How the manifest names the image.** Recommended: match by digest alone,
   exactly one line, and report its name. The alternative binds a canonical
   name (`eveningstar-<version>-<chip>.bin`) built from the image, which adds
   rules without adding a check the image's own records don't make.
3. **Where the signature lives.** Recommended: the last sector of each slot,
   published as a `signature` view. The alternative is a new region per slot,
   a layout change.
4. **The primitive.** Recommended: ed25519-dalek 3.0.0 `verify_strict`,
   default features off, no `hazmat`, no `legacy_compatibility`; `sha2` 0.11
   for its SHA-512. The audit gap above is the cost; the differential tests
   and vectors are the mitigation.
5. **Re-verify at activation.** Recommended: no, as now: `Signed` holds the
   slot's writer and activation checks the epoch, which excludes every
   firmware write; a write from outside is physical access, out of scope.
   Hashing again would cost another full slot check per update and a new
   entry point in the slot's module, which keeps one result per epoch.
6. **Disagreement with signify.** Recommended: the device may be stricter
   than signify only in the enumerated classes. Exact agreement would need a
   port of SUPERCOP's malleable verifier.
8. **Board entry.** Recommended: a board entry in the image information
   block (board name and the revisions the image supports), checked like the
   layout. Alternative: rely on the project name.
14. **One request or two for activation.** Recommended: the image upload
    verifies and activates; rebooting stays a separate call. Alternative: a
    separate activate call, which needs the OTA task to keep the `Signed`
    between requests.
16. **Bootloader configuration.** Recommended: pin
    `BOOTLOADER_SKIP_VALIDATE_ON_POWER_ON`, `_ALWAYS` and `_IN_DEEP_SLEEP`
    off (the deep sleep fast path boots without selecting, so a trial isn't
    ended), and `BOOTLOADER_APP_ANTI_ROLLBACK` off (it burns eFuses and can
    leave nothing to boot), and `BOOTLOADER_WDT_ENABLE` on with
    `BOOTLOADER_WDT_DISABLE_IN_USER_CODE` off (the RTC watchdog covers the
    start-up until esp-hal takes over), and `BOOTLOADER_FACTORY_RESET` and
    `BOOTLOADER_CUSTOM_RESERVE_RTC` off (with `_IN_DEEP_SLEEP`, they are what
    selects `BOOTLOADER_RESERVE_RTC_MEM`, whose retained struct sits at the
    top of LP RAM, where `specs/diag.md` puts its mailbox), and
    `BOOTLOADER_FLASH_XMC_SUPPORT` on (this board's flash), in
    `nix/bootloader`, reserved like `SECURE_*` and checked by
    `_check-sdkconfig.py`, which also fails when
    `BOOTLOADER_RESERVE_RTC_MEM` is set. All are at these values today by
    default.
17. **The image digest.** Recommended: the SHA accelerator, as now for the
    slot check and as the bootloader uses it; the board test compares it with
    `sha256sum`. Alternative: software `sha2` as well, for two independent
    computations.
18. **The untrusted comment.** Recommended: ignored, never shown.
19. **Key names.** Recommended: `eveningstar-<generation>-fw` and
    `eveningstar-dev-fw`.
20. **Ceremony.** Recommended as written: offline machine, passphrase, two
    offline copies, N to N+2 generated at first and one more per rotation,
    reproducible build compared before signing.
21. **Fuzzing toolchain.** Recommended: `cargo-fuzz` with a pinned nightly in
    the dev shell, short CI runs, committed corpora.
22. **The probe-rs tamper test.** Dropped: physical access is out of scope,
    and the corrupted-slot scenario already shows the bootloader's check.
25. **OTA readiness gates confirmation.** Recommended: a required self-check
    that the boot tier is reached and OTA is ready.
26. **An image must trust its signer.** Recommended: refuse an update whose
    trust entry lacks the key that signed it. No further check of the entry's
    `signing` generation: it adds nothing, and a development build, signed
    with the development key, names 0.
27. **Configuration on a switch after confirmation.** Recommended: keep the
    target's own configuration when present; migrate from the alternate one
    only when the target's is empty and the schema is known; otherwise
    defaults. The API says so before the switch.
39. **The firmware's rollback at the trial deadline.** Recommended: mark the
    running entry `Invalid` before the deadline's reboot when there is a
    rollback target, so a trial ends even when a torn bootloader write left
    its entry undefined. The host model of the bootloader tears its writes
    (any subset of the state word's bits, with the sequence number and CRC
    whole), and the installed invariant's property test runs with it.

41. **The model checker.** Recommended: a small explicit-state checker over
    the real host code (or `stateright`), not TLA+, so the checked model is
    the shipped code.
42. **External review.** Recommended: before the first signed release.
45. **Known-answer self-check.** Recommended: built-in positive and negative
    vectors required before confirmation, so a release whose own verifier is
    broken rolls back instead of being confirmed into a state where it can't
    take the fix; the image's own signature is advisory.

## Left open

- Whether builds on different host architectures (x86_64 and aarch64
  Linux) give the same bytes; until shown, the release notes name the host
  platform the release reproduces on.
- The stack frame of `verify_strict` on the 32-bit backend under the
  `large_stack_frames` lint, and its time on the ESP32-C6.
- Code size of ed25519-dalek and curve25519-dalek in the image.
- Whether the web UI's update page takes both files in one drop.
