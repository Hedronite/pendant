#!/usr/bin/env bash
# What each scratch pane runs. Fixture output only: no operator prompt, no
# typed-command echo, no paths.
#
# Usage: pane.sh <number> <state> <session> <pane>
# Env: HERDR_BIN, TICK_PANE, REFRESH_SECONDS.
set -u
set +m

number="$1"
state="$2"
session="$3"
pane="$4"
herdr="${HERDR_BIN:-herdr}"
tick_pane="${TICK_PANE:-0}"
refresh="${REFRESH_SECONDS:-2}"

# Drop the operator prompt, the echo of the command that started this script,
# and any startup noise before the first line of fixture output. The loop below
# never returns to a shell, so no new prompt is printed.
printf '\033[3J\033[2J\033[H'

printf 'PENDANT-SCRATCH-MARKER-%s\n' "$number"
tick=0
while :; do
    "$herdr" --session "$session" pane report-agent "$pane" \
        --source custom:pendant-scratch --agent "scratch-$number" --state "$state" \
        >/dev/null 2>&1 || true
    if [ "$number" = "$tick_pane" ]; then
        tick=$((tick + 1))
        printf 'PENDANT-SCRATCH-TICK-%s\n' "$tick"
    fi
    sleep "$refresh"
done
