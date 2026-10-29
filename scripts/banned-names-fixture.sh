#!/usr/bin/env bash
# Show that a planted token fails the name check, and that an empty list
# fails closed except on a fork pull request. The token is not a secret.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/banned-names.sh"
token="zzplantedtoken"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/scripts"
cp "$script" "$work/scripts/banned-names.sh"
git -C "$work" init -q
printf '%s\n' "$token" > "$work/hit.txt"
git -C "$work" add hit.txt
git -C "$work" -c user.name=fixture -c user.email=fixture@example.com commit -q -m fixture

hit="$(mktemp)"
set +e
(cd "$work" && BANNED_NAMES="$token" scripts/banned-names.sh >"$hit")
code=$?
set -e
if [[ $code -ne 1 ]]; then
  echo "planted token should fail the script" >&2
  exit 1
fi
if [[ "$(cat "$hit")" != "hit.txt:1" ]]; then
  echo "a hit must print path:line only" >&2
  exit 1
fi

set +e
(cd "$work" && BANNED_NAMES= scripts/banned-names.sh >/dev/null)
empty=$?
set -e
if [[ $empty -ne 2 ]]; then
  echo "an empty list should fail closed" >&2
  exit 1
fi

(cd "$work" && BANNED_NAMES= BANNED_NAMES_FORK_PR=true scripts/banned-names.sh >/dev/null)

expect_fail() {
  local label="$1"
  shift
  local case_dir
  case_dir="$(mktemp -d)"
  mkdir -p "$case_dir/scripts"
  cp "$script" "$case_dir/scripts/banned-names.sh"
  git -C "$case_dir" init -q
  "$@" "$case_dir"
  git -C "$case_dir" add -A
  git -C "$case_dir" -c user.name=fixture -c user.email=fixture@example.com commit -q -m fixture
  local out code
  out="$(mktemp)"
  set +e
  (cd "$case_dir" && BANNED_NAMES="$token" scripts/banned-names.sh >"$out")
  code=$?
  set -e
  if [[ $code -ne 1 ]]; then
    echo "$label should fail the script" >&2
    exit 1
  fi
  if [[ ! -s $out ]]; then
    echo "$label produced no path:line" >&2
    exit 1
  fi
  while IFS= read -r row; do
    if ! [[ $row =~ ^[^:]+:[0-9]+$ ]]; then
      echo "$label must print path:line only" >&2
      exit 1
    fi
  done <"$out"
  rm -rf "$case_dir" "$out"
}

expect_fail "path segment" bash -c 'mkdir -p "$0/docs" && printf plain > "$0/docs/'"$token"'-notes.txt"'
expect_fail "directory segment" bash -c 'mkdir -p "$0/config/'"$token"'" && printf plain > "$0/config/'"$token"'/toml.txt"'
expect_fail "identifier" bash -c 'printf "let %s_socket = 1;\n" "'"$token"'" > "$0/id.txt"'
expect_fail "upper identifier" bash -c 'printf "const %s_MESH: u8 = 1;\n" "'"$(printf '%s' "$token" | tr '[:lower:]' '[:upper:]')"'" > "$0/upper.txt"'

neg="$(mktemp -d)"
mkdir -p "$neg/scripts"
cp "$script" "$neg/scripts/banned-names.sh"
git -C "$neg" init -q
printf 'let %sextra = 1;\n' "$token" > "$neg/wide.txt"
git -C "$neg" add wide.txt
git -C "$neg" -c user.name=fixture -c user.email=fixture@example.com commit -q -m fixture
(cd "$neg" && BANNED_NAMES="$token" scripts/banned-names.sh >/dev/null)
rm -rf "$neg"
