{ lib, ... }:
let
  # What npm needs to resolve the packages, and nothing else, so editing the
  # UI's source does not reinstall them.
  npmFiles = dir: lib.fileset.toSource {
    root = dir;
    fileset = lib.fileset.unions [
      (dir + "/.npmrc")
      (dir + "/package.json")
      (dir + "/package-lock.json")
    ];
  };

  # The UI's own files. The brand artwork is only the source of the inline
  # SVG, so it is not built from.
  webSrc = lib.fileset.toSource {
    root = ../web;
    fileset = lib.fileset.difference ../web ../web/brand;
  };

  lintSrc = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../web
      ../tools/openapi-typescript/.npmrc
      ../tools/openapi-typescript/package.json
      ../tools/openapi-typescript/package-lock.json
      ../nix/web/lint.mjs
    ];
  };

  # The firmware serves the page whole, so it must stay small.
  budgetBytes = 64 * 1024;
in
{
  perSystem = { config, pkgs, ... }:
    let
      # Pinned by the lockfile of the flake; package.json's engines name its
      # version, so npm refuses to install with another.
      nodejs = pkgs.nodejs;

      # Install scripts never run: the tree is only unpacked.
      installTree = dir: pkgs.importNpmLock.buildNodeModules {
        npmRoot = npmFiles dir;
        inherit nodejs;
        derivationArgs.npmFlags = [ "--ignore-scripts" ];
      };

      # Declarations only, so a compromised generator can at worst produce
      # wrong types.
      generatorModules = installTree ../tools/openapi-typescript;
      webModules = installTree ../web;

      # Puts the installed packages and the generated types where the UI's
      # sources expect them.
      linkTree = ''
        ln -s ${config.packages.web-node-modules}/node_modules node_modules
        mkdir -p src/api/generated
        cp ${config.packages.web-types}/v1.d.ts src/api/generated/v1.d.ts
      '';

      mkWebCheck = name: script: pkgs.stdenv.mkDerivation {
        pname = "eveningstar-${name}";
        version = "0";
        src = webSrc;
        nativeBuildInputs = [ nodejs ];
        dontConfigure = true;
        dontFixup = true;
        buildPhase = ''
          runHook preBuild
          ${linkTree}
          ${script}
          runHook postBuild
        '';
        installPhase = "touch $out";
      };
    in
    {
      packages.web-node-modules = webModules;

      packages.web-types = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-web-types";
        version = "0";
        dontUnpack = true;
        dontConfigure = true;
        dontFixup = true;
        nativeBuildInputs = [ nodejs ];
        buildPhase = ''
          runHook preBuild
          ${generatorModules}/node_modules/.bin/openapi-typescript \
            ${config.packages.openapi}/v1.json -o v1.d.ts
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          install -Dm644 v1.d.ts $out/v1.d.ts
          runHook postInstall
        '';
      };

      packages.web-ui = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-web-ui";
        version = "0";
        src = webSrc;
        nativeBuildInputs = [ nodejs ];
        dontConfigure = true;
        dontFixup = true;
        buildPhase = ''
          runHook preBuild
          ${linkTree}
          node_modules/.bin/vite build
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          cp -r dist $out
          runHook postInstall
        '';
      };

      # The page as the firmware serves it: gzipped, with its ETag and the
      # policy admitting exactly its inline script and style.
      packages.web-ui-bundle = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-web-ui-bundle";
        version = "0";
        dontUnpack = true;
        dontConfigure = true;
        dontFixup = true;
        nativeBuildInputs = [ nodejs pkgs.zopfli ];
        buildPhase = ''
          runHook preBuild
          node ${../nix/web/inline.mjs} ${config.packages.web-ui} bundle
          zopfli -c bundle/index.html > bundle/index.html.gz
          size=$(stat -c %s bundle/index.html.gz)
          if [ "$size" -gt ${toString budgetBytes} ]; then
            echo "index.html.gz is $size bytes, over the budget of ${toString budgetBytes}" >&2
            exit 1
          fi
          printf '"%s"\n' "$(sha256sum bundle/index.html.gz | cut -c1-16)" > bundle/etag
          {
            echo "index.html     $(stat -c %s bundle/index.html) bytes"
            echo "index.html.gz  $size bytes (budget ${toString budgetBytes})"
          } > bundle/report.txt
          runHook postBuild
        '';
        installPhase = ''
          runHook preInstall
          mkdir -p $out
          cp bundle/index.html.gz bundle/etag bundle/content-security-policy bundle/report.txt $out/
          runHook postInstall
        '';
      };

      packages.web-typecheck = mkWebCheck "web-typecheck" ''
        node_modules/.bin/tsc --noEmit -p .
      '';

      packages.web-test = mkWebCheck "web-test" ''
        find src -name '*.test.ts' -print0 | xargs -0 node --test
      '';

      packages.web-lint = pkgs.stdenv.mkDerivation {
        pname = "eveningstar-web-lint";
        version = "0";
        src = lintSrc;
        nativeBuildInputs = [ nodejs ];
        dontConfigure = true;
        dontFixup = true;
        buildPhase = "node nix/web/lint.mjs .";
        installPhase = "touch $out";
      };
    };
}
