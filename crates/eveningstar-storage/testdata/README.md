# Test fixtures

Real binaries the storage crate's tests read with `include_bytes!`.

| File | Bytes | What it is |
|---|---|---|
| `partition-table.bin` | 3072 | The partition table Nix builds from `nix/board.nix` (`nix build .#partition-table`), as `flash-boot` writes it at 0x8000. |
| `bootloader.bin` | 22672 | The ESP-IDF bootloader Nix builds (`nix build .#bootloader`), as `flash-boot` writes it at 0x0. |
| `espflash-bootloader.bin` | 32768 | Bytes 0x0..0x8000 of what espflash 4.6.0 writes for an ESP32-C6 with 8 MB of flash: its bundled bootloader (ESP-IDF v6.1-beta1), then erased bytes. |
| `espflash-partition-table.bin` | 3072 | Bytes 0x8000..0x8C00 of the same image: espflash's default table (`nvs`, `phy_init`, a `factory` app at 0x10000). |
| `app.bin` | 217856 | An app image of the `eveningstar` firmware, as espflash 4.6.0 makes it from the release ELF. |

## Tracking the Nix build

`partition-table.bin` and `bootloader.bin` have to equal what Nix builds:
the `build` check group fails when they don't, and the `storage-testdata`
dev-shell command rewrites them. The tests also hold the bootloader's
SHA-256 digest, which changes with it.

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

They stand for what `cargo run` (probe-rs, with espflash's bootloader and
table) and an app build put on a board, and nothing checks them against
newer builds.
