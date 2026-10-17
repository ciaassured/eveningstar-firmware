{
  perSystem = { config, pkgs, ... }: {
    devShells.default = pkgs.mkShell {
      packages = with pkgs; [
        # Only the binaries: the package itself would put its Python 3.14
        # dependencies on PYTHONPATH, which breaks ESP-IDF's Python.
        (runCommand "esptool-bin" { } "mkdir $out && ln -s ${esptool}/bin $out/bin")
        espflash
        esp-generate
        gdb
        lefthook
        probe-rs-tools
        config.packages.checks
        config.packages.esp-idf
        config.packages.esp-wipe
        config.packages.bootloader-menuconfig
        config.packages.flash-boot
        config.packages.flash-app
        config.packages.flash-all
      ];

      # Default target for idf.py when a project has no sdkconfig yet.
      IDF_TARGET = "esp32c6";

      shellHook = ''
        if ${pkgs.git}/bin/git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
          if ! ${pkgs.lefthook}/bin/lefthook install >/dev/null; then
            echo "warning: could not install the Eveningstar Git hooks" >&2
            echo "run 'lefthook install' for details" >&2
          fi
        fi
      '';
    };
  };
}
