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
