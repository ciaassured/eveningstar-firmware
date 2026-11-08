{ config, lib, ... }:
let
  keys = config.eveningstar.keys;

  # A key file's path from the repository's root, which the generator reads
  # and names in what it writes.
  relative = file: lib.removePrefix "./" (lib.path.removePrefix ../. file);
in
{
  perSystem = { pkgs, ... }: {
    # eveningstar.keys as the generator reads it.
    packages.keys = pkgs.writeText "keys.json" (builtins.toJSON {
      dev = relative keys.dev;
      generations = lib.mapAttrsToList
        (generation: g: {
          generation = lib.toInt generation;
          file = relative g.file;
          inherit (g) lost compromised;
        })
        keys.generations;
      inherit (keys) signing;
    });
  };
}
