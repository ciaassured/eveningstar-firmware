# Fails when a signify secret key is in the files Git would commit or has
# committed, other than the development key's.
#
# A signify secret key's second line is the base64 of 104 bytes beginning `Ed`
# `BK`: 140 characters beginning `RWRCS`. Its first line is an untrusted
# comment ending in `secret key`. Either one, anywhere in any file, is a
# finding, except in keys/eveningstar-dev-fw.sec when that file is exactly a
# comment line and a key line whose keynum is keys/eveningstar-dev-fw.pub's
# and whose KDF rounds are 0, as `signify -G -n` writes them.
#
# Usage: secret-key-scan [--index | --commits RANGE | --unpushed]
#   --index          the files staged for the next commit (the default)
#   --commits RANGE  every commit `git rev-list RANGE` lists
#   --unpushed       every commit on HEAD that no remote branch has

dev_sec=keys/eveningstar-dev-fw.sec
dev_pub=keys/eveningstar-dev-fw.pub

# The two lines, assembled so this file doesn't match them.
key_line='RWRC''S[A-Za-z0-9+/]{40,}'
comment_line='^untrusted comment''[:] .*secret key[[:space:]]*$'

# The bytes of the base64 on line $2 of the file whose contents are $1, as
# lowercase hex.
decoded_hex() {
  sed -n "$2p" <<< "$1" | base64 -d 2>/dev/null | od -An -v -tx1 | tr -d ' \n'
}

# Whether the development secret key, with contents $1, is the committed
# development key: two lines, no passphrase, the public key's keynum.
dev_key_allowed() {
  local sec=$1 pub=$2 sec_hex pub_hex
  [[ $(wc -l <<< "$sec") -eq 2 ]] || return 1
  sec_hex=$(decoded_hex "$sec" 2)
  pub_hex=$(decoded_hex "$pub" 2)
  # 104 bytes: `Ed`, `BK`, the rounds, the salt, the checksum, the keynum and
  # the secret key.
  [[ ${#sec_hex} -eq 208 && ${sec_hex:0:8} == 4564424b ]] || return 1
  [[ ${sec_hex:8:8} == 00000000 ]] || return 1
  [[ ${#pub_hex} -eq 84 && ${sec_hex:64:16} == "${pub_hex:4:16}" ]]
}

# Scans the tree $1 (`--cached` for the index, or a commit), and prints a line
# for each finding.
scan_tree() {
  local where=$1 prefix="" path sec pub
  [[ $where == --cached ]] || prefix="$where:"
  # git grep exits 1 when nothing matches.
  { git grep -l -E -e "$key_line" -e "$comment_line" "$where" -- . || true; } | while read -r path; do
    path=${path#"$prefix"}
    if [[ $path == "$dev_sec" ]]; then
      sec=$(git show "${prefix:-:}$path")
      pub=$(git show "${prefix:-:}$dev_pub" 2>/dev/null || true)
      if dev_key_allowed "$sec" "$pub"; then
        continue
      fi
      echo "${prefix}$path: not the development key as signify -G -n writes it"
    else
      echo "${prefix}$path: holds a signify secret key"
    fi
  done
}

mode=${1:---index}
case "$mode" in
  --index)
    (( $# <= 1 )) || { echo "usage: secret-key-scan [--index | --commits RANGE | --unpushed]" >&2; exit 2; }
    findings=$(scan_tree --cached)
    ;;
  --commits | --unpushed)
    if [[ $mode == --commits ]]; then
      (( $# == 2 )) || { echo "usage: secret-key-scan --commits RANGE" >&2; exit 2; }
      mapfile -t commits < <(git rev-list "$2")
    else
      mapfile -t commits < <(git rev-list HEAD --not --remotes)
    fi
    findings=$(for commit in "${commits[@]}"; do scan_tree "$commit"; done)
    ;;
  *)
    echo "usage: secret-key-scan [--index | --commits RANGE | --unpushed]" >&2
    exit 2
    ;;
esac

if [[ -n $findings ]]; then
  echo "$findings" >&2
  echo "secret-key-scan: a signify secret key is in the repository; only the development key's may be" >&2
  exit 1
fi
