# Releasing firmware

How a firmware release is built, checked, signed and published. A release is
three files: the app image, its ELF (for symbolized logs), and `SHA256.sig`,
a signify signature over their SHA-256 digests. Devices install only images
signed by a release key their firmware trusts, so a mistake here reaches
every device that installs the release.

> [!IMPORTANT]
> The signed update path is designed in `specs/ota-signing.md` and isn't
> built yet. Commands marked _(planned)_ don't exist yet; this page is
> updated as they land. Until then, releases are flashed over USB only.

## Rules

- Release secret keys never touch a networked machine, and are never copied
  into the repository, a chat, a ticket or a backup service.
- Only an image built by Nix from a signed tag is signed, and only after two
  independent builds give the same digest.
- `secure_version` never goes down, and is at least 1 for a release.
  Development builds have `secure_version` 0.
- A release never trusts the development key.
- `signify -C` is always given the file names to check: with none, a list
  that doesn't name the image still verifies.

## What the tooling refuses

These checks stop the mistakes they name; each runs without being asked.

- The trust set generator (`lint` group) refuses a release trust set that
  holds the development key, a compromised generation, a gap in the
  generations, or a missing key file.
- The secret key scan (`lint` group, pre-push, and every commit of a pull
  request) refuses any signify secret key in the repository other than the
  development key.
- `release-check` _(planned)_ refuses an image that trusts the development
  key, has `secure_version` 0 or lower than the previous release's, trusts
  generations other than `signing` to `signing + 2`, names a different
  `signing` generation than `nix/keys.nix`, refers to the eFuse controller
  outside the committed allowlist of esp-hal's readers, or fails any
  `check-images` check (layout, size, chip, project).
- Every firmware refuses at upload an image signed with a release key that
  trusts the development key, an image whose trust set lacks the key that
  signed it, an image with a lower `secure_version`, an image signed with
  the development key whose `secure_version` isn't 0 or one signed with a
  release key whose `secure_version` is 0, and an image that can't migrate
  the device's configuration.

They don't catch a wrong decision: a security fix released without raising
`secure_version`, a `secure_version` raised by mistake (devices can't switch
back to the previous release over the network once they confirm), or release
notes that omit a stepping-stone release.

## Before the release

1. Decide the version (`version` in the root `Cargo.toml`).
2. Decide `secure_version` (`secureVersion` in `nix/release.nix`
   _(planned)_; only the release build reads it, and development builds
   always have 0):
   - raise it by one when the release fixes a vulnerability that makes
     earlier firmware unsafe to run;
   - otherwise keep it.

   Raising it means devices that confirm the release can't switch back to an
   earlier one except over USB, and the web UI says so at upload and at
   confirmation.
3. Check the configuration schema: when it changed, the new firmware carries
   the migration, the host migration tests cover a fixture of every past
   schema, and the compatibility entry names the oldest schema it migrates
   from.
4. Check the key generation: when a rotation is due (yearly, or now after a
   compromise or loss), rotate first ("Rotating keys").
5. Write the release notes:
   - the version, and whether it raises `secure_version` and why;
   - the signing generation and the public key lines of the generations it
     trusts;
   - the releases a device must install first: the oldest configuration
     schema it migrates from, and the oldest release whose firmware trusts
     its signing generation.
6. Merge to `main`, and tag the commit with a signed tag
   (`git tag -s v<version>`).

## Building

1. Build on CI from the tag, and locally from the same tag:

   ```sh
   nix build .#release   # (planned)
   ```

   `result/` holds `eveningstar-<version>-esp32c6.bin`,
   `eveningstar-<version>-esp32c6.elf` and `SHA256`
   (`SHA256 (<file>) = <hex>` lines).
2. Compare the two `SHA256` files byte for byte. They must be identical;
   different digests mean the build isn't reproducible, and nothing is
   signed. The descriptor's build date comes from `SOURCE_DATE_EPOCH`, which
   Nix sets, so it doesn't differ between the builds.
3. Run the checks and read the report:

   ```sh
   release-check result/   # (planned)
   ```

   It prints the version, `secure_version`, the signing generation, the
   trusted keys with their generations, the board, the configuration schema
   range, the layout check and the size. Compare them with the release
   notes.

## Signing

On the offline machine: a live system started from read-only media, with no
network interface up, built from the tag as `nix build .#signing-iso`
_(planned)_ so that it carries `signify`, `sha256sum` and `release-check`.

1. Copy `result/` to a USB stick and attach it to the offline machine.
2. Run `release-check` on it again, and check that the digests in `SHA256`
   match the files:

   ```sh
   release-check /media/release
   sha256sum --tag eveningstar-<version>-esp32c6.bin \
     eveningstar-<version>-esp32c6.elf | diff - SHA256
   ```

3. Attach the medium holding the secret key of the signing generation and
   sign:

   ```sh
   signify -S -e -s eveningstar-<N>-fw.sec -m SHA256 -x SHA256.sig
   ```

   signify asks for the key's passphrase. A key that signs without one is
   not a release key: stop.
4. Detach the key medium. Only `SHA256.sig` leaves the offline machine, on
   the USB stick.

## Publishing

1. On a networked machine, verify the signature with the public key from the
   repository:

   ```sh
   signify -C -p keys/eveningstar-<N>-fw.pub -x SHA256.sig \
     eveningstar-<version>-esp32c6.bin eveningstar-<version>-esp32c6.elf
   ```

   It prints `Signature Verified` and `OK` for each file.
2. Install the release over the air on a test board running the previous
   release ("Updating firmware", `docs/updating.md`): it must verify, boot
   as a trial, pass its self-checks and confirm. On a second board, let the
   trial expire and check that it rolls back.
3. Publish a GitHub release for the tag with the image, the ELF,
   `SHA256.sig` and the release notes.

## Rotating keys

Once a year with a release, and at once after a compromise or loss.

1. On the offline machine, generate the next generation's key, with a
   passphrase:

   ```sh
   signify -G -p eveningstar-<N+3>-fw.pub -s eveningstar-<N+3>-fw.sec
   ```

   Copy the secret key to both offline media. Copy only the `.pub` out.
2. Add `keys/eveningstar-<N+3>-fw.pub` and its entry in `nix/keys.nix`, and
   raise `signing` by one. Commit with a signed commit.
3. The next release is signed with the new `signing` generation and trusts
   it and the two after it.

## After a compromise

A release secret key, or the passphrase and a copy of it, has left the
offline media.

1. Mark its generation `compromised = true` in `nix/keys.nix`, and raise
   `signing` past it.
2. Release at once, signed with the next generation, with `secure_version`
   raised: devices that confirm it stop trusting the compromised key and
   can't switch back to a release that trusts it.
3. Say in the release notes which generation is compromised and since when.
   Devices that don't update keep trusting it.

A secret key committed to the repository counts as published, even if the
commit is removed.
