{
  perSystem = { config, pkgs, ... }:
    let
      scan = pkgs.writeShellApplication {
        name = "secret-key-scan";
        runtimeInputs = [ pkgs.git pkgs.coreutils pkgs.gnugrep ];
        text = builtins.readFile ./secret-key-scan.sh;
      };
    in
    {
      # Fails when a signify secret key is in the files Git would commit or
      # has committed, whatever the file's name, other than the development
      # key's: see specs/ota-signing.md, "Secret keys and the repository".
      # Usage: secret-key-scan [--index | --commits RANGE | --unpushed]
      packages.secret-key-scan = scan;

      # The scan over a repository of generated fixtures: the development key
      # passes, and a release key under its own name, renamed, pasted into
      # another file, copied over the development key, appended to it, a
      # development key with a passphrase, a lone comment line, and a key
      # committed and removed again are each found.
      packages.secret-key-scan-test = pkgs.runCommand "secret-key-scan-test"
        {
          nativeBuildInputs = [ scan pkgs.git pkgs.signify pkgs.coreutils ];
        } ''
        export HOME=$TMPDIR
        git config --global user.name test
        git config --global user.email test@example.com
        git config --global init.defaultBranch main

        mkdir repo && cd repo
        git init -q
        mkdir keys
        signify -G -n -c dev -p keys/eveningstar-dev-fw.pub -s keys/eveningstar-dev-fw.sec
        signify -G -n -c release -p "$TMPDIR/eveningstar-1-fw.pub" -s "$TMPDIR/eveningstar-1-fw.sec"
        release=$TMPDIR/eveningstar-1-fw.sec
        echo "a file" > notes.md
        git add -A && git commit -qm base

        passes() {
          secret-key-scan "$@" || { echo "FAIL: refused the clean tree ($*)"; exit 1; }
        }
        refuses() {
          local case=$1
          shift
          if secret-key-scan "$@"; then
            echo "FAIL: passed $case"
            exit 1
          fi
          echo "ok: refused $case"
        }
        # Stages a change for one case, scans the index, and undoes it.
        case_of() {
          local case=$1
          shift
          "$@"
          git add -A
          refuses "$case" --index
          git reset -q --hard
          git clean -qfd
        }

        passes --index
        passes --commits HEAD~0
        case_of "a release key under its own name" cp "$release" keys/eveningstar-1-fw.sec
        case_of "a release key renamed" cp "$release" keys/notes.txt
        case_of "a release key pasted into another file" sh -c "cat '$release' >> notes.md"
        case_of "an indented key line" sh -c "sed -n 2p '$release' | sed 's/^/    /' >> notes.md"
        case_of "a release key over the development key" cp "$release" keys/eveningstar-dev-fw.sec
        case_of "a release key appended to the development key" \
          sh -c "cat '$release' >> keys/eveningstar-dev-fw.sec"
        case_of "a lone secret key comment line" \
          sh -c "echo 'untrusted comment: someone secret key' >> notes.md"

        # The development key with 42 KDF rounds, as a key with a passphrase.
        rounds() {
          {
            sed -n 1p keys/eveningstar-dev-fw.sec
            sed -n 2p keys/eveningstar-dev-fw.sec | base64 -d \
              | { head -c 4; printf '\x00\x00\x00\x2a'; tail -c +9; } | base64 -w0
            echo
          } > "$TMPDIR/rounds.sec"
          cp "$TMPDIR/rounds.sec" keys/eveningstar-dev-fw.sec
        }
        case_of "a development key with a passphrase" rounds

        # A key committed and removed again.
        cp "$release" keys/eveningstar-1-fw.sec
        git add -A && git commit -qm "add a key"
        git rm -q keys/eveningstar-1-fw.sec && git commit -qm "remove it"
        passes --index
        refuses "a key committed and removed again" --commits HEAD~2..HEAD

        touch $out
      '';
    };
}
