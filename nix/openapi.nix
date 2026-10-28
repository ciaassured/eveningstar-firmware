{ inputs, lib, ... }:
let
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  workspace = import ./_workspace.nix { inherit lib; };
  inherit (workspace) cargoConfig;

  # Only the API crate and the manifests of the other members, so editing the
  # firmware does not rebuild the document or anything built from it.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      cargoConfig
      workspace.manifests
      ../crates/eveningstar-api
    ];
  };

  depsSrc = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [ cargoConfig workspace.manifests ];
  };

  # The members that stand in for the real ones in the source tree.
  others = lib.remove "eveningstar-api" workspace.members;

  cargoArgs = ''
    --locked --offline --package eveningstar-api --example openapi \
      --features schema --target "$(rustc --print host-tuple)"'';
in
{
  perSystem = { config, pkgs, ... }:
    let
      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../rust-toolchain.toml;

      cargoDeps = pkgs.rustPlatform.importCargoLock {
        lockFile = ../Cargo.lock;
      };

      # Crates come from the vendored lockfile, copied in because the vendor
      # directory is symlinks to one store path per crate. Both builds must
      # set up cargo identically, or cargo will not reuse the dependencies.
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
        EOF
        runHook postConfigure
      '';
    in
    {
      # Every dependency of the document's generator compiled against stub
      # workspace crates, as a cargo target directory the document build
      # starts from.
      packages.openapi-deps = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-openapi-deps";
        inherit version configurePhase;
        src = depsSrc;

        nativeBuildInputs = [ toolchain ];

        postPatch = workspace.stubs;

        buildPhase = ''
          runHook preBuild
          cargo rustc ${cargoArgs} -- --emit=metadata
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

      # The OpenAPI document of version 1 of the API, as `v1.json`, printed by
      # eveningstar-api's `openapi` example on the build machine.
      packages.openapi = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-openapi";
        inherit version src configurePhase;

        nativeBuildInputs = [ toolchain ];

        # The other members only need to exist for cargo to load the workspace.
        postPatch = workspace.stubsFor others;

        # Cargo decides whether a workspace crate is stale by comparing source
        # and output timestamps; touching the real sources makes them newer
        # than the stubs the dependency build compiled.
        preBuild = ''
          cp -r --preserve=mode,timestamps ${config.packages.openapi-deps}/target target
          chmod -R u+w target
          find crates/eveningstar-api -type f -exec touch {} +
        '';

        buildPhase = ''
          runHook preBuild
          cargo run ${cargoArgs} > v1.json
          runHook postBuild
        '';

        installPhase = ''
          runHook preInstall
          install -Dm644 v1.json $out/v1.json
          runHook postInstall
        '';
      };
    };
}
