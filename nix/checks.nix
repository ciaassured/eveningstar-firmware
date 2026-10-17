{ inputs, ... }:
{
  perSystem = { config, pkgs, ... }:
    let
      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../rust-toolchain.toml;
    in
    {
      # Every repository check, with a combined result, so the Git hooks and CI
      # share one entry point. Arguments select check groups: `lint` (rustfmt
      # and clippy) and `build` (a release build, which also links). With no
      # arguments, every group runs.
      packages.checks = pkgs.writeShellApplication {
        name = "eveningstar-checks";
        runtimeInputs = [ toolchain pkgs.stdenv.cc ];
        text = ''
          status=0

          run_check() {
            check_name="$1"
            shift

            echo
            echo "==> $check_name"

            if "$@"; then
              echo "==> $check_name passed"
            else
              check_status=$?
              echo "::error::$check_name failed (exit code $check_status)"
              status=1
            fi
          }

          # A separate target directory, so these checks and builds with another
          # cargo don't keep invalidating each other's artifacts.
          export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-target/checks}"

          groups=("$@")
          if [ "''${#groups[@]}" -eq 0 ]; then
            groups=(lint build)
          fi

          for group in "''${groups[@]}"; do
            case "$group" in
              lint)
                run_check "rustfmt" cargo fmt --all -- --check
                run_check "clippy" cargo clippy --all-features --workspace -- -D warnings
                ;;
              build)
                run_check "release build" cargo build --release
                ;;
              *)
                echo "unknown check group: $group (expected lint or build)" >&2
                exit 2
                ;;
            esac
          done

          echo
          if [ "$status" -eq 0 ]; then
            echo "All checks passed."
          else
            echo "One or more checks failed."
          fi

          exit "$status"
        '';
      };

      apps.checks = {
        type = "app";
        program = "${config.packages.checks}/bin/eveningstar-checks";
      };
    };
}
