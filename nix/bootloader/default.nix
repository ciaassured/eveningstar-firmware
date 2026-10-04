{ config, lib, ... }:
let
  inherit (config.eveningstar) board bootloader;

  # Symbols owned by eveningstar.board, plus a hard ban on secure boot and
  # flash encryption.
  boardSdkconfig = {
    IDF_TARGET = board.chip;
    "ESPTOOLPY_FLASHSIZE_${board.flash.size}" = true;
    "ESPTOOLPY_FLASHMODE_${lib.toUpper board.flash.mode}" = true;
    "ESPTOOLPY_FLASHFREQ_${lib.toUpper board.flash.freq}" = true;
    PARTITION_TABLE_CUSTOM = true;
    PARTITION_TABLE_CUSTOM_FILENAME = "partitions.csv";
    PARTITION_TABLE_OFFSET = board.partitionTableOffset;
    SECURE_BOOT = false;
    SECURE_FLASH_ENC_ENABLED = false;
  };

  reservedPrefixes = [ "IDF_TARGET" "ESPTOOLPY_FLASH" "PARTITION_TABLE_" "SECURE_" ];
  reservedKeys = lib.filter
    (key: lib.any (prefix: lib.hasPrefix prefix key) reservedPrefixes)
    (lib.attrNames bootloader.sdkconfig);

  settings =
    if reservedKeys != [ ] then
      throw "eveningstar.bootloader.sdkconfig may not set ${lib.concatStringsSep ", " reservedKeys}; these come from eveningstar.board or are forbidden"
    else
      boardSdkconfig // bootloader.sdkconfig;

  partitionsCsv = lib.concatMapStrings
    (p: lib.concatStringsSep ", " [
      p.name
      p.type
      p.subtype
      (if p.offset == null then "" else p.offset)
      p.size
      (lib.concatStringsSep ":" p.flags)
    ] + "\n")
    board.partitions;
in
{
  perSystem = { config, pkgs, ... }:
    let
      project = pkgs.callPackage ./_project.nix {
        partitionsCsv = pkgs.writeText "partitions.csv" partitionsCsv;
      };
    in
    {
      packages = {
        sdkconfig = pkgs.callPackage ./_sdkconfig.nix {
          inherit (config.packages) esp-idf;
          inherit project settings;
          target = board.chip;
        };

        bootloader = pkgs.callPackage ./_bootloader.nix {
          inherit (config.packages) sdkconfig;
        };

        partition-table = pkgs.callPackage ./_partition-table.nix {
          inherit (config.packages) sdkconfig;
        };
      };
    };
}
