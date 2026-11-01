{
  perSystem = { pkgs, ... }: {
    # Write the storage crate's layout module from eveningstar.board into the
    # repository the command runs in.
    # Usage: storage-layout
    packages.storage-layout-write = pkgs.writeShellApplication {
      name = "storage-layout";
      runtimeInputs = [ pkgs.git pkgs.coreutils ];
      text = ''
        if (( $# != 0 )); then
          echo "Usage: storage-layout" >&2
          exit 1
        fi

        root=$(git rev-parse --show-toplevel)
        layout=$(nix build --no-link --print-out-paths "$root#storage-layout")
        install -m 644 "$layout" "$root/crates/eveningstar-storage/src/layout.rs"
      '';
    };
  };
}
