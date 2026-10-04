{
  perSystem = { pkgs, ... }: {
    # Erase the entire flash of the connected ESP32.
    # Usage: esp-wipe [-y|--yes] [espflash erase-flash options, e.g. --port /dev/ttyUSB0]
    packages.esp-wipe = pkgs.writeShellApplication {
      name = "esp-wipe";
      runtimeInputs = [ pkgs.espflash ];
      text = ''
        confirm=1
        if [[ "''${1:-}" == "-y" || "''${1:-}" == "--yes" ]]; then
          confirm=0
          shift
        fi

        if (( confirm )); then
          read -r -p "Erase the entire flash of the connected ESP32? [y/N] " answer
          if [[ "$answer" != [yY]* ]]; then
            echo "Aborted." >&2
            exit 1
          fi
        fi

        exec espflash erase-flash "$@"
      '';
    };
  };
}
