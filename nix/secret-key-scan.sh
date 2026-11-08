# Fails when a signify secret key is in the files Git would commit or has
# committed, other than the development key's.
#
# A signify secret key's second line is the base64 of 104 bytes beginning `Ed`
# `BK`: 140 characters beginning `RWRCS`. Its first line is an untrusted
# comment ending in `secret key`. Either one, anywhere in any file, is a
# finding; the key line matches from `RWRCSw` (any rounds below 2^24) and 40
# more base64 characters, so a key wrapped at 46 columns or more is found too.
# The one exception is
# keys/eveningstar-dev-fw.sec when that file is exactly a comment line and a
# key line whose keynum is keys/eveningstar-dev-fw.pub's and whose KDF rounds
# are 0, as `signify -G -n` writes them.
#
# A Git command that fails fails the scan (exit 2): nothing passes unscanned.
#
# Usage: secret-key-scan [--index | --commits REV... | --unpushed | --pre-push [REMOTE]]
#   --index        the files staged for the next commit (the default)
#   --commits REV  every commit `git rev-list REV...` lists
#   --unpushed     every commit on HEAD that no remote branch has
#   --pre-push     every commit of the refs a push to REMOTE sends, read from
#                  a pre-push hook's standard input; a new ref's commits that
#                  REMOTE's tracking branches (every remote's, without REMOTE)
#                  don't have

dev_sec=keys/eveningstar-dev-fw.sec
dev_pub=keys/eveningstar-dev-fw.pub

# The two lines, assembled so this file doesn't match them.
key_line='RWRC''Sw[A-Za-z0-9+/]{40,}'
comment_line='^untrusted comment''[:] .*secret key[[:space:]]*$'

usage() {
  echo "usage: secret-key-scan [--index | --commits REV... | --unpushed | --pre-push [REMOTE]]" >&2
  exit 2
}

# The bytes of the base64 line $1, as lowercase hex.
decoded_hex() {
  base64 -d <<< "$1" 2>/dev/null | od -An -v -tx1 | tr -d ' \n'
}

# Whether the development secret key, with contents $1, is the committed
# development key: a comment line and one 140-character key line, no
# passphrase, the public key's keynum.
dev_key_allowed() {
  local sec=$1 pub=$2 comment line sec_hex pub_hex
  [[ $(wc -l <<< "$sec") -eq 2 ]] || return 1
  comment=$(sed -n 1p <<< "$sec")
  line=$(sed -n 2p <<< "$sec")
  [[ $comment =~ ^untrusted\ comment:\  ]] || return 1
  ! grep -qE "$key_line" <<< "$comment" || return 1
  [[ $line =~ ^[A-Za-z0-9+/]{139}=$ ]] || return 1
  sec_hex=$(decoded_hex "$line")
  pub_hex=$(decoded_hex "$(sed -n 2p <<< "$pub")")
  # 104 bytes: `Ed`, `BK`, the rounds, the salt, the checksum, the keynum and
  # the secret key.
  [[ ${#sec_hex} -eq 208 && ${sec_hex:0:8} == 4564424b ]] || return 1
  [[ ${sec_hex:8:8} == 00000000 ]] || return 1
  [[ ${#pub_hex} -eq 84 && ${sec_hex:64:16} == "${pub_hex:4:16}" ]]
}

findings=""

# Scans the tree $1 (`--cached` for the index, or a commit), adding a line to
# `findings` for each finding.
scan_tree() {
  local where=$1 prefix="" matches status=0 path sec pub
  [[ $where == --cached ]] || prefix="$where:"
  # git grep exits 1 when nothing matches, and more on an error.
  matches=$(git grep -l -E -e "$key_line" -e "$comment_line" "$where" -- .) || status=$?
  if (( status > 1 )); then
    echo "secret-key-scan: git grep failed on $where" >&2
    exit 2
  fi
  while read -r path; do
    [[ -n $path ]] || continue
    path=${path#"$prefix"}
    if [[ $path == "$dev_sec" ]]; then
      sec=$(git show "${prefix:-:}$path")
      pub=$(git show "${prefix:-:}$dev_pub" 2>/dev/null || true)
      if dev_key_allowed "$sec" "$pub"; then
        continue
      fi
      findings+="${prefix}$path: not the development key as signify -G -n writes it"$'\n'
    else
      findings+="${prefix}$path: holds a signify secret key"$'\n'
    fi
  done <<< "$matches"
}

# Scans every commit `git rev-list` lists for its arguments.
scan_commits() {
  local list commit
  list=$(git rev-list "$@") || {
    echo "secret-key-scan: git rev-list $* failed" >&2
    exit 2
  }
  while read -r commit; do
    if [[ -n $commit ]]; then
      scan_tree "$commit"
    fi
  done <<< "$list"
}

# The scan reads paths from the repository's root.
root=$(git rev-parse --show-toplevel)
cd "$root"

mode=${1:---index}
case "$mode" in
  --index)
    (( $# <= 1 )) || usage
    scan_tree --cached
    ;;
  --commits)
    (( $# >= 2 )) || usage
    scan_commits "${@:2}"
    ;;
  --unpushed)
    (( $# == 1 )) || usage
    scan_commits HEAD --not --remotes
    ;;
  --pre-push)
    (( $# <= 2 )) || usage
    # Only the tracking branches of the remote pushed to count as published.
    # A remote given as a URL matches none, so everything is scanned.
    published=--remotes
    if (( $# == 2 )); then
      published="--remotes=$2"
    fi
    # A last line without a newline is read too.
    while read -r _local_ref local_sha _remote_ref remote_sha || [[ -n ${local_sha:-} ]]; do
      # A deleted ref sends nothing.
      if [[ $local_sha =~ ^0+$ ]]; then
        continue
      fi
      if [[ $remote_sha =~ ^0+$ ]] || ! git cat-file -e "$remote_sha^{commit}" 2>/dev/null; then
        # A new ref, or one whose remote commit isn't here: everything the
        # remote's tracking branches don't have.
        scan_commits "$local_sha" --not "$published"
      else
        scan_commits "$remote_sha..$local_sha"
      fi
    done
    ;;
  *)
    usage
    ;;
esac

if [[ -n $findings ]]; then
  printf '%s' "$findings" >&2
  echo "secret-key-scan: a signify secret key is in the repository; only the development key's may be" >&2
  exit 1
fi
