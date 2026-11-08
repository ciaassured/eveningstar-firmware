{
  perSystem = { pkgs, ... }:
    let
      scan = pkgs.writeShellApplication {
        name = "secret-key-scan";
        runtimeInputs = [ pkgs.git pkgs.coreutils pkgs.gnugrep pkgs.gnused ];
        text = builtins.readFile ./secret-key-scan.sh;
      };
    in
    {
      # Fails when a signify secret key is in the files Git would commit or
      # has committed, whatever the file's name, other than the development
      # key's: see specs/ota-signing.md, "Secret keys and the repository".
      # Usage: secret-key-scan [--index | --commits REV... | --unpushed | --pre-push]
      packages.secret-key-scan = scan;

      # The scan over a repository of generated fixtures. The development key
      # passes. Found, each with a finding naming the file: a release key under
      # its own name, renamed, pasted into another file, indented, over the
      # development key, appended to it, put before it, on its key line, or in
      # its comment; a development key with a passphrase; a lone comment line;
      # a key committed and removed again; a key on a branch a push sends while
      # another is checked out. A Git error fails the scan.
      packages.secret-key-scan-test = pkgs.runCommand "secret-key-scan-test"
        {
          nativeBuildInputs = [ scan pkgs.git pkgs.signify pkgs.coreutils pkgs.gnugrep pkgs.gnused ];
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
        release_line=$(sed -n 2p "$release")
        dev_sec=keys/eveningstar-dev-fw.sec
        echo "a file" > notes.md
        git add -A && git commit -qm base

        passes() {
          secret-key-scan "$@" || { echo "FAIL: refused the clean tree ($*)"; exit 1; }
        }
        # Runs the scan with the arguments after the first two, which has to
        # exit 1 with a finding naming the file $2.
        refuses() {
          local case=$1 file=$2 status=0
          shift 2
          secret-key-scan "$@" 2> "$TMPDIR/out" || status=$?
          if (( status != 1 )) || ! grep -qF "$file:" "$TMPDIR/out"; then
            echo "FAIL: $case: exit $status"
            cat "$TMPDIR/out"
            exit 1
          fi
          echo "ok: refused $case"
        }
        # Makes a change with the arguments after the first two, stages it,
        # scans the index, and undoes it.
        case_of() {
          local case=$1 file=$2
          shift 2
          "$@"
          git add -A
          refuses "$case" "$file" --index
          git reset -q --hard
          git clean -qfd
        }

        passes --index
        passes --commits HEAD
        case_of "a release key under its own name" keys/eveningstar-1-fw.sec \
          cp "$release" keys/eveningstar-1-fw.sec
        case_of "a release key renamed" keys/notes.txt cp "$release" keys/notes.txt
        case_of "a release key pasted into another file" notes.md \
          sh -c "cat '$release' >> notes.md"
        case_of "an indented key line" notes.md \
          sh -c "echo '    $release_line' >> notes.md"
        case_of "a release key over the development key" "$dev_sec" cp "$release" "$dev_sec"
        case_of "a release key appended to the development key" "$dev_sec" \
          sh -c "cat '$release' >> $dev_sec"
        case_of "a release key line before the development key's" "$dev_sec" \
          sh -c "{ echo '$release_line'; sed -n 2p $dev_sec; } > $TMPDIR/f && mv $TMPDIR/f $dev_sec"
        case_of "a release key on the development key's line" "$dev_sec" \
          sh -c "{ sed -n 1p $dev_sec; echo \"\$(sed -n 2p $dev_sec) $release_line\"; } > $TMPDIR/f && mv $TMPDIR/f $dev_sec"
        case_of "a release key in the development key's comment" "$dev_sec" \
          sh -c "{ echo 'untrusted comment: $release_line'; sed -n 2p $dev_sec; } > $TMPDIR/f && mv $TMPDIR/f $dev_sec"
        case_of "a lone secret key comment line" notes.md \
          sh -c "echo 'untrusted comment: someone secret key' >> notes.md"

        # The development key with 42 KDF rounds, as a key with a passphrase.
        rounds() {
          sed -n 2p "$dev_sec" | base64 -d > "$TMPDIR/key"
          { head -c 4 "$TMPDIR/key"; printf '\x00\x00\x00\x2a'; tail -c +9 "$TMPDIR/key"; } > "$TMPDIR/rounds"
          [[ $(wc -c < "$TMPDIR/rounds") -eq 104 ]] || { echo "FAIL: the rounds fixture isn't 104 bytes"; exit 1; }
          { sed -n 1p "$dev_sec"; base64 -w0 "$TMPDIR/rounds"; echo; } > "$TMPDIR/rounds.sec"
          cp "$TMPDIR/rounds.sec" "$dev_sec"
        }
        case_of "a development key with a passphrase" "$dev_sec" rounds

        # A key committed and removed again.
        cp "$release" keys/eveningstar-1-fw.sec
        git add -A && git commit -qm "add a key"
        git rm -q keys/eveningstar-1-fw.sec && git commit -qm "remove it"
        passes --index
        refuses "a key committed and removed again" keys/eveningstar-1-fw.sec --commits HEAD~2..HEAD

        # A push of a branch other than the one checked out.
        git checkout -q -b feature
        cp "$release" notes.txt
        git add -A && git commit -qm "a key on a branch"
        feature=$(git rev-parse HEAD)
        git checkout -q main
        printf 'refs/heads/feature %s refs/heads/feature %040d\n' "$feature" 0 > "$TMPDIR/push"
        refuses "a key on a pushed branch that isn't checked out" notes.txt --pre-push < "$TMPDIR/push"

        # Git errors fail the scan, rather than passing with nothing scanned.
        status=0
        secret-key-scan --commits nosuchref 2>/dev/null || status=$?
        (( status == 2 )) || { echo "FAIL: a bad revision exited $status"; exit 1; }
        echo "ok: a bad revision fails the scan"

        touch $out
      '';
    };
}
