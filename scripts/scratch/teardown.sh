#!/usr/bin/env bash
# Stop and delete the scratch herdr session created by seed.sh.
#
# Usage: scripts/scratch/teardown.sh [session-name]
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
SESSION="$(scratch_session_name "$@")" || exit 1
HERDR="${HERDR_BIN:-$(command -v herdr)}"

# seed.sh records the per-seed bin directory inside the session directory; read
# it before deleting the session.
bin_state="$HOME/.config/herdr/sessions/$SESSION/scratch-bin.path"
bin_dir=""
if [ -f "$bin_state" ]; then
    bin_dir="$(cat "$bin_state")"
fi

if "$HERDR" --session "$SESSION" api snapshot >/dev/null 2>&1; then
    "$HERDR" --session "$SESSION" server stop >/dev/null 2>&1 || true
fi
"$HERDR" session delete "$SESSION" >/dev/null 2>&1 || true

case "$bin_dir" in
    "") ;;
    */pendant-scratch-bin.*) rm -rf "$bin_dir" ;;
    *) echo "refusing to remove unexpected bin dir: $bin_dir" >&2 ;;
esac
echo "session $SESSION stopped and deleted"
