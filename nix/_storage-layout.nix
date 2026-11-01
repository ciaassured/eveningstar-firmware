# The partition layout of eveningstar.board as the Rust source of the storage
# crate's layout module, and as data for comparing against the table ESP-IDF
# builds. Pure Nix evaluation: nothing here builds ESP-IDF. Evaluating either
# result throws, naming the partition, when the layout breaks one of the rules
# below.
{ lib }:
board:
let
  # From components/partition_table/gen_esp32part.py.
  typeValues = {
    app = 0;
    data = 1;
    bootloader = 2;
    partition_table = 3;
  };

  subtypeValues = {
    app =
      { factory = 0; test = 32; tee_0 = 48; tee_1 = 49; }
      // lib.listToAttrs (map
        (n: lib.nameValuePair "ota_${toString n}" (16 + n))
        (lib.range 0 15));
    data = {
      ota = 0;
      phy = 1;
      nvs = 2;
      coredump = 3;
      nvs_keys = 4;
      efuse = 5;
      undefined = 6;
      esphttpd = 128;
      fat = 129;
      spiffs = 130;
      littlefs = 131;
      tee_ota = 144;
    };
    bootloader = { primary = 0; ota = 1; recovery = 2; };
    partition_table = { primary = 0; ota = 1; };
  };

  # Flags in a table entry: gen_esp32part sets bit 0 for encrypted and bit 1
  # for readonly.
  flagValues = { readonly = 2; };

  sectorSize = 4096;
  appAlignment = 65536;

  # Where the first partition may start at the earliest: after the bootloader
  # and the partition table's sector.
  firstOffset = 36864;

  # Partitions that stand in for each other, so they have to match in size.
  pairs = [
    [ "config_0" "config_1" ]
    [ "ota_0" "ota_1" ]
  ];

  # Names of the generated items that a partition's constant can't take.
  reservedNames = [ "ENTRY" "FLASH_SIZE" "PARTITIONS" "TABLE_OFFSET" ];

  # Integer, hex, or with a K/M suffix, as gen_esp32part parses them.
  parseAmount = what: text:
    let
      hexDigits = builtins.match "0x([0-9a-fA-F]+)" text;
      decimal = builtins.match "0*([0-9]+)([KM]?)" text;
    in
    if hexDigits != null then lib.fromHexString text
    else if decimal != null then
      lib.toInt (builtins.elemAt decimal 0)
      * { "" = 1; K = 1024; M = 1048576; }.${builtins.elemAt decimal 1}
    else throw "${what}: cannot read ${builtins.toJSON text} as a number";

  flashSize =
    let size = builtins.match "([0-9]+)MB" board.flash.size;
    in
    if size == null then throw "flash size ${board.flash.size} is not in MB"
    else lib.toInt (builtins.head size) * 1048576;

  tableOffset = parseAmount "partitionTableOffset" board.partitionTableOffset;

  resolve = p:
    let
      what = "partition ${p.name}";
      lookup = table: key: kind:
        table.${key} or (throw "${what}: ${kind} ${key} has no known value");
    in
    {
      inherit (p) name;
      typeName = p.type;
      subtypeName = p.subtype;
      type = lookup typeValues p.type "type";
      subtype = lookup subtypeValues.${p.type} p.subtype "subtype";
      offset =
        if p.offset == null
        then throw "${what}: offset is missing; the layout writes every offset out"
        else parseAmount "${what} offset" p.offset;
      size = parseAmount "${what} size" p.size;
      flags = lib.foldl'
        (acc: flag: acc + lookup flagValues flag "flag")
        0
        p.flags;
    };

  resolved = map resolve board.partitions;

  end = p: p.offset + p.size;
  hex = n: lib.toLower (lib.toHexString n);
  showHex = n: "0x${hex n}";
  constName = p: lib.toUpper p.name;
  alignment = p: if p.type == typeValues.app then appAlignment else sectorSize;

  problemsOf = p:
    let
      what = "partition ${p.name}";
      aligned = "${toString (alignment p / 1024)} KiB aligned";
    in
    lib.optional (builtins.match "[A-Za-z_][A-Za-z0-9_]*" p.name == null)
      "${what}: the name is not usable as a Rust identifier"
    ++ lib.optional (lib.elem (constName p) reservedNames)
      "${what}: the name collides with a generated item"
    ++ lib.optional (p.size == 0) "${what}: size is zero"
    ++ lib.optional (lib.mod p.offset (alignment p) != 0)
      "${what}: offset ${showHex p.offset} is not ${aligned}"
    ++ lib.optional (lib.mod p.size (alignment p) != 0)
      "${what}: size ${showHex p.size} is not ${aligned}"
    ++ lib.optional (p.offset < lib.max firstOffset (tableOffset + sectorSize))
      "${what}: offset ${showHex p.offset} is below ${showHex (lib.max firstOffset (tableOffset + sectorSize))}, where the bootloader and the partition table end"
    ++ lib.optional (end p > flashSize)
      "${what}: ends at ${showHex (end p)}, past the ${board.flash.size} flash";

  duplicateProblems = lib.concatMap
    (name: lib.optional
      (lib.count (p: constName p == name) resolved > 1)
      "partition ${name}: the name is used more than once")
    (lib.unique (map constName resolved));

  overlapProblems = lib.concatMap
    (a: lib.concatMap
      (b: lib.optional
        (a.name < b.name && a.offset < end b && b.offset < end a)
        "partitions ${a.name} and ${b.name} overlap")
      resolved)
    resolved;

  pairProblems = lib.concatMap
    (pair:
      let
        find = name: lib.findFirst (p: p.name == name) null resolved;
        first = builtins.elemAt pair 0;
        second = builtins.elemAt pair 1;
        a = find first;
        b = find second;
      in
      if a == null && b == null then [ ]
      else if a == null then [ "partition ${second}: ${first} is missing" ]
      else if b == null then [ "partition ${first}: ${second} is missing" ]
      else lib.optional (a.size != b.size)
        "partitions ${first} and ${second} differ in size (${showHex a.size} and ${showHex b.size})")
    pairs;

  problems = duplicateProblems
    ++ lib.concatMap problemsOf resolved
    ++ overlapProblems
    ++ pairProblems;

  partitions =
    if problems == [ ] then resolved
    else throw "eveningstar.board.partitions:\n  ${lib.concatStringsSep "\n  " problems}";

  # Hex literals the way clippy wants them: groups of four digits.
  pad = width: n: lib.fixedWidthString width "0" (hex n);
  u8 = n: "0x${pad 2 n}";
  u32 = n:
    let digits = pad 8 n;
    in "0x${builtins.substring 0 4 digits}_${builtins.substring 4 4 digits}";

  entryRust = p: ''
    /// The `${p.name}` partition, of type `${p.typeName}` and subtype `${p.subtypeName}`.
    pub const ${constName p}: Entry = Entry {
        label: "${p.name}",
        kind: ${u8 p.type},
        subtype: ${u8 p.subtype},
        offset: ${u32 p.offset},
        size: ${u32 p.size},
        flags: ${u32 p.flags},
    };
  '';

  rust = ''
    //! Partition layout of the internal flash, generated from `nix/board.nix` by
    //! the `storage-layout` command and not edited by hand.

    use crate::Entry;

    /// Size of the flash chip in bytes.
    pub const FLASH_SIZE: u32 = ${u32 flashSize};

    /// Offset of the partition table.
    pub const TABLE_OFFSET: u32 = ${u32 tableOffset};

    ${lib.concatMapStringsSep "\n" entryRust partitions}
    /// Every partition, in table order.
    // One partition per line, whatever the lengths of the names.
    #[rustfmt::skip]
    pub const PARTITIONS: [Entry; ${toString (lib.length partitions)}] = [
    ${lib.concatMapStrings (p: "    ${constName p},\n") partitions}];
  '';
in
{
  inherit rust;

  # The partitions in table order, with numeric types, as the table ESP-IDF
  # builds is read back.
  table = map
    (p: { inherit (p) name type subtype offset size flags; })
    partitions;
}
