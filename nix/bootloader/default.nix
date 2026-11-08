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
    BOOTLOADER_APP_ROLLBACK_ENABLE = true;
    BOOTLOADER_COMPILE_TIME_DATE = false;
  } // pinned;

  # What OTA's rollback and recovery rely on: see specs/ota-signing.md,
  # "Bootloader configuration". Every image is validated on every boot, the
  # deep sleep fast path that boots without selecting is off, no eFuse is
  # burnt for anti-rollback, the RTC watchdog covers start-up until esp-hal
  # takes over, and nothing reserves the RTC memory the diagnostics mailbox
  # uses.
  pinned = {
    BOOTLOADER_SKIP_VALIDATE_ON_POWER_ON = false;
    BOOTLOADER_SKIP_VALIDATE_ALWAYS = false;
    BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP = false;
    BOOTLOADER_APP_ANTI_ROLLBACK = false;
    BOOTLOADER_WDT_ENABLE = true;
    BOOTLOADER_WDT_DISABLE_IN_USER_CODE = false;
    BOOTLOADER_FACTORY_RESET = false;
    BOOTLOADER_CUSTOM_RESERVE_RTC = false;
    BOOTLOADER_FLASH_XMC_SUPPORT = true;
  };

  # Symbols that must resolve unset, whatever selects them.
  forbidden = [ "BOOTLOADER_RESERVE_RTC_MEM" ];

  reservedPrefixes = [ "IDF_TARGET" "ESPTOOLPY_FLASH" "PARTITION_TABLE_" "SECURE_" ];
  reservedKeys = lib.filter
    (key: lib.any (prefix: lib.hasPrefix prefix key) reservedPrefixes
      || lib.elem key (lib.attrNames pinned ++ forbidden))
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
          inherit project settings forbidden;
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
