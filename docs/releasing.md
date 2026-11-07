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
  key, has `secure_version` 0 or lower than the previous release's (from
  the history in `nix/release.nix`), trusts a set of generations other than
  the one `nix/keys.nix` yields, names a different `signing` generation,
  refers to the eFuse controller outside the committed allowlist of
  esp-hal's readers or to the ROM's eFuse programming functions (the link
  map's cross-reference table), or fails any `check-images` check (layout,
  size, chip, project).
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
     earlier firmware unsafe to run, or drops a compromised generation;
   - otherwise keep it.

   `nix/release.nix` keeps each release's value, so the check knows the
   previous one.

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
     its signing generation;
   - who rebuilt it independently, once that rebuild has matched.
6. Merge to `main`, and tag the commit with a signed tag
   (`git tag -s v<version>`).

## Building

1. Build on CI from the tag, and locally from the same tag:

   ```sh
   nix build .#release   # (planned)
   ```

   `result/` holds `eveningstar-<version>-esp32c6.bin`,
   `eveningstar-<version>-esp32c6.elf`, the link map, and `SHA256`
   (`SHA256 (<file>) = <hex>` lines).
2. Compare the two `SHA256` files byte for byte. They must be identical;
   different digests mean the build isn't reproducible, and nothing is
   signed (`diffoscope` shows where they differ). The descriptor's build
   date comes from `SOURCE_DATE_EPOCH`, which Nix sets, so it doesn't differ
   between the builds. Write down the image's digest as CI's log shows it,
   read from CI's page, not from a file the local machine produced.
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
2. Run `release-check` on it again, giving it the image digest written
   down from CI's page, so a networked machine that swapped both the image
   and the listing is caught:

   ```sh
   release-check /media/release --expect <digest from CI>
   ```

   It also checks that the digests in `SHA256` match the files.

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
3. Publish a GitHub release for the tag with the image, the ELF, `SHA256`,
   `SHA256.sig` and the release notes.

## Rotating keys

Once a year with a release, and at once after a compromise or loss.

1. On the offline machine, generate the next generation's key, with a
   passphrase:

   ```sh
   signify -G -p eveningstar-<N+3>-fw.pub -s eveningstar-<N+3>-fw.sec
   ```

   Copy the secret key to both offline media. Copy only the `.pub` out.
2. Add `keys/eveningstar-<N+3>-fw.pub` and its entry in `nix/keys.nix`,
   raise `signing` by one, and regenerate the README's key table. Commit
   with a signed commit.
3. The next release is signed with the new `signing` generation and trusts
   it and the two after it.

## After a compromise or a loss

A release secret key, or the passphrase and a copy of it, has left the
offline media (compromised), or both copies of a key are gone (lost).

1. Mark its generation `compromised = true` or `lost = true` in
   `nix/keys.nix`. The trust set becomes the next three generations from
   `signing` that are neither; generate a further key if needed
   ("Rotating keys", step 1).
2. Raise `signing` only if the marked generation was the signing one. A
   future generation is skipped instead: installed images already trust
   the current one, which keeps signing.
3. Release at once, signed with the current usable generation. After a
   compromise, raise `secure_version`: devices that confirm the release
   stop trusting the compromised key and can't switch back to a release
   that trusts it.
4. Say in the release notes which generation is compromised or lost, and
   since when. Devices that don't update keep trusting a compromised key.

A secret key committed to the repository counts as published, even if the
commit is removed.
