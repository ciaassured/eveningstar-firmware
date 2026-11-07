# Updating firmware

How to install a firmware release on a device, over the network or over USB,
and what the device does with it.

> [!IMPORTANT]
> The signed update path is designed in `specs/ota-signing.md` and isn't
> built yet. Steps marked _(planned)_ don't exist yet; this page is updated as
> they land. Until then, firmware is installed over USB only.

## The files of a release

Each release on GitHub has:

- `eveningstar-<version>-esp32c6.bin`: the app image;
- `eveningstar-<version>-esp32c6.elf`: the same firmware with symbols, for
  logs;
- `SHA256`: both files' SHA-256 digests;
- `SHA256.sig`: the signature over that list, with the list embedded.

The device checks the signature itself. Checking it on a computer as well
confirms the download before anything is sent to the device:

```sh
signify -C -p eveningstar-<N>-fw.pub -x SHA256.sig eveningstar-<version>-esp32c6.bin
```

`eveningstar-<N>-fw.pub` is in the repository's `keys/` directory and in the
release notes. On Debian and Ubuntu the program is `signify-openbsd`.

## Over the network _(planned)_

### Upload

On the device's firmware page, choose the image and `SHA256.sig`, and
upload. Or with `curl`:

```sh
curl -T SHA256.sig http://<device>/api/v1/firmware/update/signature
curl -T eveningstar-<version>-esp32c6.bin http://<device>/api/v1/firmware/update/image
```

The device writes the image to the slot that isn't running, reads it back,
and checks it before selecting it for the next boot:

- the signature, by a key the running firmware trusts;
- the image's integrity, chip, project, board and partition layout;
- its security version, which may not be lower than the running firmware's;
- that it can migrate the device's configuration;
- that it trusts the key that signed it.

A refused upload changes nothing that boots, and the page names the reason.

The firmware page shows each slot's SHA-256. After an update it matches the
image's line in the release's `SHA256` file, which is also the message
`SHA256.sig` signs:

```sh
sha256sum eveningstar-<version>-esp32c6.bin
```

The page warns before installing:

- a release that raises the security version: once confirmed, firmware with
  a lower security version can't be started through updates or slot
  switches; flashing over USB still restores any firmware;
- an image that trusts the development signing key.

### Trial and confirmation

1. Reboot the device from the firmware page. The new firmware starts on
   trial.
2. The firmware page lists the self-checks:
   - required checks (for example configuration migration, storage, OTA
     readiness, the API and the signature verifier's known answers) must
     pass before the firmware can be confirmed;
   - advisory checks (for example MQTT uploads, the charge controller, the
     network, whether it trusts the development key) are shown for the user
     to judge.
3. Check what matters for the installation. The trial lasts 10 minutes and
   can be extended in steps, up to 24 hours after the reboot.
   Configuration can be changed during the trial; the changes are discarded
   if the trial ends in a rollback.
4. Confirm. From then on the new firmware stays.

When the trial ends without a confirmation, or the device restarts or loses
power during it, the device returns to the firmware it ran before, with that
firmware's configuration as it was before the update. The firmware page then
says that the update was rolled back, and why.

### Switching back

The firmware that ran before stays in the other slot until the next update.
The firmware page can switch to it at any time:

- during a trial, at once: the trial ends and the previous firmware runs
  again, as after an automatic rollback;
- after confirmation, as a trial of the other firmware, which rolls back to
  the current one unless confirmed.

The page names the reason when a switch is refused, for example:

- the running firmware is confirmed and the other one has a lower security
  version;
- the other slot holds no complete firmware (an update was refused or
  interrupted), or a corrupted one.

A switch starts the other firmware with its own configuration, as it was
when it last ran: changes made since are not carried back.

## Over USB

USB works whatever the firmware on the device, as long as that firmware
neither reconfigures the USB pins nor burns the eFuses that disable the
loader (ours does neither): it uses the chip's built-in loader. It
is the way to install firmware for the first time, to recover a device that
doesn't come up, and to return to a release that the security version rules
out over the network. When USB doesn't come up at all, the debug header J2
reaches the same loader: hold GPIO9 low while resetting the chip
_(pinout to be confirmed)_.

### With the repository

From the repository, in its Nix development shell:

```sh
flash-all
```

writes the bootloader, the partition table and the app built from the
checkout, and selects the app for the next boot. `flash-app` writes the app
and clears the boot selection, so the board starts it. Both pass the port
with `-p PORT` when there is more than one board.

### With a release

With `esptool` and the release image, into the first slot, erasing the boot
selection so the device starts that slot:

```sh
esptool --chip esp32c6 write-flash 0x10000 eveningstar-<version>-esp32c6.bin
esptool --chip esp32c6 erase-region 0xa000 0x2000
```

Don't pass `--flash-mode`, `--flash-freq` or `--flash-size` when writing
the bootloader: esptool then rewrites its header, and the firmware no
longer recognizes it. Writing `SHA256.sig` into the slot's signature
sector, so the device reports who signed the image, is _(planned)_.

Flashing over USB keeps the configuration. `esp-wipe` erases the whole flash
for a clean start, after which `flash-all` is needed.
