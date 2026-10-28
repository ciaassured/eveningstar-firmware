{
  perSystem = { pkgs, ... }:
    let
      src = pkgs.lib.fileset.toSource {
        root = ../web;
        fileset = pkgs.lib.fileset.unions [
          ../web/.npmrc
          ../web/package.json
          ../web/package-lock.json
          ../web/tsconfig.json
          ../web/vite.config.ts
          ../web/index.html
          ../web/src
        ];
      };

      nodejs = pkgs.nodejs;

      nodeModules = pkgs.importNpmLock.buildNodeModules {
        npmRoot = src;
        inherit nodejs;
        derivationArgs = {
          npmFlags = [ "--ignore-scripts" ];
        };
      };
    in
    {
      packages.web-node-modules = nodeModules;

      # Milestone 0 spike: type check, test and build a minimal Preact project.
      packages.web-spike = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-web-spike";
        version = "0";
        inherit src;
        nativeBuildInputs = [ nodejs ];
        buildPhase = ''
          runHook preBuild
          ln -s ${nodeModules}/node_modules node_modules
          node_modules/.bin/tsc --version
          node_modules/.bin/tsc --noEmit -p .
          node --test src/lib/format.test.ts
          node_modules/.bin/vite build
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          cp -r dist $out
          runHook postInstall
        '';
      };
    };
}
