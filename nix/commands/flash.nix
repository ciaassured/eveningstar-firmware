{ config, lib, ... }:
let
  inherit (config.eveningstar) board;

  bootloaderOffset = { esp32c6 = "0x0"; }.${board.chip};

  appPartition = (lib.findFirst (p: p.type == "app")
    (throw "eveningstar.board.partitions has no app partition")
    board.partitions).name;
in
{
  perSystem = { config, pkgs, ... }:
    let
      inherit (config.packages) bootloader partition-table;

      # Shared setup: parses -p/--port, looks up the resolved layout, and
      # defines helpers. Everything is written with one esptool connection;
      # espflash only converts ELFs to app images, so its bundled bootloader
      # never reaches the device.
      prelude = ''
        port_args=()
        args=()
        while (( $# )); do
          case "$1" in
            -p|--port) port_args=(--port "$2"); shift 2 ;;
            *) args+=("$1"); shift ;;
          esac
        done

        layout=${partition-table}/partitions.json
        work=$(mktemp -d)
        trap 'rm -rf "$work"' EXIT

        # "offset size" of the one partition matching a jq filter on the layout.
        partition() {
          jq -er "[to_entries[] | select($1)] | select(length == 1) | .[0].value | \"\\(.offset) \\(.size)\"" "$layout" \
            || { echo "Expected exactly one partition matching: $1" >&2; exit 1; }
        }

        otadata=$(partition '.value.type == 1 and .value.subtype == 0')
        app=$(partition '.key == "${appPartition}"')
        read -r otadata_offset otadata_size <<< "$otadata"
        read -r app_offset app_size <<< "$app"
        printf -v otadata_offset '0x%x' "$otadata_offset"
        printf -v app_offset '0x%x' "$app_offset"

        # Blank otadata, so the bootloader boots the first app slot.
        head -c "$otadata_size" /dev/zero | tr '\0' '\377' > "$work/otadata.bin"

        boot_regions=(
          ${bootloaderOffset} ${bootloader}/bootloader.bin
          ${board.partitionTableOffset} ${partition-table}/partition-table.bin
          "$otadata_offset" "$work/otadata.bin"
        )

        app_image() {
          local elf=$1
          if [[ ! -f "$elf" ]]; then
            echo "No such ELF: $elf" >&2
            exit 1
          fi

          espflash save-image --chip ${board.chip} \
            --flash-size ${lib.toLower board.flash.size} \
            --flash-mode ${board.flash.mode} \
            --flash-freq ${board.flash.freq}hz \
            "$elf" "$work/app.bin" >&2

          local size
          size=$(stat -c %s "$work/app.bin")
          if (( size > app_size )); then
            echo "App image is $size bytes, ${appPartition} holds $app_size" >&2
            exit 1
          fi
          echo "$work/app.bin"
        }

        write_flash() {
          esptool --chip ${board.chip} "''${port_args[@]}" write-flash "$@"
        }

        require_elf() {
          if (( ''${#args[@]} != 1 )); then
            echo "Usage: $1 [-p PORT] <app.elf>" >&2
            exit 1
          fi
        }
      '';

      mkFlashCommand = name: text: pkgs.writeShellApplication {
        inherit name;
        runtimeInputs = [ pkgs.esptool pkgs.espflash pkgs.jq pkgs.coreutils ];
        # Not every command uses every variable from the shared prelude.
        excludeShellChecks = [ "SC2034" ];
        text = prelude + text;
      };
    in
    {
      packages = {
        # Bootloader, partition table and blank otadata.
        # Usage: flash-boot [-p PORT]
        flash-boot = mkFlashCommand "flash-boot" ''
          if (( ''${#args[@]} != 0 )); then
            echo "Usage: flash-boot [-p PORT]" >&2
            exit 1
          fi
          write_flash "''${boot_regions[@]}"
        '';

        # App into the first app slot, plus blank otadata so it is the one booted.
        # Stub: takes an ELF for now; will default to the Rust firmware package.
        # Usage: flash-app [-p PORT] <app.elf>
        flash-app = mkFlashCommand "flash-app" ''
          require_elf flash-app
          image=$(app_image "''${args[0]}")
          write_flash "$app_offset" "$image" "$otadata_offset" "$work/otadata.bin"
        '';

        # Everything: bootloader, partition table, blank otadata and app.
        # Usage: flash-all [-p PORT] <app.elf>
        flash-all = mkFlashCommand "flash-all" ''
          require_elf flash-all
          image=$(app_image "''${args[0]}")
          write_flash "''${boot_regions[@]}" "$app_offset" "$image"
        '';
      };
    };
}
