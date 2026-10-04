{
  perSystem = { config, pkgs, ... }:
    let
      inherit (config.packages) sdkconfig;
    in
    {
      # Browse the resolved bootloader sdkconfig in menuconfig. Changes are not
      # saved; they are printed as Nix to paste into eveningstar.bootloader.sdkconfig.
      # Run inside the dev shell, which provides the matching ESP-IDF.
      packages.bootloader-menuconfig = pkgs.writeShellApplication {
        name = "bootloader-menuconfig";
        runtimeInputs = [ pkgs.diffutils pkgs.gnused ];
        text = ''
          if [[ "''${IDF_PATH:-}" != "${sdkconfig.esp-idf}" ]]; then
            echo "Run this inside the dev shell (nix develop)." >&2
            exit 1
          fi

          work=$(mktemp -d)
          trap 'rm -rf "$work"' EXIT

          cp -r ${sdkconfig.project}/. "$work"
          chmod -R u+w "$work"
          cp ${sdkconfig}/sdkconfig "$work/sdkconfig"
          chmod u+w "$work/sdkconfig"
          cd "$work"

          export IDF_COMPONENT_MANAGER=0
          export IDF_TARGET=${sdkconfig.target}

          # Only symbols set away from their defaults, without derived values.
          defconfig() {
            idf.py save-defconfig >/dev/null
            grep -E '^(CONFIG_|# CONFIG_.* is not set)' sdkconfig.defaults | sort
          }

          defconfig > before
          idf.py menuconfig
          defconfig > after

          changes=$(comm -13 before after)
          if [[ -z "$changes" ]]; then
            echo "No changes."
            exit 0
          fi

          echo
          echo "Changed settings, for eveningstar.bootloader.sdkconfig:"
          echo
          sed -E \
            -e 's/^# CONFIG_(.*) is not set$/  \1 = false;/' \
            -e 's/^CONFIG_([^=]*)=y$/  \1 = true;/' \
            -e 's/^CONFIG_([^=]*)=(0x[0-9a-fA-F]+)$/  \1 = "\2";/' \
            -e 's/^CONFIG_([^=]*)=(-?[0-9]+)$/  \1 = \2;/' \
            -e 's/^CONFIG_([^=]*)=(".*")$/  \1 = \2;/' \
            <<< "$changes"

          # Symbols that went back to their default no longer appear at all.
          keys() { sed -E 's/^(# )?CONFIG_([A-Za-z0-9_]+).*/\2/' | sort -u; }
          removed=$(comm -23 <(comm -23 before after | keys) <(keys <<< "$changes"))
          if [[ -n "$removed" ]]; then
            echo
            echo "Back to default (remove if set):"
            while read -r key; do echo "  $key"; done <<< "$removed"
          fi
        '';
      };
    };
}
