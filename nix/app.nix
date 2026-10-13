{ config, inputs, lib, ... }:
let
  inherit (config.eveningstar) board;

  appPartition = (lib.findFirst (p: p.type == "app")
    (throw "eveningstar.board.partitions has no app partition")
    board.partitions).name;

  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  cargoConfig = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../.cargo
  ];

  # Only what cargo needs, so editing nix/ or docs does not rebuild the app.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [ cargoConfig ../crates ];
  };

  # Just the manifests, so the dependency build only changes when they do.
  depsSrc = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      cargoConfig
      (lib.fileset.fileFilter (file: file.name == "Cargo.toml") ../crates)
    ];
  };

  # Empty stand-ins for each workspace crate's targets, matching the ones the
  # real crate has so cargo resolves the same targets and features.
  members = lib.attrNames (lib.filterAttrs (_: type: type == "directory") (builtins.readDir ../crates));
  stubs = lib.concatMapStrings
    (member:
      let
        dir = ../crates + "/${member}";
        has = path: builtins.pathExists (dir + "/${path}");
      in
      ''
        mkdir -p crates/${member}/src
      ''
      + lib.optionalString (has "src/lib.rs") ''
        echo '#![no_std]' > crates/${member}/src/lib.rs
      ''
      + lib.optionalString (has "src/main.rs") ''
        cat > crates/${member}/src/main.rs <<'EOF'
        #![no_std]
        #![no_main]
        #[panic_handler]
        fn panic(_: &core::panic::PanicInfo) -> ! {
            loop {}
        }
        EOF
      ''
      + lib.optionalString (has "build.rs") ''
        echo 'fn main() {}' > crates/${member}/build.rs
      '')
    members;
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

      # Crates come from the vendored lockfile, copied in because the vendor
      # directory is symlinks to one store path per crate. Build paths are
      # remapped so the ELF carries no references to the build inputs. Cargo
      # appends these rustflags to the ones in .cargo/config.toml.
      #
      # Both builds must set up cargo identically, at the same paths, or cargo
      # will not reuse the dependencies. Store timestamps are kept on the
      # vendored crates so build scripts watching them stay fresh.
      configurePhase = ''
        runHook preConfigure
        vendor=$NIX_BUILD_TOP/vendor
        cp -rL --no-preserve=mode --preserve=timestamps ${cargoDeps} $vendor
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

      cargoArgs = "--release --locked --offline --package eveningstar";

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
      # Every dependency compiled against stub workspace crates, as a cargo
      # target directory the app build starts from. The stub binary cannot be
      # linked without std, so it is only checked; the dependencies are still
      # compiled in full, exactly as `cargo build` would.
      packages.app-deps = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-app-deps";
        inherit version configurePhase;
        src = depsSrc;

        nativeBuildInputs = [ toolchain ];

        # Fixed, so a new commit does not rebuild the dependencies. The app
        # build sets the real time; only the crates whose build scripts watch
        # it are rebuilt there.
        SOURCE_DATE_EPOCH = "0";

        postPatch = stubs;

        buildPhase = ''
          runHook preBuild
          cargo rustc ${cargoArgs} --bin eveningstar -- --emit=metadata
          runHook postBuild
        '';

        installPhase = ''
          runHook preInstall
          mkdir -p $out
          cp -r target $out/target
          runHook postInstall
        '';

        dontFixup = true;
      };

      packages.app = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-app";
        inherit version src configurePhase;

        nativeBuildInputs = [ toolchain pkgs.espflash ];

        # Stamped into the app descriptor as the build time.
        SOURCE_DATE_EPOCH = toString inputs.self.lastModified;

        # Cargo decides whether a workspace crate is stale by comparing source
        # and output timestamps. Everything copied out of the store has the same
        # timestamp, so the stubs would look up to date; touching the real
        # sources makes them newer.
        preBuild = ''
          cp -r --preserve=mode,timestamps ${config.packages.app-deps}/target target
          chmod -R u+w target
          find crates -type f -exec touch {} +
        '';

        buildPhase = ''
          runHook preBuild
          cargo build ${cargoArgs}
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
