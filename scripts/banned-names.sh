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

names=()
while IFS= read -r entry || [[ -n $entry ]]; do
  entry="${entry#"${entry%%[![:space:]]*}"}"
  entry="${entry%"${entry##*[![:space:]]}"}"
  [[ -z $entry ]] && continue
  if ! [[ $entry =~ ^[a-z0-9-]+$ ]]; then
    echo "BANNED_NAMES has an entry that is not [a-z0-9-]+" >&2
    exit 2
  fi
  names+=(-e "$entry")
done < <(printf '%s\n' "${BANNED_NAMES-}" | tr ',' '\n' | tr '[:upper:]' '[:lower:]')

if [[ ${#names[@]} -eq 0 ]]; then
  if [[ ${BANNED_NAMES_FORK_PR-} == true ]]; then
    echo "::notice::name check skipped: secrets are not available on a fork pull request"
    exit 0
  fi
  echo "BANNED_NAMES is empty: failing closed" >&2
  exit 2
fi

status=0
files=0
git grep -l -z -I -i -w -E "${names[@]}" >/dev/null 2>&1 || files=$?
if [[ $files -gt 1 ]]; then
  echo "name check failed to run" >&2
  exit 2
fi
if [[ $files -eq 0 ]]; then
  while IFS= read -r -d '' path; do
    { git grep -n -h -I -i -w -E "${names[@]}" -- ":(literal)$path" 2>/dev/null || true; } |
      cut -d: -f1 |
      while IFS= read -r line; do
        echo "$path:$line"
      done
  done < <(git grep -l -z -I -i -w -E "${names[@]}" 2>/dev/null)
  echo "banned name in file contents" >&2
  status=1
fi
exit "$status"
