{ config, lib, ... }:
let
  layout = import ./_storage-layout.nix { inherit lib; } config.eveningstar.board;
in
{
  perSystem = { pkgs, ... }: {
    # The storage crate's layout module, generated from eveningstar.board.
    packages.storage-layout = pkgs.writeText "layout.rs" layout.rust;

    # The same layout as data: the partitions in table order, to compare with
    # the table ESP-IDF builds from the same board.
    packages.storage-layout-table = pkgs.writeText "storage-layout-table.json"
      (builtins.toJSON layout.table);
  };
}
