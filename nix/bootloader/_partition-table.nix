# Build the partition table binary from a resolved sdkconfig, so flash size and
# table offset always match the bootloader.
{ callPackage, sdkconfig }:

let
  inherit (sdkconfig) esp-idf project target;
  idfBuild = callPackage ./_idf-build.nix { inherit esp-idf; };
in
idfBuild {
  name = "eveningstar-partition-table";
  inherit project target;
  sdkconfig = "${sdkconfig}/sdkconfig";
  idfArgs = "partition-table";

  installPhase = ''
    cmp sdkconfig ${sdkconfig}/sdkconfig

    mkdir -p $out
    cp build/partition_table/partition-table.bin $out/
    cp partitions.csv $out/

    # Resolved layout (offsets filled in), read back from the binary itself.
    python3 - build/partition_table/partition-table.bin $out/partitions.json <<'PY'
    import json, os, sys
    sys.path.insert(0, os.path.join(os.environ["IDF_PATH"], "components", "partition_table"))
    import gen_esp32part

    table = gen_esp32part.PartitionTable.from_binary(open(sys.argv[1], "rb").read())
    layout = {
        p.name: {"type": p.type, "subtype": p.subtype, "offset": p.offset, "size": p.size}
        for p in table
    }
    json.dump(layout, open(sys.argv[2], "w"), indent=2)
    PY
  '';
}
