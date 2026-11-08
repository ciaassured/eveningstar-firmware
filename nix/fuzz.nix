{ inputs, ... }:
{
  perSystem = { pkgs, ... }:
    let
      # cargo-fuzz needs a nightly compiler: this dated one, which
      # rust-overlay's locked revision provides.
      nightly = (inputs.rust-overlay.lib.mkRustBin { } pkgs).nightly."2026-10-12".minimal;
    in
    {
      # Runs the fuzz targets in `fuzz/` from copies of their committed
      # corpora, so a run adds nothing to the repository; with `--merge`, the
      # inputs a run found that reach new coverage are merged into the
      # committed corpora afterwards.
      # Usage: fuzz [--merge] [SECONDS] [TARGET...]; every target, 60 seconds
      # each, by default.
      packages.fuzz = pkgs.writeShellApplication {
        name = "fuzz";
        runtimeInputs = [ nightly pkgs.cargo-fuzz pkgs.stdenv.cc pkgs.coreutils pkgs.git ];
        text = ''
          merge=false
          if [[ ''${1:-} == --merge ]]; then
            merge=true
            shift
          fi
          seconds=''${1:-60}
          shift || true
          # The fuzz crate builds into its own target directory, whoever runs it.
          unset CARGO_TARGET_DIR
          root=$(git rev-parse --show-toplevel)
          cd "$root/fuzz"
          if (( $# == 0 )); then
            mapfile -t targets < <(cargo fuzz list)
          else
            targets=("$@")
          fi
          work=$(mktemp -d)
          trap 'rm -rf "$work"' EXIT
          for target in "''${targets[@]}"; do
            echo "==> fuzzing $target for $seconds s"
            mkdir -p "$work/$target"
            if [[ -d "corpus/$target" ]]; then
              cp -r "corpus/$target/." "$work/$target/"
            fi
            cargo fuzz run "$target" "$work/$target" -- \
              -max_total_time="$seconds" -print_final_stats=1
            if $merge; then
              echo "==> merging $target's new inputs into corpus/$target"
              mkdir -p "corpus/$target"
              cargo fuzz run "$target" "corpus/$target" "$work/$target" -- -merge=1
            fi
          done
        '';
      };
    };
}
