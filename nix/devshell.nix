{
  perSystem = { config, pkgs, ... }: {
    devShells.default = pkgs.mkShell {
      packages = with pkgs; [
        # Only the binaries: the package itself would put its Python 3.14
        # dependencies on PYTHONPATH, which breaks ESP-IDF's Python.
        (runCommand "esptool-bin" { } "mkdir $out && ln -s ${esptool}/bin $out/bin")
        espflash
        esp-generate
        probe-rs-tools
        config.packages.esp-idf
        config.packages.esp-wipe
        config.packages.bootloader-menuconfig
        config.packages.flash-boot
        config.packages.flash-app
        config.packages.flash-all
      ];

      # Default target for idf.py when a project has no sdkconfig yet.
      IDF_TARGET = "esp32c6";
    };
  };
}
