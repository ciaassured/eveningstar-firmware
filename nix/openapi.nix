{ inputs, lib, ... }:
let
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  # The whole workspace, as cargo reads every member's manifest and targets.
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
  perSystem = { pkgs, ... }:
    let
      toolchain = (inputs.rust-overlay.lib.mkRustBin { } pkgs)
        .fromRustupToolchainFile ../rust-toolchain.toml;

      cargoDeps = pkgs.rustPlatform.importCargoLock {
        lockFile = ../Cargo.lock;
      };
    in
    {
      # The OpenAPI document of version 1 of the API, as `v1.json`, printed by
      # eveningstar-api's `openapi` example on the build machine.
      packages.openapi = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-openapi";
        inherit version src;

        nativeBuildInputs = [ toolchain ];

        # Crates come from the vendored lockfile, copied in because the vendor
        # directory is symlinks to one store path per crate.
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
          EOF
          runHook postConfigure
        '';

        buildPhase = ''
          runHook preBuild
          cargo run --locked --offline --package eveningstar-api --example openapi \
            --features schema --target "$(rustc --print host-tuple)" > v1.json
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
