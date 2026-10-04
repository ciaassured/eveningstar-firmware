{ lib, ... }:
let
  inherit (lib) mkOption types;

  # From components/partition_table/gen_esp32part.py.
  subtypes = {
    app = [ "factory" "test" ]
      ++ map (n: "ota_${toString n}") (lib.range 0 15)
      ++ [ "tee_0" "tee_1" ];
    data = [
      "ota" "phy" "nvs" "coredump" "nvs_keys" "efuse" "undefined"
      "esphttpd" "fat" "spiffs" "littlefs" "tee_ota"
    ];
    bootloader = [ "primary" "ota" "recovery" ];
    partition_table = [ "primary" "ota" ];
  };

  # Data partitions the bootloader and app always write to.
  alwaysReadWrite = [ "ota" "coredump" ];

  # Integer, hex, or with a K/M suffix, as gen_esp32part parses them.
  flashAmount = types.strMatching "0x[0-9a-fA-F]+|[0-9]+[KM]?";

  partition = types.submodule {
    options = {
      name = mkOption {
        type = types.strMatching ".{1,16}";
        description = "Partition name, at most 16 characters.";
      };
      type = mkOption {
        type = types.enum (lib.attrNames subtypes);
      };
      subtype = mkOption {
        type = types.enum (lib.unique (lib.concatLists (lib.attrValues subtypes)));
        description = "Must be a subtype of the partition's type.";
      };
      offset = mkOption {
        type = types.nullOr flashAmount;
        default = null;
        description = "Offset, or null to place it after the previous partition.";
      };
      size = mkOption {
        type = flashAmount;
        description = "Size, e.g. 0x4000, 16K or 3M.";
      };
      flags = mkOption {
        # "encrypted" is left out on purpose: this firmware never uses flash encryption.
        type = types.listOf (types.enum [ "readonly" ]);
        default = [ ];
      };
    };
  };

  checkPartition = p:
    if !lib.elem p.subtype subtypes.${p.type} then
      throw "partition ${p.name}: subtype ${p.subtype} is not valid for type ${p.type} (valid: ${lib.concatStringsSep ", " subtypes.${p.type}})"
    else if p.type == "data" && lib.elem p.subtype alwaysReadWrite && lib.elem "readonly" p.flags then
      throw "partition ${p.name}: data/${p.subtype} partitions are always read-write and cannot be readonly"
    else
      p;
in
{
  options.eveningstar = {
    board = {
      chip = mkOption {
        type = types.enum [ "esp32c6" ];
        description = "ESP chip on the board.";
      };

      flash = {
        size = mkOption {
          type = types.enum [ "1MB" "2MB" "4MB" "8MB" "16MB" "32MB" "64MB" "128MB" ];
        };
        mode = mkOption {
          type = types.enum [ "qio" "qout" "dio" "dout" ];
        };
        freq = mkOption {
          type = types.enum [ "80m" "40m" "20m" ];
        };
      };

      partitionTableOffset = mkOption {
        type = types.str;
        default = "0x8000";
      };

      partitions = mkOption {
        type = types.listOf partition;
        apply = map checkPartition;
        description = "Partition table, in flash order.";
      };
    };

    bootloader.sdkconfig = mkOption {
      type = types.attrsOf (types.oneOf [ types.bool types.int types.str ]);
      default = { };
      description = ''
        Bootloader-only Kconfig settings, without the CONFIG_ prefix. Hex values
        are written as strings ("0x8000"). Every value is checked against the
        resolved sdkconfig, so typos and settings whose dependencies are not met
        fail the build. Symbols derived from eveningstar.board, and SECURE_*, may
        not be set here.
      '';
    };
  };
}
