{
  perSystem = { config, pkgs, ... }: {
    # The formats crate's test fixtures that Nix builds from eveningstar.board:
    # the partition table and the bootloader, as `flash-boot` writes them.
    packages.storage-testdata = pkgs.runCommand "storage-testdata" { } ''
      mkdir -p $out
      cp ${config.packages.partition-table}/partition-table.bin $out/
      cp ${config.packages.bootloader}/bootloader.bin $out/
    '';
  };
}
