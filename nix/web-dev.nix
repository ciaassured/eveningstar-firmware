{
  perSystem = { config, lib, pkgs, ... }:
    let
      nodejs = pkgs.nodejs;

      # Runs `"$@"` in the directory `dir`, which is all of the repository it
      # may write. The Nix store and the rest of the repository are read-only,
      # the home directory is empty, nothing of the environment is passed on
      # but what the command names, and the network is shared. Packages are
      # code from the internet, so none of it runs with the person's files.
      sandbox = ''
        sandbox() {
          dir="$1"
          shift
          if ! command -v bwrap >/dev/null 2>&1; then
            if [ "''${EVENINGSTAR_UNSANDBOXED:-}" = 1 ]; then
              (cd "$dir" && env -i PATH="${nodejs}/bin" HOME="$HOME" ''${EVENINGSTAR_DEVICE:+EVENINGSTAR_DEVICE="$EVENINGSTAR_DEVICE"} "$@")
              return
            fi
            echo "bubblewrap isn't available here, so this refuses to run npm packages unsandboxed." >&2
            echo "Set EVENINGSTAR_UNSANDBOXED=1 to run them anyway, at your own risk." >&2
            return 1
          fi
          bwrap \
            --unshare-all --share-net --die-with-parent --new-session \
            --clearenv \
            --setenv PATH "${nodejs}/bin" \
            --setenv HOME "$HOME" \
            --setenv SSL_CERT_FILE "''${NIX_SSL_CERT_FILE:-''${SSL_CERT_FILE:-/etc/ssl/certs/ca-certificates.crt}}" \
            ''${EVENINGSTAR_DEVICE:+--setenv EVENINGSTAR_DEVICE "$EVENINGSTAR_DEVICE"} \
            --tmpfs "$HOME" \
            --tmpfs /tmp \
            --ro-bind /nix /nix \
            --ro-bind "$root" "$root" \
            --bind "$dir" "$dir" \
            --ro-bind-try /etc/resolv.conf /etc/resolv.conf \
            --ro-bind-try /run/systemd/resolve /run/systemd/resolve \
            --ro-bind-try /etc/hosts /etc/hosts \
            --ro-bind-try /etc/nsswitch.conf /etc/nsswitch.conf \
            --ro-bind-try /etc/ssl /etc/ssl \
            --ro-bind-try /etc/ca-certificates /etc/ca-certificates \
            --ro-bind-try /etc/pki /etc/pki \
            --ro-bind-try /etc/static /etc/static \
            --proc /proc \
            --dev /dev \
            --chdir "$dir" \
            "$@"
        }
      '';

      # `web-dev` and `web-types` find the built packages and generated types
      # where the sources expect them: links in the repository, to the store.
      repositoryRoot = ''
        root="$(git rev-parse --show-toplevel)"
      '';

      linkTypes = ''
        nix build --out-link "$root/web/src/api/generated" "$root#web-types"
      '';

      linkPackages = ''
        nix build --out-link "$root/web/.nix-node-modules" "$root#web-node-modules"
        ln -sfn .nix-node-modules/node_modules "$root/web/node_modules"
      '';

      runtime = [ pkgs.git ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.bubblewrap ];
    in
    {
      # The dev server, for the web UI, with `/api` forwarded to the device
      # that `EVENINGSTAR_DEVICE` names, such as http://192.168.1.50.
      packages.web-dev = pkgs.writeShellApplication {
        name = "web-dev";
        runtimeInputs = runtime;
        text = ''
          ${repositoryRoot}
          ${sandbox}
          ${linkPackages}
          ${linkTypes}
          # Vite loads its config natively: bundling it would write beside the read-only packages.
          sandbox "$root/web" node node_modules/vite/bin/vite.js --configLoader native "$@"
        '';
      };

      # npm for the lockfiles of web/ and tools/openapi-typescript/: run it in
      # the directory of the one to change. It resolves versions in a copy of
      # the manifest and lockfile, and writes those back; it runs no package
      # code, and the packages the dev server uses are out of its reach.
      packages.web-npm = pkgs.writeShellApplication {
        name = "web-npm";
        runtimeInputs = runtime;
        text = ''
          ${repositoryRoot}
          ${sandbox}
          case "$PWD/" in
            "$root/web/"*) package_dir="$root/web" ;;
            "$root/tools/openapi-typescript/"*) package_dir="$root/tools/openapi-typescript" ;;
            *)
              echo "web-npm: run it in web/ or tools/openapi-typescript/" >&2
              exit 2
              ;;
          esac
          stage="$(mktemp -d)"
          trap 'rm -rf "$stage"' EXIT
          cp "$package_dir/.npmrc" "$package_dir/package.json" "$package_dir/package-lock.json" "$stage/"
          status=0
          sandbox "$stage" npm "$@" --package-lock-only --ignore-scripts || status=$?
          cp "$stage/package.json" "$stage/package-lock.json" "$package_dir/"
          exit "$status"
        '';
      };

      # Builds the types of the API again, after the API changed.
      packages.web-types-link = pkgs.writeShellApplication {
        name = "web-types";
        runtimeInputs = runtime;
        text = ''
          ${repositoryRoot}
          ${linkTypes}
        '';
      };

      # The tools of web development, for the dev shell.
      packages.web-dev-tools = pkgs.symlinkJoin {
        name = "eveningstar-web-dev-tools";
        paths = [
          config.packages.web-dev
          config.packages.web-npm
          config.packages.web-types-link
          nodejs
        ];
      };
    };
}
