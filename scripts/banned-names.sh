#!/usr/bin/env bash
# Fail when a name from BANNED_NAMES appears in a tracked file.
#
# The list is the BANNED_NAMES environment variable (the CI secret of that
# name), separated by commas or newlines. An empty list fails closed, except
# on a fork pull request, where secrets are not available:
# BANNED_NAMES_FORK_PR=true skips the check with a notice. A hit prints only
# path:line. This file does not contain the list and does not print it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

raw=()
while IFS= read -r entry || [[ -n $entry ]]; do
  entry="${entry#"${entry%%[![:space:]]*}"}"
  entry="${entry%"${entry##*[![:space:]]}"}"
  [[ -z $entry ]] && continue
  if ! [[ $entry =~ ^[a-z0-9-]+$ ]]; then
    echo "BANNED_NAMES has an entry that is not [a-z0-9-]+" >&2
    exit 2
  fi
  raw+=("$entry")
done < <(printf '%s\n' "${BANNED_NAMES-}" | tr ',' '\n' | tr '[:upper:]' '[:lower:]')

if [[ ${#raw[@]} -eq 0 ]]; then
  if [[ ${BANNED_NAMES_FORK_PR-} == true ]]; then
    echo "::notice::name check skipped: secrets are not available on a fork pull request"
    exit 0
  fi
  echo "BANNED_NAMES is empty: failing closed" >&2
  exit 2
fi

status=0
alt=""
for name in "${raw[@]}"; do
  if [[ -n $alt ]]; then
    alt+="|"
  fi
  alt+="$name"
done
re="(^|[^[:alnum:]])(${alt})($|[^[:alnum:]])"

hits="$(mktemp)"
trap 'rm -f "$hits"' EXIT
if git grep -n -I -i -E "$re" >"$hits" 2>/dev/null; then
  cut -d: -f1,2 "$hits"
  status=1
fi
rm -f "$hits"

while IFS= read -r -d '' path; do
  if printf '%s\n' "$path" | grep -q -i -E "$re"; then
    echo "$path:1"
    status=1
  fi
done < <(git ls-files -z)

if [[ $status -ne 0 ]]; then
  echo "banned name in file contents" >&2
fi
exit "$status"
