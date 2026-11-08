{ inputs, ... }:
{
  perSystem = { pkgs, ... }:
    let
      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../../rust-toolchain.toml;
    in
    {
      # Write the signify crate's trust sets and the README's key table from
      # eveningstar.keys into the repository the command runs in, or, with
      # `--check`, fail when the committed ones differ.
      # Usage: trust-sets [--check]
      packages.trust-sets = pkgs.writeShellApplication {
        name = "trust-sets";
        runtimeInputs = [ toolchain pkgs.stdenv.cc pkgs.git pkgs.coreutils pkgs.diffutils pkgs.gawk ];
        text = ''
          check=false
          case "$*" in
            "") ;;
            --check) check=true ;;
            *)
              echo "Usage: trust-sets [--check]" >&2
              exit 1
              ;;
          esac

          root=$(git rev-parse --show-toplevel)
          cd "$root"
          keys=$(nix build --no-link --print-out-paths .#keys)
          work=$(mktemp -d)
          trap 'rm -rf "$work"' EXIT

          generate() {
            cargo run --quiet --package eveningstar-signify --example trust-sets \
              --target "$(rustc --print host-tuple)" -- "$keys" "$1"
          }
          generate rust | rustfmt --edition 2024 > "$work/trust.rs"
          # Blank lines around the table, so it is a block of its own.
          { echo; generate readme; echo; } > "$work/table.md"

          # The README with the lines between its key table markers replaced.
          awk -v table="$work/table.md" '
            /^<!-- trust-sets:end -->$/ { while ((getline line < table) > 0) print line; skip = 0 }
            !skip { print }
            /^<!-- trust-sets:begin -->$/ { skip = 1; found = 1 }
            END { if (!found) exit 1 }
          ' README.md > "$work/README.md" || {
            echo "README.md has no <!-- trust-sets:begin --> marker" >&2
            exit 1
          }

          if $check; then
            status=0
            diff -u crates/eveningstar-signify/src/trust.rs "$work/trust.rs" || status=1
            diff -u README.md "$work/README.md" || status=1
            if (( status != 0 )); then
              echo "the trust sets or the README's key table don't match nix/keys.nix: run trust-sets" >&2
            fi
            exit "$status"
          fi
          install -m 644 "$work/trust.rs" crates/eveningstar-signify/src/trust.rs
          install -m 644 "$work/README.md" README.md
        '';
      };
    };
}
