{ config, inputs, lib, ... }:
let
  inherit (config.eveningstar) board;

  appPartition = (lib.findFirst (p: p.type == "app")
    (throw "eveningstar.board.partitions has no app partition")
    board.partitions).name;

  # Only what cargo needs, so editing nix/ or docs does not rebuild the app.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../.cargo
      ../crates
    ];
  };
in
{
  perSystem = { config, pkgs, ... }:
    let
      inherit (config.packages) partition-table;

      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../rust-toolchain.toml;

      cargoDeps = pkgs.rustPlatform.importCargoLock {
        lockFile = ../Cargo.lock;
      };

      # ELF to app image, checked against the app partition it will be written to.
      saveImageArgs = lib.escapeShellArgs [
        "--chip" board.chip
        "--flash-size" (lib.toLower board.flash.size)
        "--flash-mode" board.flash.mode
        "--flash-freq" "${board.flash.freq}hz"
        "--partition-table" "${partition-table}/partitions.csv"
        "--partition-table-offset" board.partitionTableOffset
        "--target-app-partition" appPartition
        "--skip-update-check"
      ];
    in
    {
      packages.app = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-app";
        version = (lib.importTOML ../Cargo.toml).workspace.package.version;
        inherit src;

        nativeBuildInputs = [ toolchain pkgs.espflash ];

        # Stamped into the app descriptor as the build time.
        SOURCE_DATE_EPOCH = toString inputs.self.lastModified;

        # Crates come from the vendored lockfile, copied in because the vendor
        # directory is symlinks to one store path per crate. Build paths are
        # remapped so the ELF carries no references to the build inputs. Cargo
        # appends these rustflags to the ones in .cargo/config.toml.
        configurePhase = ''
          runHook preConfigure
          vendor=$NIX_BUILD_TOP/vendor
          cp -rL --no-preserve=mode ${cargoDeps} $vendor
          export CARGO_HOME=$NIX_BUILD_TOP/cargo-home
          mkdir -p $CARGO_HOME
          cat > $CARGO_HOME/config.toml <<EOF
          [source.crates-io]
          replace-with = "vendored"

          [source.vendored]
          directory = "$vendor"

          [build]
          rustflags = [
            "--remap-path-prefix", "$vendor=/vendor",
            "--remap-path-prefix", "${toolchain}=/toolchain",
            "--remap-path-prefix", "$PWD=/source",
          ]
          EOF
          runHook postConfigure
        '';

        buildPhase = ''
          runHook preBuild
          cargo build --release --locked --offline --package eveningstar
          runHook postBuild
        '';

        installPhase = ''
          runHook preInstall
          mkdir -p $out
          cp target/riscv32imac-unknown-none-elf/release/eveningstar $out/eveningstar.elf
          espflash save-image ${saveImageArgs} $out/eveningstar.elf $out/eveningstar.bin
          runHook postInstall
        '';

        # Built for another chip: nothing for the host fixup steps to do.
        dontFixup = true;

        passthru = { inherit saveImageArgs; };
      };
    };
}
