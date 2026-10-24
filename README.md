# Eveningstar firmware

Firmware for **Eveningstar**, an open-source interface board for Morningstar
solar charge controllers. The board connects to a charge controller's MeterBus
port and makes it reachable over Ethernet. WiFi is only used for a
configuration portal.

**The board's hardware design lives in
[ciaassured/eveningstar-hardware](https://github.com/ciaassured/eveningstar-hardware).**
This repository holds only the firmware that runs on it.

> [!WARNING]
> This firmware is under active development and is missing the features needed
> to monitor a charge controller. Expect its behaviour and API to change
> without notice.

## Hardware

The board is built around an ESP32-C6. Its USB-C port is wired to the chip's
USB Serial/JTAG controller, which is used for flashing and debugging, so no
external probe is needed. See the
[hardware repository](https://github.com/ciaassured/eveningstar-hardware) for
everything else.

## Building and flashing

The tools come from the Nix flake's development shell. Enter it with
`nix develop`, or run single commands with `nix develop --command <cmd>`. The
Rust toolchain is pinned in `rust-toolchain.toml`.

To flash a board, connect it over USB-C and run:

```sh
flash-all
```

This writes the custom bootloader with OTA update support, the partition table
and the app, as laid out in `nix/board.nix`. `flash-boot` and `flash-app`
write the bootloader and the app on their own.

For development, `cargo run --release` builds the app, flashes it through
probe-rs and streams its logs.

> [!CAUTION]
> Don't use `cargo run` to flash a board for real use. probe-rs writes its own
> bootloader and partition table instead of the custom ones, so the board loses
> OTA update support. Run `flash-all` to restore them.

Checks (formatting, Clippy and a release build), the same ones CI runs:

```sh
nix run .#checks
```

## Design

[docs/architecture.md](docs/architecture.md) describes how the firmware is
structured and why.

## License

Licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
