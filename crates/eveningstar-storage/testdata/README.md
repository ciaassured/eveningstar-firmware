# Test fixtures

Real binaries the storage crate's tests read with `include_bytes!`.

| File | Bytes | What it is |
|---|---|---|
| `partition-table.bin` | 3072 | The partition table Nix builds from `nix/board.nix` (`nix build .#partition-table`), as `flash-boot` writes it at 0x8000. |
| `bootloader.bin` | 22672 | The ESP-IDF bootloader Nix builds (`nix build .#bootloader`), as `flash-boot` writes it at 0x0. |
| `espflash-bootloader.bin` | 32768 | Bytes 0x0..0x8000 of what espflash 4.6.0 writes for an ESP32-C6 with 8 MB of flash: its bundled bootloader (ESP-IDF v6.1-beta1), then erased bytes. |
| `espflash-partition-table.bin` | 3072 | Bytes 0x8000..0x8C00 of the same image: espflash's default table (`nvs`, `phy_init`, a `factory` app at 0x10000). |
| `app.bin` | 217856 | An app image of the `eveningstar` firmware, as espflash 4.6.0 makes it from the release ELF. |
| `otadata-first-boot.bin` | 8192 | The otadata partition (0xA000..0xC000) read from a dev board after the Nix bootloader's first boot on blank otadata: entry 0 is sequence 1, `ESP_OTA_IMG_VALID`, and the rest is erased. |

## Tracking the Nix build

`partition-table.bin` and `bootloader.bin` have to equal what Nix builds:
the `build` check group fails when they don't, and the `storage-testdata`
dev-shell command rewrites them. The tests also hold the bootloader's
SHA-256 digests (the one the image stores, and the `sha256sum` of the file),
which change with it, and a new bootloader needs its entry
in `src/known.rs`, which the `build` group prints.

## Snapshots

The espflash files and `app.bin` are frozen snapshots, made once with the dev
shell's espflash from a `cargo build --release` ELF (without the web UI):

```sh
elf=target/riscv32imac-unknown-none-elf/release/eveningstar
espflash save-image --chip esp32c6 --flash-size 8mb --merge "$elf" merged.bin
head -c 32768 merged.bin > espflash-bootloader.bin
tail -c +32769 merged.bin | head -c 3072 > espflash-partition-table.bin
espflash save-image --chip esp32c6 --flash-size 8mb "$elf" app.bin
```

They are what espflash 4.6.0 writes. `cargo run` flashes through probe-rs,
which writes espflash's bootloader and table through the espflash library it
links, of a version that may differ. Nothing checks them against newer
builds.

## Captured from a board

`otadata-first-boot.bin` is what the Nix-built bootloader
(`eveningstar-bootloader-esp32c6-idf-v5.5.2`, the same bytes as
`bootloader.bin`) wrote on a dev board. With the board on USB, from the
repository root:

```sh
nix develop --command flash-all
# about 10 s for the board to boot
nix develop --command esptool --chip esp32c6 read-flash 0xA000 0x2000 \
  crates/eveningstar-storage/testdata/otadata-first-boot.bin
```

`flash-all` writes the bootloader, the table, blank otadata and the app at
`ota_0`, with `ota_1` left as it was. The read used esptool v5.4.0 and
resets the chip into the ROM loader and back. The file's SHA-256 is
`8ba3b110139f45443d4f268d1a3373ef99a1718b71d51664531b83ee2d4b91a3`. It is a
frozen snapshot: nothing rebuilds it.
