#!/usr/bin/env bash
# Stop and delete the scratch herdr session created by seed.sh.
#
# Usage: scripts/scratch/teardown.sh [session-name]
set -euo pipefail

SESSION="${1:-${PENDANT_SCRATCH_SESSION:-pendant-shell-g1}}"
HERDR="${HERDR_BIN:-$(command -v herdr)}"

if "$HERDR" --session "$SESSION" api snapshot >/dev/null 2>&1; then
    "$HERDR" --session "$SESSION" server stop >/dev/null 2>&1 || true
fi
"$HERDR" session delete "$SESSION" >/dev/null 2>&1 || true
echo "session $SESSION stopped and deleted"
