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
      # and clippy, and the web UI's lint and type check), `build` (a release
      # build, which also links, with the web UI bundled) and `test` (the tests
      # of the crates that also build on the host, and the web UI's). With no
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
            groups=(lint build test)
          fi

          # The crates that also build on the host, and the host's target.
          # Their features stay out of the firmware's lint, as `--all-features`
          # would otherwise enable them in the firmware's dependency on them.
          host_crates=(eveningstar-api)
          host_target="$(rustc --print host-tuple)"

          for group in "''${groups[@]}"; do
            case "$group" in
              lint)
                run_check "rustfmt" cargo fmt --all -- --check
                run_check "clippy" cargo clippy --all-features --workspace \
                  "''${host_crates[@]/#/--exclude=}" -- -D warnings
                run_check "clippy (host crates, chip target)" cargo clippy --all-features \
                  "''${host_crates[@]/#/--package=}" -- -D warnings
                # The stack frame limit is the chip's, which the run above checks.
                run_check "clippy (host crates, host target)" cargo clippy --all-features \
                  --all-targets "''${host_crates[@]/#/--package=}" --target "$host_target" \
                  -- -D warnings -A clippy::large_stack_frames
                run_check "web lint and type check" nix build --no-link .#web-lint .#web-typecheck
                ;;
              build)
                # The web UI is built by Nix alone, so no npm code runs on the
                # host, and only here, so `lint` doesn't build the whole chain.
                if web_ui="$(nix build --no-link --print-out-paths .#web-ui-bundle)"; then
                  EVENINGSTAR_WEB_UI="$web_ui" run_check "release build" cargo build --release
                else
                  echo "::error::web UI bundle failed"
                  status=1
                fi
                ;;
              test)
                run_check "host tests" cargo test --all-features \
                  "''${host_crates[@]/#/--package=}" --target "$host_target"
                run_check "web tests" nix build --no-link .#web-test
                ;;
              *)
                echo "unknown check group: $group (expected lint, build or test)" >&2
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
