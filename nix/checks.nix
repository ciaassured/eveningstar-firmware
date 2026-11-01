{ config, inputs, lib, ... }:
let
  inherit (config.eveningstar) board;

  # ELF to app image, as nix/app.nix makes it, but with the partition table's
  # path given at run time: a store path here would build ESP-IDF for every
  # check group.
  saveImageArgs = lib.escapeShellArgs [
    "--chip" board.chip
    "--flash-size" (lib.toLower board.flash.size)
    "--flash-mode" board.flash.mode
    "--flash-freq" "${board.flash.freq}hz"
    "--partition-table-offset" board.partitionTableOffset
    "--target-app-partition" "ota_0"
    "--skip-update-check"
  ];
in
{
  perSystem = { config, pkgs, ... }:
    let
      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../rust-toolchain.toml;
    in
    {
      # Every repository check, with a combined result, so the Git hooks and CI
      # share one entry point. Arguments select check groups: `lint` (rustfmt
      # and clippy, that the generated storage layout is current, and the web
      # UI's lint and type check), `build` (a release build, which also links,
      # with the web UI bundled, the partition table ESP-IDF builds checked
      # against the storage layout, the storage crate's Nix-built test fixtures
      # checked against what Nix builds, and the Nix bootloader, table and the
      # built app image run through the storage crate's `check-images`) and
      # `test` (the tests of the
      # crates that also build on the host, and the web UI's). With no
      # arguments, every group runs. `lint` and `test` are each two finer
      # groups, `rust-lint` and `web-lint`, `rust-test` and `web-test`, which CI
      # runs as separate jobs.
      packages.checks = pkgs.writeShellApplication {
        name = "eveningstar-checks";
        runtimeInputs = [ toolchain pkgs.stdenv.cc pkgs.diffutils pkgs.espflash pkgs.jq ];
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

          # The storage crate's layout module is generated from nix/board.nix, and
          # the committed file has to equal what the board generates.
          # shellcheck disable=SC2329 # run through run_check
          check_storage_layout() {
            local generated
            generated="$(nix build --no-link --print-out-paths .#storage-layout)" || return 1
            diff -u crates/eveningstar-storage/src/layout.rs "$generated" || {
              echo "crates/eveningstar-storage/src/layout.rs does not match nix/board.nix: run storage-layout" >&2
              return 1
            }
          }

          # The table ESP-IDF builds from nix/board.nix has to resolve to the
          # partitions, offsets, sizes, types, subtypes and flags that the
          # generated layout lists, in the same order.
          # shellcheck disable=SC2329 # run through run_check
          check_partition_table() {
            local table expected
            table="$(nix build --no-link --print-out-paths .#partition-table)" || return 1
            expected="$(nix build --no-link --print-out-paths .#storage-layout-table)" || return 1
            diff -u <(jq -S . "$expected") \
              <(jq -S '[to_entries[] | {name: .key} + .value]' "$table/partitions.json") || {
              echo "the partition table ESP-IDF builds differs from the storage layout" >&2
              return 1
            }
          }

          # The storage crate's tests read the partition table and bootloader
          # Nix builds from committed copies, which have to equal what Nix
          # builds now.
          # shellcheck disable=SC2329 # run through run_check
          check_storage_testdata() {
            local testdata file stale=0
            testdata="$(nix build --no-link --print-out-paths .#storage-testdata)" || return 1
            for file in partition-table.bin bootloader.bin; do
              cmp "crates/eveningstar-storage/testdata/$file" "$testdata/$file" || stale=1
            done
            if [ "$stale" -ne 0 ]; then
              echo "crates/eveningstar-storage/testdata does not match what Nix builds: run storage-testdata" >&2
              return 1
            fi
          }

          # The bootloader and table Nix builds and the app image of the release
          # build, checked by the code the firmware runs at boot: the table
          # against the layout, the bootloader against the known list, and the
          # app's chip, project and size against its slot.
          # shellcheck disable=SC2329 # run through run_check
          check_images() {
            local bootloader table dir result=0
            bootloader="$(nix build --no-link --print-out-paths .#bootloader)" || return 1
            table="$(nix build --no-link --print-out-paths .#partition-table)" || return 1
            dir="$(mktemp -d)"
            if espflash save-image ${saveImageArgs} --partition-table "$table/partitions.csv" \
              "$CARGO_TARGET_DIR/riscv32imac-unknown-none-elf/release/eveningstar" \
              "$dir/eveningstar.bin"; then
              cargo run --quiet --package eveningstar-storage --example check-images \
                --target "$host_target" -- \
                --bootloader "$bootloader/bootloader.bin" \
                --partition-table "$table/partition-table.bin" \
                --app "$dir/eveningstar.bin" || result=1
            else
              result=1
            fi
            rm -rf "$dir"
            return "$result"
          }

          # The release build, noting that it passed, so the image checks never
          # read an ELF left from an earlier build.
          # shellcheck disable=SC2329 # run through run_check
          release_build() {
            EVENINGSTAR_WEB_UI="$1" cargo build --release && release_built=1
          }
          release_built=0

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
          host_crates=(eveningstar-api eveningstar-storage)
          host_target="$(rustc --print host-tuple)"

          expanded=()
          for group in "''${groups[@]}"; do
            case "$group" in
              lint) expanded+=(rust-lint web-lint) ;;
              test) expanded+=(rust-test web-test) ;;
              *) expanded+=("$group") ;;
            esac
          done

          for group in "''${expanded[@]}"; do
            case "$group" in
              rust-lint)
                run_check "rustfmt" cargo fmt --all -- --check
                run_check "storage layout" check_storage_layout
                run_check "clippy" cargo clippy --all-features --workspace \
                  "''${host_crates[@]/#/--exclude=}" -- -D warnings
                run_check "clippy (host crates, chip target)" cargo clippy --all-features \
                  "''${host_crates[@]/#/--package=}" -- -D warnings
                # The stack frame limit is the chip's, which the run above checks.
                run_check "clippy (host crates, host target)" cargo clippy --all-features \
                  --all-targets "''${host_crates[@]/#/--package=}" --target "$host_target" \
                  -- -D warnings -A clippy::large_stack_frames
                ;;
              web-lint)
                run_check "web lint and type check" nix build --no-link .#web-lint .#web-typecheck
                ;;
              build)
                # The web UI is built by Nix alone, so no npm code runs on the
                # host, and only here, so the lint groups don't build the whole
                # chain.
                if web_ui="$(nix build --no-link --print-out-paths .#web-ui-bundle)"; then
                  run_check "release build" release_build "$web_ui"
                else
                  echo "::error::web UI bundle failed"
                  status=1
                fi
                run_check "partition table" check_partition_table
                run_check "storage test fixtures" check_storage_testdata
                if [ "$release_built" -eq 1 ]; then
                  run_check "images" check_images
                else
                  echo "::error::images not checked: no release build"
                  status=1
                fi
                ;;
              rust-test)
                run_check "host tests" cargo test --all-features \
                  "''${host_crates[@]/#/--package=}" --target "$host_target"
                ;;
              web-test)
                run_check "web tests" nix build --no-link .#web-test
                ;;
              *)
                echo "unknown check group: $group (expected lint, build, test, rust-lint, web-lint, rust-test or web-test)" >&2
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
