{
  perSystem = { pkgs, ... }: {
    # Write the formats crate's Nix-built test fixtures, the partition table
    # and the bootloader, into the repository the command runs in.
    # Usage: storage-testdata
    packages.storage-testdata-write = pkgs.writeShellApplication {
      name = "storage-testdata";
      runtimeInputs = [ pkgs.git pkgs.coreutils ];
      text = ''
        if (( $# != 0 )); then
          echo "Usage: storage-testdata" >&2
          exit 1
        fi

        root=$(git rev-parse --show-toplevel)
        testdata=$(nix build --no-link --print-out-paths "$root#storage-testdata")
        for file in partition-table.bin bootloader.bin; do
          install -m 644 "$testdata/$file" "$root/crates/eveningstar-esp-formats/testdata/$file"
        done
      '';
    };
  };
}
